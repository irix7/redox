# webfs: a read-only filesystem over HTTP and HTTPS

`tools/webfs` is one Rust crate with two builds. On the build host it packs a directory
into a store and serves it. On the Indy it is the scheme daemon that mounts the store at
`/scheme/web`. Status 2026-11-17: `make indy-webfs-gate` passes on iris (32 MiB
`indy_ip24`, TLS to a server on the host). It has not run on the real machine or against
Cloudflare from the guest.

## Store format (version 1)

A store is a directory of static files under one base URL.

| Path | Content |
|---|---|
| `root` | `webfs1 <hex SHA-256 of the root directory blob> <unix time>` |
| `blob/ab/abcd…` | one blob, named by the SHA-256 of its content |

A directory blob is text, one line per entry, sorted by name:
`<d|f|l> <mode, octal> <size> <hex hash> <name>`. A file of at most 64 KiB is one blob. A
larger file is cut into 64 KiB pieces and its hash names a list blob holding the 32-byte
piece hashes. A symbolic link's blob holds its target. Modes are normalised to 644, 755
(executables and directories) and 777 (links); owner is 0. Every file reports the pack
time as its modification time.

Blobs never change, so a cache can keep them for a year. Only `root` changes when a tree
is published again. The client checks each blob against its name, so a wrong or stale
cache entry is an I/O error on the client.

## Build and host

```sh
. tools/env.sh
make webfs                                   # tools/webfs/target/release/webfs
webfs pack /path/to/tree /srv/indy-store     # incremental: writes only new blobs
webfs serve /srv/indy-store                  # http://127.0.0.1:8733
```

`webfs serve` answers `GET` and `HEAD` for `/root` and `/blob/ab/<64 hex digits>` and 404
for every other path. It sends `Cache-Control: public, max-age=60` for `root`
(`--root-max-age SECONDS`) and `public, max-age=31536000, immutable` for blobs.
`webfs ls URL [PATH]` and `webfs cat URL PATH` read a store from the host, and
`webfs get URL` times a GET of any URL.

`pack` does not delete blobs that no tree refers to any more. To prune, pack into a new
directory and switch `serve` to it.

### Cloudflare Tunnel

Add an ingress rule that sends a hostname to the origin server, for example in the
tunnel's `config.yml`:

```yaml
ingress:
  - hostname: indy.example.com
    service: http://127.0.0.1:8733
  - service: http_status:404
```

Cloudflare caches by file extension by default and blob names have none. Add a Cache Rule
for the hostname: "Eligible for cache", edge TTL "Use cache-control header if present".
Check with `curl -sI https://indy.example.com/root`: the second request shows
`cf-cache-status: HIT`. A published tree reaches clients when the cached `root` expires,
60 seconds by default.

## The client on the Indy

```sh
make indy INDY_WEBFS_URL=https://indy.example.com
```

With `INDY_WEBFS_URL` set, the initfs gains `bin/webfs` and
`lib/init.d/50_webfs.service` (`tools/webfs-unit.sh`). Without it the image is unchanged.
`INDY_WEBFS_ARGS` passes mount options and `INDY_WEBFS_CA` adds a certificate file.

| Option | Default | Meaning |
|---|---|---|
| `--threads N` | 4 | worker threads; each uses at most one connection |
| `--cache KIB` | 1024 | blob cache cap; least recently used blobs go first |
| `--root HASH` | none | accept only this root hash from the server |
| `--connect IP` | DNS | connect to IP; the URL still names the host |
| `--ca FILE` | none | trust the certificates in FILE too (DER or PEM) |

The daemon registers the scheme at once and fetches `root` on the first open. For the
first 60 seconds after start it retries a failed mount every 2 seconds, which covers the
wait for DHCP. It keeps the root it fetched until it exits, so one boot sees one
snapshot. Symbolic links are followed inside the tree. Writes return `EROFS`.

Host names are looked up with an A query to the server in
`/scheme/netcfg/resolv/nameserver`, which `dhcpd` sets. relibc's resolver reads
`/etc/net/dns`, which the initfs does not have.

### TLS

The client is `embedded-tls` 0.19: TLS 1.3, `TLS_AES_128_GCM_SHA256`, P-256 key exchange.
`src/trust.rs` checks the chain: ECDSA P-256 and P-384, RSA PKCS#1 v1.5 in certificates
and RSA-PSS in the handshake, host name against `subjectAltName`, CA flag on
intermediates. Four roots are compiled in: ISRG Root X1 and X2, GTS Root R1 and R4
(`tools/webfs/roots/`, taken from the host CA bundle on 2026-11-17).

Limits:

- The Indy port has no clock source yet. When the clock reads before 2026-01-01 the
  validity dates of certificates are not checked.
- `randd` has no entropy source yet (bootstrap.md T9), so the TLS key exchange is
  predictable to an attacker who knows the boot state. Blob hashes and the server's
  signature still protect integrity.
- Revocation is not checked.
- Servers limited to TLS 1.2 fail the handshake.

## Memory

| Item | Bytes |
|---|---:|
| `webfs` for the target, stripped (`opt-level=z`, LTO) | 1,067,900 |
| Blob cache, default cap | 1,048,576 |
| TLS record buffers, per open connection | 20,736 |
| Worker stack reservation, per thread | 524,288 |

The binary is resident in the initfs, which is over its 2 MiB budget before webfs is
added. A blob larger than the cache cap is held alone until the next insert. The largest
blob the client accepts is 4 MiB (a directory or a piece list).

## Gate

`make indy-webfs-gate` (`tools/webfs-gate.sh`) packs a test tree with a 200 KiB file,
serves it with `webfs serve` and with a Python HTTPS server under a CA made for the run,
checks both from the host, checks that the unknown CA is refused without `--ca`, then
boots an image with webfs on iris and reads `/scheme/web/motd` (a link) and the large
file over the TCP shell. The gate image is built in `build/indy/webfs-gate`;
`build/indy/redox` is left alone.

Not covered: `mmap` of files on the scheme, running a program from `/scheme/web`, and the
scheme registered as `file` to serve as the root filesystem.
