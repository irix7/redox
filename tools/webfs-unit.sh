#!/usr/bin/env bash
# Add webfs to an initfs directory: the daemon, its unit and, with WEBFS_CA
# set, a certificate file to trust besides the built-in roots.
#
#   tools/webfs-unit.sh INITFS_DIR WEBFS_ELF URL [MOUNT_OPTION...]
#
# Writes lib/init.d/50_webfs.service, which mounts URL as scheme `web`, and
# adds it to the units 90_initfs.target starts.
set -euo pipefail
USAGE="usage: webfs-unit.sh INITFS_DIR WEBFS_ELF URL [MOUNT_OPTION...]"
DIR="${1:?$USAGE}"
ELF="${2:?$USAGE}"
URL="${3:?$USAGE}"
shift 3
install -m 0755 "$ELF" "$DIR/bin/webfs"
if [ -n "${WEBFS_CA:-}" ]; then
  install -D -m 0644 "$WEBFS_CA" "$DIR/etc/webfs/ca.pem"
  set -- --ca /scheme/initfs/etc/webfs/ca.pem "$@"
fi
args='"mount"'
for word in "$@" "$URL"; do args="$args, \"$word\""; done
cat >"$DIR/lib/init.d/50_webfs.service" <<UNIT
[unit]
description = "Read-only filesystem over HTTP: /scheme/web"
requires_weak = ["20_netstack.service"]

[service]
cmd = "webfs"
args = [$args]
type = { scheme = "web" }
UNIT
sed -i 's/^\( *\)"50_rootfs.service",$/&\n\1"50_webfs.service",/' "$DIR/lib/init.d/90_initfs.target"
grep -q '"50_webfs.service"' "$DIR/lib/init.d/90_initfs.target"
