# webfs: a read-only filesystem over HTTP and HTTPS

`tools/webfs` is one Rust crate with two builds. On the build host it packs a directory
into a store and serves it. On the Indy it is the scheme daemon that mounts the store as
the root filesystem (scheme `file`). `make indy-gate` and `make indy-webfs-gate` pass on
iris. On the real Indy the root is mounted over plain HTTP from the build host. The TLS
client reaches Cloudflare from the real machine (see TLS), but no store is published
through the tunnel yet, so the root has not been mounted over HTTPS.

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

The store is the Indy's root filesystem. The last initfs unit, `50_rootfs.service`
(written by `tools/indy-image.sh`), runs `webfs mount --name file URL`, which registers
scheme `file`. init then switches root to `/usr` and `/etc` and starts every unit in
`/usr/lib/init.d`.

```sh
make indy            # kernel + initfs: init, randd, seeqd, netstack, webfs
make indy-root       # build/indy/root, packed into build/indy/store
tools/webfs/target/release/webfs serve build/indy/store --listen 0.0.0.0:8734
```

| Variable | Default | Meaning |
|---|---|---|
| `INDY_ROOT_URL` | `http://10.1.1.3:8734/` | where the machine finds the store |
| `INDY_ROOT_ARGS` | `--connect 10.1.1.3 --threads 2 --cache 256` | `webfs mount` options |
| `INDY_STORE` | `build/indy/store` | where `make indy-root` packs the tree |

The root tree is `config/indy/root` plus the programs: the base daemons (`dhcpd`,
`zerod`, `ipcd`, `ptyd`, `ramfs`, `getty`), `indy-sh`, `kdbg`, `webfs` and the userland
staged by `tools/indy-userland.sh` in `build/indy/userland`. `config/indy/root` holds
`usr/lib/init.d` (null and zero devices, PTYs, the RAM filesystem, dhcpd, the console
login, the maintenance shell on TCP port 23), `etc` (`passwd`, `group`, `shadow`,
`login_schemes.toml`, `hostname`) and links that leave the read-only store:
`dev/{null,zero,random,urandom,ptmx}`, `etc/net/dns`, and `tmp`, `root`, `home` and
`var`, which point into the RAM filesystem at `/scheme/tmp`. A changed program needs
`make indy-root` and a reboot; the netboot image only changes when the kernel or the
five initfs programs do.

A link whose target starts with `/scheme/` leaves the store. `open` answers `EXDEV` and
`fpath` on the link itself (opened with `O_SYMLINK`) returns the target, which is how
relibc follows `/dev/null`. The link may be in the middle of a path (`/tmp/x` with
`/tmp` -> `/scheme/tmp/tmp`): the same `O_SYMLINK` open then reports the target with the
rest of the path appended (`/scheme/tmp/tmp/x`).

Status 2026-11-18 on the real Indy (R4600, 32 MiB): the store mounts as the root, files
of 46 bytes to 2 MiB read back with the right CRC-32, and programs run from `/usr/bin`.
Reading runs at about 45 KiB/s; the kernel's cache flushing on every trap is the main
cost.

A second store can be mounted beside the root with `tools/webfs-unit.sh`, which adds
`50_webfs.service` for `/scheme/web`; the gate uses it for the TLS path.

Mount options:

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

2026-11-18 on the real Indy (R4600PC 133 MHz): `webfs get
https://www.cloudflare.com/cdn-cgi/trace` returns status 200. The first request takes
5.7 s (name lookup, connect and handshake); the second, on the open connection, 65 ms.

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

Not covered: `mmap` of files on the scheme.

The client reuses a kept connection only within 2 seconds of its last exchange. The
stalls seen on iris in November 2026 were first blamed on idle connections; the cause
was the kernel's timer, which stopped for about a minute after a late tick (fixed in the
kernel).
