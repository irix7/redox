#!/usr/bin/env bash
# Assemble the Indy netboot image: the kernel with the initfs appended.
#
#   tools/indy-image.sh OUT INITFS_DIR ROOT_URL [MOUNT_OPTION...]
#
# The initfs holds what must run before there is a filesystem: init, randd,
# the Seeq driver, the network stack and webfs. Its last unit mounts the
# webfs store at ROOT_URL as scheme `file`, the root filesystem. Everything
# else (the shell, dhcpd, the null and zero devices) lives in that store;
# `make indy-root` builds it.
#
# MOUNT_OPTIONs go to `webfs mount`, e.g. --connect 10.1.1.3 for a URL that
# names an address.
set -euo pipefail
USAGE="usage: indy-image.sh OUT INITFS_DIR ROOT_URL [MOUNT_OPTION...]"
OUT="${1:?$USAGE}"
DIR="${2:?$USAGE}"
URL="${3:?$USAGE}"
shift 3
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BASE=recipes/core/base/source
BASE_OUT=$BASE/target/mips64-sgi-redox/release

rm -rf "$DIR"
mkdir -p "$DIR/bin" "$DIR/lib/init.d" "$DIR/etc/net"
for bin in init randd seeqd netstack; do install -m 0755 "$BASE_OUT/$bin" "$DIR/bin/$bin"; done
install -m 0755 tools/webfs/target/mips64-sgi-redox/release/webfs "$DIR/bin/webfs"
install -m 0644 config/indy/init.d/* "$DIR/lib/init.d/"
install -m 0644 config/indy/net/* "$DIR/etc/net/"

args='"mount", "--name", "file"'
for word in "$@" "$URL"; do args="$args, \"$word\""; done
cat >"$DIR/lib/init.d/50_rootfs.service" <<UNIT
[unit]
description = "Root filesystem: the webfs store at $URL"
requires_weak = ["20_netstack.service"]

[service]
cmd = "webfs"
args = [$args]
type = { scheme = "file" }
UNIT

$BASE/target/release/redox-initfs-ar "$DIR" \
  $BASE/bootstrap/target/mips64-sgi-redox/release/bootstrap -o "$DIR.img"
tools/ecoff/target/release/ecoff convert build/indy/kernel/kernel "$OUT" --append "$DIR.img"
ls -l "$DIR.img" "$DIR/bin"
