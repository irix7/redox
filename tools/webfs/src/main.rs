//! webfs: a read-only filesystem served as content-addressed blobs over HTTP
//! or HTTPS.
//!
//! On the build host `pack` turns a directory into a store and `serve` is
//! its origin server. On Redox `mount` is the scheme daemon. `ls`, `cat` and
//! `get` read a store or a URL from either side.

mod dns;
mod format;
mod http;
#[cfg(not(target_os = "redox"))]
mod pack;
#[cfg(target_os = "redox")]
mod scheme;
#[cfg(not(target_os = "redox"))]
mod serve;
mod store;
mod tls;
mod trust;

use std::io::Write;
use std::net::IpAddr;
use std::process::ExitCode;
use std::time::Instant;

use format::Kind;
use http::{Client, Url};
use store::Store;
use trust::TrustStore;

const USAGE: &str = "usage:
  webfs pack DIR STORE                 write DIR into STORE as blobs and a root file
  webfs serve STORE [--listen ADDR] [--root-max-age SECONDS] [--quiet]
                                       origin server (default 127.0.0.1:8733, 60 s)
  webfs mount [OPTIONS] URL            scheme daemon, started by init (Redox)
  webfs ls [OPTIONS] URL [PATH]        list a directory of the store at URL
  webfs cat [OPTIONS] URL PATH         write a file of the store to stdout
  webfs get [OPTIONS] URL              GET any URL; print status, size and time

options for mount, ls, cat and get:
  --ca FILE        trust the certificates in FILE too (DER, or PEM blocks)
  --connect IP     connect to IP; the URL still names the host
  --root HASH      accept only this root hash from the server
  --cache KIB      blob cache size (default 1024)
  --name NAME      scheme name reported in paths (default web)
  --threads N      mount: worker threads, one connection each at most (default 4)";

struct Options {
    trust: TrustStore,
    connect: Option<IpAddr>,
    root: Option<format::Hash>,
    cache_kib: usize,
    name: String,
    threads: usize,
    listen: String,
    root_max_age: u32,
    quiet: bool,
    words: Vec<String>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        trust: TrustStore::built_in(),
        connect: None,
        root: None,
        cache_kib: 1024,
        name: "web".to_string(),
        threads: 4,
        listen: "127.0.0.1:8733".to_string(),
        root_max_age: 60,
        quiet: false,
        words: Vec::new(),
    };
    let mut args = args;
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg} needs {what}"));
        match arg.as_str() {
            "--ca" => {
                let file = value("a file")?;
                let data = std::fs::read(&file).map_err(|err| format!("{file}: {err}"))?;
                options
                    .trust
                    .add_file(&data)
                    .map_err(|err| format!("{file}: {err:?}"))?;
            }
            "--connect" => {
                options.connect = Some(value("an address")?.parse().map_err(|_| "bad --connect address")?);
            }
            "--root" => {
                options.root = Some(format::parse_hex(&value("a hash")?).ok_or("bad --root hash")?);
            }
            "--cache" => options.cache_kib = value("a size")?.parse().map_err(|_| "bad --cache size")?,
            "--name" => options.name = value("a name")?,
            "--threads" => options.threads = value("a count")?.parse().map_err(|_| "bad --threads count")?,
            "--listen" => options.listen = value("an address")?,
            "--root-max-age" => {
                options.root_max_age = value("seconds")?.parse().map_err(|_| "bad --root-max-age")?;
            }
            "--quiet" => options.quiet = true,
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => options.words.push(arg),
        }
    }
    Ok(options)
}

fn client(options: &mut Options, url: &str) -> Result<Client, String> {
    let url = Url::parse(url).ok_or(format!("{url}: not an http:// or https:// URL"))?;
    let trust = std::mem::replace(&mut options.trust, TrustStore::empty());
    Ok(Client::new(url, std::sync::Arc::new(trust), options.connect))
}

fn store(options: &mut Options, url: &str) -> Result<Store, String> {
    let url = Url::parse(url).ok_or(format!("{url}: not an http:// or https:// URL"))?;
    let trust = std::mem::replace(&mut options.trust, TrustStore::empty());
    Ok(Store::new(url, trust, options.connect, options.cache_kib * 1024, options.root))
}

