//! Bring-up shell for the Indy.
//!
//! One command interpreter serves the serial console (stdin/stdout, the
//! kernel's debug scheme) and every TCP connection to port 23, so the machine
//! can be driven without a serial cable. A reporter prints the interface
//! address on the console when it changes.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const TCP_PORT: u16 = 23;
const BIN_DIR: &str = "/usr/bin";

fn read_trim(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn net_line() -> String {
    let mac = read_trim("/scheme/netcfg/ifaces/eth0/mac").unwrap_or_default();
    let addr = read_trim("/scheme/netcfg/ifaces/eth0/addr/list").unwrap_or_default();
    format!("mac {mac} addr {}", addr.replace('\n', " "))
}

fn help(out: &mut dyn Write) {
    let _ = writeln!(
        out,
        "commands:\r\n  help               this list\r\n  echo TEXT          print TEXT\r\n  ls [DIR]           list a directory (default {BIN_DIR})\r\n  cat FILE           print a file\r\n  ps                 contexts (/scheme/sys/context)\r\n  mem                memory (/scheme/sys/memory... if present)\r\n  net                interface address\r\n  uname              kernel identification\r\n  dmesg              kernel log\r\n  sum FILE           size and CRC-32 of a file\r\n  peek ADDR [LEN]    hex dump of physical memory\r\n  sleep SECONDS      wait; checks the timer against a wall clock\r\n  bench              time a compute loop, syscalls and page faults\r\n  polltest           pipe, fcntl and poll, one call at a time\r\n  sigtest            deliver SIGUSR1 to a handler 1000 times around a checksum\r\n  reboot             reset to the PROM\r\n  PROGRAM [ARGS]     run {BIN_DIR}/PROGRAM\r"
    );
}

fn cat(out: &mut dyn Write, path: &str) {
    match fs::read(path) {
        Ok(data) => {
            let text = String::from_utf8_lossy(&data).replace('\n', "\r\n");
            let _ = write!(out, "{text}");
            if !text.ends_with("\r\n") {
                let _ = write!(out, "\r\n");
            }
        }
        Err(err) => {
            let _ = writeln!(out, "cat: {path}: {err}\r");
        }
    }
}

/// Run one command line. Returns false when the session should end.
fn run(line: &str, out: &mut dyn Write) -> bool {
    let mut words = line.split_whitespace();
    let Some(cmd) = words.next() else { return true };
    let args: Vec<&str> = words.collect();
    match cmd {
        "help" | "?" => help(out),
        "echo" => {
            let _ = writeln!(out, "{}\r", args.join(" "));
        }
        "ls" => {
            let dir = args.first().copied().unwrap_or(BIN_DIR);
            match fs::read_dir(dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let _ = writeln!(out, "{}\r", entry.file_name().to_string_lossy());
                    }
                }
                Err(err) => {
                    let _ = writeln!(out, "ls: {dir}: {err}\r");
                }
            }
        }
        "cat" => {
            for path in &args {
                cat(out, path);
            }
        }
        "ps" => cat(out, "/scheme/sys/context"),
        "mem" => cat(out, "/scheme/sys/stat"),
        "uname" => cat(out, "/scheme/sys/uname"),
        "dmesg" => cat(out, "/scheme/sys/log"),
        "net" => {
            let _ = writeln!(out, "{}\r", net_line());
        }
        "peek" => {
            let addr = args.first().copied().and_then(parse_number);
            let len = args.get(1).copied().and_then(parse_number).unwrap_or(256);
            match addr {
                Some(addr) => peek(out, addr, len),
                None => {
                    let _ = writeln!(out, "usage: peek PHYS_ADDR [LEN]\r");
                }
            }
        }
        "sum" => match args.first() {
            Some(path) => sum(out, path),
            None => {
                let _ = writeln!(out, "usage: sum FILE\r");
            }
        },
        "sleep" => {
            let seconds = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
            thread::sleep(Duration::from_secs(seconds));
            let _ = writeln!(out, "slept {seconds} s\r");
        }
        "bench" => bench(out),
        "polltest" => polltest(out),
        "profile" => {
            let _ = fs::write("/scheme/sys/kstop", "profile");
            let log = fs::read("/scheme/sys/log").unwrap_or_default();
            let text = String::from_utf8_lossy(&log);
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.iter().rposition(|l| l.contains("profile: ") && l.contains("samples")).unwrap_or(0);
            for line in &lines[start..] {
                if line.contains("Indy: profile") {
                    let _ = writeln!(out, "{line}\r");
                }
            }
        }
        "ipcbench" => {
            use std::os::unix::fs::FileExt;
            if let Ok(file) = fs::File::open("/scheme/initfs/lib/init.d/00_randd.service") {
                let mut byte = [0u8; 1];
                let start = std::time::Instant::now();
                for _ in 0..4000 {
                    let _ = file.read_at(&mut byte, 0);
                }
                let _ = writeln!(out, "4000 reads through the initfs server: {} ms\r", start.elapsed().as_millis());
            }
        }
        "cachestat" => {
            let _ = fs::write("/scheme/sys/kstop", "cachestat");
            let log = fs::read("/scheme/sys/log").unwrap_or_default();
            let text = String::from_utf8_lossy(&log);
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.iter().rposition(|l| l.contains("Indy: cache:")).unwrap_or(0);
            for line in &lines[start.saturating_sub(10)..=start.min(lines.len().saturating_sub(1))] {
                if line.contains("Indy: ") {
                    let _ = writeln!(out, "{line}\r");
                }
            }
        }
        "sigtest" => sigtest(out),
        "reboot" => {
            let _ = writeln!(out, "rebooting to the PROM\r");
            let _ = out.flush();
            thread::sleep(Duration::from_millis(200));
            if let Err(err) = fs::write("/scheme/sys/kstop", "reset") {
                let _ = writeln!(out, "reboot: {err}\r");
            }
        }
        "exit" | "quit" => return false,
        program => {
            let path = if program.contains('/') {
                program.to_string()
            } else {
                format!("{BIN_DIR}/{program}")
            };
            // One pipe, read to its end, then wait. `Command::output` reads two
            // pipes with poll(), which fails here with EOPNOTSUPP and panics
            // inside std. The program's stderr goes to the console.
            let child = Command::new(&path)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn();
            match child {
                Ok(mut child) => {
                    let mut text = Vec::new();
                    if let Some(mut stdout) = child.stdout.take() {
                        let _ = stdout.read_to_end(&mut text);
                    }
                    let text = String::from_utf8_lossy(&text).replace('\n', "\r\n");
                    let _ = write!(out, "{text}");
                    match child.wait() {
                        Ok(status) if status.success() => {}
                        Ok(status) => {
                            let _ = writeln!(out, "{program}: {status}\r");
                        }
                        Err(err) => {
                            let _ = writeln!(out, "{program}: wait: {err}\r");
                        }
                    }
                }
                Err(err) => {
                    let _ = writeln!(out, "{program}: {err}\r");
                }
            }
        }
    }
    true
}

