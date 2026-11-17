#!/usr/bin/env bash
# GDB on the running Indy kernel, through tools/kdbg on the machine.
#
#   tools/indy-gdb.sh [GDB_ARGUMENT...]
#
# Loads the kernel's symbols (build/indy/kernel/kernel.all, the unstripped
# image of the last `make indy`) and connects to INDY_HOST (default
# 10.1.1.112) port 2159. The symbols must be those of the kernel that is
# running. Inspection only: see tools/kdbg/src/main.rs.
#
#   tools/indy-gdb.sh -batch -ex 'info threads' -ex 'thread apply all bt 8'
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GDB="${GDB:-/mnt/europa/toolchains/mips64-n32--uclibc--stable-2026.08-1/bin/mips64-linux-gdb}"
HOST="${INDY_HOST:-10.1.1.112}"
# -readnow: without it this GDB finds no line for most kernel addresses.
exec "$GDB" -q -readnow "$ROOT/build/indy/kernel/kernel.all" \
  -ex 'set confirm off' -ex 'set pagination off' \
  -ex "target remote $HOST:2159" "$@"
