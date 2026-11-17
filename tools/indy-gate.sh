#!/usr/bin/env bash
# Boot an ECOFF image on an emulated 32 MiB SGI Indy through the PROM's
# `boot -f bootp()redox` path and assert a serial marker.
#
# Usage:
#   tools/indy-gate.sh IMAGE [MARKER] [TIMEOUT_SECONDS]
#
# IMAGE is served as "redox" from a TFTP directory inside WORK (default
# build/indy/gate). The gate passes when MARKER (default REDOX-PROBE-OK)
# appears on the serial console within the timeout. Logs stay in WORK:
# serial.log (the console), iris.out (emulator stderr, with the TFTP trace).
#
# Environment:
#   IRIS, IRIS_CI   emulator binaries (default: /mnt/europa/iris/target/release)
#   WORK            working directory
#   MONITOR_PORT    iris monitor port (default 18893); give concurrent runs their own
#   SHELL_PORT      host port forwarded to the guest's TCP shell (default 12323)
#   IRIS_ARGS       extra emulator options, e.g. "--cache full --cpu r5000"
#   SHELL_CHECK     1 to also drive the TCP shell: run a command, then `reboot`
#                   and wait for the PROM to come back
#   SHELL_EXPECT_CMD, SHELL_EXPECT, SHELL_WAIT
#                   with SHELL_CHECK: a shell command, a string its output must
#                   contain, and the seconds to wait for it (default 30)
set -euo pipefail

IMAGE="${1:?usage: indy-gate.sh IMAGE [MARKER] [TIMEOUT_SECONDS]}"
MARKER="${2:-REDOX-PROBE-OK}"
TIMEOUT="${3:-120}"

IRIS="${IRIS:-/mnt/europa/iris/target/release/iris}"
IRIS_CI="${IRIS_CI:-/mnt/europa/iris/target/release/iris-ci}"
WORK="${WORK:-$(pwd)/build/indy/gate}"
MONITOR_PORT="${MONITOR_PORT:-18893}"
SHELL_PORT="${SHELL_PORT:-12323}"

for path in "$IMAGE" "$IRIS" "$IRIS_CI"; do
  if [ ! -e "$path" ]; then
    echo "indy-gate: missing $path" >&2
    exit 2
  fi
done

rm -rf "$WORK"
mkdir -p "$WORK/tftp"
cp "$IMAGE" "$WORK/tftp/redox"

cat >"$WORK/machine.toml" <<TOML
banks = [32, 0, 0, 0]
no_audio = true
headless = true
nvram = "$WORK/nvram.bin"

[machine]
profile = "indy_ip24"

[network]
mode = "nat"
tftp_dir = "$WORK/tftp"

[scsi]

[[port_forward]]
proto = "tcp"
host_port = $SHELL_PORT
guest_port = 23
bind = "localhost"
TOML

IRIS_NO_IDLE=1 IRIS_TFTP_TRACE=1 "$IRIS" --config "$WORK/machine.toml" --ci \
  --ci-socket "$WORK/iris.sock" --monitor-port "$MONITOR_PORT" \
  --serial-log "$WORK/serial.log" --headless --noaudio ${IRIS_ARGS:-} >"$WORK/iris.out" 2>&1 &
IRIS_PID=$!

cleanup() {
  local status=$?
  trap - EXIT
  kill "$IRIS_PID" 2>/dev/null || true
  wait "$IRIS_PID" 2>/dev/null || true
  if [ "$status" -ne 0 ]; then
    echo "--- iris output (tail)" >&2
    tail -20 "$WORK/iris.out" >&2 || true
    echo "--- serial log (tail)" >&2
    tail -40 "$WORK/serial.log" >&2 || true
  fi
  exit "$status"
}
trap cleanup EXIT

ic() { "$IRIS_CI" --socket "$WORK/iris.sock" -q "$@"; }

ic status --wait-ready 30
ic start
# Headless, the PROM puts its maintenance menu on the serial console.
ic serial-wait 'Option?' --timeout 60
ic serial-send '5'
ic serial-wait '>>' --timeout 30
ic serial-send 'boot -f bootp()redox'
ic serial-wait 'entry: 0x' --timeout 60
ic serial-wait "$MARKER" --timeout "$TIMEOUT"

if [ "${SHELL_CHECK:-0}" = 1 ]; then
  reply="$( (printf 'echo tcp-shell-ok\r\nuname\r\n'; sleep 4) | socat - "TCP:127.0.0.1:$SHELL_PORT" | tr -d '\r' )"
  echo "$reply" | sed 's/^/indy-gate: tcp> /'
  echo "$reply" | grep -q 'tcp-shell-ok'
  if [ -n "${SHELL_EXPECT_CMD:-}" ]; then
    reply="$( (printf '%s\r\n' "$SHELL_EXPECT_CMD"; sleep "${SHELL_WAIT:-30}") | socat - "TCP:127.0.0.1:$SHELL_PORT" | tr -d '\r' )"
    echo "$reply" | tail -5 | sed 's/^/indy-gate: tcp> /'
    echo "$reply" | grep -qF "$SHELL_EXPECT"
    echo "indy-gate: \"$SHELL_EXPECT_CMD\" printed \"$SHELL_EXPECT\""
  fi
  (printf 'reboot\r\n'; sleep 2) | socat - "TCP:127.0.0.1:$SHELL_PORT" >/dev/null || true
  ic serial-wait 'System Maintenance Menu' --timeout 120
  echo "indy-gate: reboot returned to the PROM"
fi

echo "indy-gate: PASS  marker \"$MARKER\" seen; logs in $WORK"
grep -E 'tftp .*(RRQ|complete)' "$WORK/iris.out" || true
grep -E 'entry: 0x' "$WORK/serial.log" | tr -d '\r' | sed 's/.*\(entry: 0x[0-9a-fA-F]*\).*/indy-gate: PROM reports \1/'