/// Read lines from `input`, run them, write results to `output`.
fn session(input: &mut dyn Read, output: &mut dyn Write, echo: bool) {
    let _ = write!(output, "\r\nRedox on the SGI Indy. Type help.\r\nindy# ");
    let _ = output.flush();
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut last = 0u8;
    loop {
        match input.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let previous = last;
        last = byte[0];
        match byte[0] {
            // The second half of a CR LF pair.
            b'\n' if previous == b'\r' => {}
            b'\r' | b'\n' => {
                if echo {
                    let _ = write!(output, "\r\n");
                }
                let text = String::from_utf8_lossy(&line).to_string();
                line.clear();
                if !run(&text, output) {
                    return;
                }
                let _ = write!(output, "indy# ");
            }
            0x08 | 0x7f => {
                if line.pop().is_some() && echo {
                    let _ = write!(output, "\x08 \x08");
                }
            }
            c if c >= 0x20 => {
                line.push(c);
                if echo {
                    let _ = output.write_all(&[c]);
                }
            }
            _ => {}
        }
        let _ = output.flush();
    }
}

fn serve(stream: TcpStream) {
    let Ok(mut reader) = stream.try_clone() else { return };
    let mut writer = stream;
    session(&mut reader, &mut writer, false);
}

fn tcp_server() {
    // The network stack may not be up yet: keep trying to bind.
    let listener = loop {
        match TcpListener::bind(("0.0.0.0", TCP_PORT)) {
            Ok(listener) => break listener,
            Err(_) => thread::sleep(Duration::from_secs(2)),
        }
    };
    println!("REDOX-INDY-SH: listening on tcp port {TCP_PORT}\r");
    // The kernel reboots a test image that never gets this far.
    let _ = fs::write("/scheme/sys/kstop", "disarm");
    for stream in listener.incoming().flatten() {
        thread::spawn(move || serve(stream));
    }
}