fn ls(mut options: Options) -> Result<(), String> {
    let url = options.words.first().cloned().ok_or(USAGE)?;
    let path = options.words.get(1).cloned().unwrap_or_default();
    let store = store(&mut options, &url)?;
    let entry = store.resolve(&path, true).map_err(|err| format!("{path}: {err}"))?;
    let entries = match entry.kind {
        Kind::Dir => store.list(&entry).map_err(|err| format!("{path}: {err}"))?,
        _ => vec![entry],
    };
    for entry in entries {
        let kind = match entry.kind {
            Kind::Dir => 'd',
            Kind::File => '-',
            Kind::Link => 'l',
        };
        println!("{kind} {:o} {:>10} {}", entry.mode, entry.size, entry.name);
    }
    Ok(())
}

fn cat(mut options: Options) -> Result<(), String> {
    let (Some(url), Some(path)) = (options.words.first().cloned(), options.words.get(1).cloned()) else {
        return Err(USAGE.to_string());
    };
    let store = store(&mut options, &url)?;
    let entry = store.resolve(&path, true).map_err(|err| format!("{path}: {err}"))?;
    let mut out = std::io::stdout().lock();
    let mut buf = vec![0; format::CHUNK];
    let mut offset = 0;
    loop {
        let n = store.read(&entry, offset, &mut buf).map_err(|err| format!("{path}: {err}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|err| err.to_string())?;
        offset += n as u64;
    }
    Ok(())
}

fn get(mut options: Options) -> Result<(), String> {
    let url = options.words.first().cloned().ok_or(USAGE)?;
    let mut client = client(&mut options, &url)?;
    let path = client.url.path.clone();
    // Twice: the second request shows what an open connection costs.
    for _ in 0..2 {
        let start = Instant::now();
        let response = client.request(&path, format::MAX_BLOB).map_err(|err| err.to_string())?;
        println!(
            "status {} bytes {} ms {} connects {}",
            response.status,
            response.body.len(),
            start.elapsed().as_millis(),
            client.stats.connects
        );
    }
    Ok(())
}

#[cfg(not(target_os = "redox"))]
fn pack(options: Options) -> Result<(), String> {
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};
    let (Some(src), Some(out)) = (options.words.first(), options.words.get(1)) else {
        return Err(USAGE.to_string());
    };
    // SOURCE_DATE_EPOCH makes a pack reproducible, root file included.
    let time = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|text| text.parse().ok())
        .or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
        .unwrap_or(0);
    let (root, stats) =
        pack::pack(Path::new(src), Path::new(out), time).map_err(|err| err.to_string())?;
    println!(
        "packed {} files, {} directories, {} links: {} bytes of content",
        stats.files, stats.dirs, stats.links, stats.content_bytes
    );
    println!(
        "blobs: {} written ({} bytes), {} already present",
        stats.blobs_written, stats.bytes_written, stats.blobs_present
    );
    println!("root {}", format::hex(&root.hash));
    Ok(())
}

#[cfg(not(target_os = "redox"))]
fn serve(options: Options) -> Result<(), String> {
    let dir = options.words.first().ok_or(USAGE)?;
    let config = serve::Config {
        dir: dir.into(),
        root_max_age: options.root_max_age,
        log: !options.quiet,
    };
    serve::serve(config, &options.listen).map_err(|err| err.to_string())
}

#[cfg(target_os = "redox")]
fn mount(mut options: Options) -> Result<(), String> {
    use redox_scheme::Socket;

    let url = options.words.first().cloned().ok_or(USAGE)?;
    let store = store(&mut options, &url)?;
    let name = options.name;
    let threads = options.threads;
    daemon::SchemeDaemon::new(move |daemon| {
        let socket = Socket::create().expect("webfs: failed to create the scheme socket");
        let mut scheme = scheme::WebScheme::new(name, store);
        daemon
            .ready_sync_scheme(&socket, &mut scheme)
            .expect("webfs: failed to report readiness to init");
        scheme.run(socket, threads)
    })
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let result = parse(args).and_then(|options| match command.as_str() {
        "ls" => ls(options),
        "cat" => cat(options),
        "get" => get(options),
        #[cfg(not(target_os = "redox"))]
        "pack" => pack(options),
        #[cfg(not(target_os = "redox"))]
        "serve" => serve(options),
        #[cfg(target_os = "redox")]
        "mount" => mount(options),
        _ => Err(USAGE.to_string()),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("webfs: {message}");
            ExitCode::FAILURE
        }
    }
}