fn reporter() {
    let mut reported = String::new();
    loop {
        thread::sleep(Duration::from_secs(2));
        let line = net_line();
        if line != reported {
            println!("REDOX-INDY-NET: {line}\r");
            reported = line;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `indy-sh mkdir DIR...`: make the directories of the RAM filesystem at
    // boot, before a login needs a home directory.
    if args.first().map(String::as_str) == Some("mkdir") {
        for dir in &args[1..] {
            if let Err(err) = fs::create_dir_all(dir) {
                println!("indy-sh: mkdir {dir}: {err}\r");
            }
        }
        return;
    }
    println!("REDOX-INDY-SH: hello from std on the Indy\r");
    thread::spawn(reporter);
    // `indy-sh --tcp`: serve port 23 only. The console belongs to getty.
    if args.first().map(String::as_str) == Some("--tcp") {
        tcp_server();
        return;
    }
    thread::spawn(tcp_server);
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    loop {
        session(&mut stdin, &mut stdout, true);
        // Console input ended or failed: wait and offer the prompt again.
        thread::sleep(Duration::from_secs(5));
    }
}

fn parse_number(text: &str) -> Option<usize> {
    match text.strip_prefix("0x") {
        Some(hex) => usize::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

/// Hex dump of physical memory, 32 bytes to a line, each line prefixed with
/// its address. Maps the range through `/scheme/memory/physical@dev`
/// (uncached), the way the drivers reach device registers.
fn peek(out: &mut dyn Write, addr: usize, len: usize) {
    use std::os::fd::AsRawFd;
    const PROT_READ: i32 = 0x0004;
    const MAP_SHARED: i32 = 0x0001;
    let file = match fs::File::open("/scheme/memory/physical@dev") {
        Ok(file) => file,
        Err(err) => {
            let _ = writeln!(out, "peek: {err}\r");
            return;
        }
    };
    let base = addr & !0xfff;
    let span = (addr + len - base + 0xfff) & !0xfff;
    let map = unsafe {
        mmap(core::ptr::null_mut(), span, PROT_READ, MAP_SHARED, file.as_raw_fd(), base as i64)
    };
    if map as usize == usize::MAX {
        let _ = writeln!(out, "peek: cannot map {base:#x}+{span:#x}\r");
        return;
    }
    let mut line = String::with_capacity(80);
    for offset in (0..len).step_by(32) {
        line.clear();
        line.push_str(&format!("{:08x}:", addr + offset));
        for i in offset..(offset + 32).min(len) {
            let byte = unsafe { map.add(addr - base + i).read_volatile() };
            line.push_str(&format!("{byte:02x}"));
        }
        if writeln!(out, "{line}\r").is_err() {
            break;
        }
    }
    let _ = out.flush();
    unsafe { munmap(map, span) };
}

/// Three timings in milliseconds, to compare kernels on the real machine:
/// a compute loop that stays in user mode, 2000 small file reads (each is
/// several scheme round trips), and first touches of 256 fresh pages.
fn bench(out: &mut dyn Write) {
    use std::time::Instant;

    let start = Instant::now();
    let mut x: u32 = 1;
    for i in 0..20_000_000u32 {
        x = x.wrapping_mul(1_664_525).wrapping_add(i);
    }
    let compute = start.elapsed().as_millis();

    let start = Instant::now();
    let mut bytes = 0;
    for _ in 0..2000 {
        bytes += fs::read("/scheme/sys/uname").map(|d| d.len()).unwrap_or(0);
    }
    let syscalls = start.elapsed().as_millis();

    let start = Instant::now();
    let mut pages = vec![0u8; 0];
    pages.reserve_exact(256 * 4096);
    unsafe { pages.set_len(256 * 4096) };
    let mut sum = 0u32;
    for page in 0..256 {
        pages[page * 4096] = page as u8;
        sum += pages[page * 4096] as u32;
    }
    let faults = start.elapsed().as_millis();

    let start = Instant::now();
    let mut flags = 0;
    for _ in 0..20_000 {
        flags ^= unsafe { fcntl(0, 3, 0) };
    }
    let fcntls = start.elapsed().as_millis();
    let _ = writeln!(out, "bench: 20000 fcntl calls {fcntls} ms ({flags:#x})\r");

    // One read of an open file: a kernel scheme (no context switch), a
    // scheme served by the bootstrap process, and the network filesystem.
    for path in ["/scheme/sys/uname", "/scheme/initfs/lib/init.d/00_randd.service", "/usr/lib/init.d/40_sh.service"] {
        use std::os::unix::fs::FileExt;
        let Ok(file) = fs::File::open(path) else { continue };
        let mut byte = [0u8; 1];
        let start = Instant::now();
        let mut got = 0;
        for _ in 0..2000 {
            got += file.read_at(&mut byte, 0).unwrap_or(0);
        }
        let _ = writeln!(out, "bench: 2000 one-byte reads of {path}: {} ms ({got})\r", start.elapsed().as_millis());
    }

    let _ = writeln!(
        out,
        "bench: compute {compute} ms ({x:#x}), 2000 reads {syscalls} ms ({bytes} bytes), 256 page touches {faults} ms ({sum})\r"
    );
}

/// Size, CRC-32 (the zlib polynomial) and read time of a file, read in
/// 16 KiB pieces. Compare with `python3 -c "import zlib,sys;..."` on the host.
fn sum(out: &mut dyn Write, path: &str) {
    let start = std::time::Instant::now();
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            let _ = writeln!(out, "sum: {path}: {err}\r");
            return;
        }
    };
    let mut crc: u32 = !0;
    let mut size = 0usize;
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                size += n;
                for &byte in &buf[..n] {
                    crc ^= byte as u32;
                    for _ in 0..8 {
                        crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
                    }
                }
            }
            Err(err) => {
                let _ = writeln!(out, "sum: {path}: {err} after {size} bytes\r");
                return;
            }
        }
    }
    let _ = writeln!(
        out,
        "{size} bytes, crc32 {:08x}, {} ms\r",
        !crc,
        start.elapsed().as_millis()
    );
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, arg: usize) -> i32;
    fn poll(fds: *mut PollFd, nfds: usize, timeout: i32) -> i32;
    fn write(fd: i32, buf: *const u8, len: usize) -> isize;
}

/// The calls std makes to read a child's two pipes, one at a time, with the
/// error of each. Finds which one this port gets wrong.
fn polltest(out: &mut dyn Write) {
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: usize = 0x0004_0000;
    let err = || std::io::Error::last_os_error();
    let mut fds = [0i32; 2];
    unsafe {
        let r = pipe(fds.as_mut_ptr());
        let _ = writeln!(out, "pipe -> {r} ({:?}) {}\r", fds, if r < 0 { err().to_string() } else { String::new() });
        let flags = fcntl(fds[0], F_GETFL, 0);
        let _ = writeln!(out, "F_GETFL -> {flags:#x} {}\r", if flags < 0 { err().to_string() } else { String::new() });
        let r = fcntl(fds[0], F_SETFL, flags as usize | O_NONBLOCK);
        let _ = writeln!(out, "F_SETFL -> {r} {}\r", if r < 0 { err().to_string() } else { String::new() });
        let mut set = [PollFd { fd: fds[0], events: 1, revents: 0 }];
        let r = poll(set.as_mut_ptr(), 1, 100);
        let _ = writeln!(out, "poll empty -> {r} revents {:#x} {}\r", set[0].revents, if r < 0 { err().to_string() } else { String::new() });
        let _ = write(fds[1], b"x".as_ptr(), 1);
        let r = poll(set.as_mut_ptr(), 1, 100);
        let _ = writeln!(out, "poll ready -> {r} revents {:#x} {}\r", set[0].revents, if r < 0 { err().to_string() } else { String::new() });
    }
}

static SIGNAL_HITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

extern "C" fn on_signal(_signal: i32) {
    SIGNAL_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

unsafe extern "C" {
    fn signal(signal: i32, handler: usize) -> usize;
    fn raise(signal: i32) -> i32;
}

/// Signal delivery through the trampoline: 1000 signals to a handler, each
/// in the middle of a running checksum. The checksum must match the same
/// loop without signals, or the trampoline lost a register.
fn sigtest(out: &mut dyn Write) {
    use std::sync::atomic::Ordering;
    const SIGUSR1: i32 = 10;
    fn step(x: u64, i: u64) -> u64 {
        x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(i).rotate_left(17)
    }
    let mut plain: u64 = 1;
    for i in 0..1000u64 {
        for j in 0..100 {
            plain = step(plain, i ^ j);
        }
    }
    SIGNAL_HITS.store(0, Ordering::Relaxed);
    let previous = unsafe { signal(SIGUSR1, on_signal as usize) };
    let mut with: u64 = 1;
    let mut failed = 0;
    for i in 0..1000u64 {
        for j in 0..50 {
            with = step(with, i ^ j);
        }
        if unsafe { raise(SIGUSR1) } != 0 {
            failed += 1;
        }
        for j in 50..100 {
            with = step(with, i ^ j);
        }
    }
    unsafe { signal(SIGUSR1, previous) };
    let hits = SIGNAL_HITS.load(Ordering::Relaxed);
    let verdict = if hits == 1000 && failed == 0 && with == plain { "ok" } else { "FAILED" };
    let _ = writeln!(
        out,
        "sigtest {verdict}: {hits} handler calls, {failed} raise errors, checksum {with:#x} (expect {plain:#x})\r"
    );
}
