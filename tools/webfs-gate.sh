#!/usr/bin/env bash
# Gate for tools/webfs: pack a tree, serve it over HTTPS on this host and read
# it from an emulated 32 MiB Indy through /scheme/web.
#
# The HTTPS server is Python's, with a certificate from a CA made for the
# run; it stands where Cloudflare's edge stands in production. The guest
# reaches it at the emulator's gateway address, which iris maps to this
# host's loopback. `webfs serve`, the origin server, is checked from the host.
#
# Needs `make indy` and tools/webfs-indy.sh to have run (`make indy-webfs-gate`
# does both).
#
# Environment: WORK (default build/indy/webfs-gate), TLS_PORT (8743),
# HTTP_PORT (8733), GATEWAY (192.168.0.1), plus what tools/indy-gate.sh reads.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${WORK:-$ROOT/build/indy/webfs-gate}"
TLS_PORT="${TLS_PORT:-8743}"
HTTP_PORT="${HTTP_PORT:-8733}"
GATEWAY="${GATEWAY:-192.168.0.1}"
WEBFS="$ROOT/tools/webfs/target/release/webfs"

rm -rf "$WORK"
mkdir -p "$WORK/tree/etc" "$WORK/tree/data"
echo "hello from webfs" >"$WORK/tree/etc/motd"
ln -s etc/motd "$WORK/tree/motd"
# 200 KiB: four pieces and a piece list.
seq -f 'line %06.0f of the large file' 1 6400 >"$WORK/tree/data/large"
echo "large file ends here" >>"$WORK/tree/data/large"
"$WEBFS" pack "$WORK/tree" "$WORK/store"

python3 - "$WORK" <<'PY'
import datetime, sys
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID
work = sys.argv[1]
now = datetime.datetime.now(datetime.timezone.utc)
def name(cn): return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, cn)])
def build(subject, issuer, key, signer, ca, san=None):
    b = (x509.CertificateBuilder().subject_name(name(subject)).issuer_name(name(issuer))
         .public_key(key.public_key()).serial_number(x509.random_serial_number())
         .not_valid_before(now - datetime.timedelta(days=1))
         .not_valid_after(now + datetime.timedelta(days=30))
         .add_extension(x509.BasicConstraints(ca=ca, path_length=None), critical=True))
    if san: b = b.add_extension(x509.SubjectAlternativeName([x509.DNSName(san)]), critical=False)
    return b.sign(signer, hashes.SHA256())
ca_key = ec.generate_private_key(ec.SECP256R1())
key = ec.generate_private_key(ec.SECP256R1())
ca = build("webfs gate CA", "webfs gate CA", ca_key, ca_key, True)
leaf = build("webfs.test", "webfs gate CA", key, ca_key, False, "webfs.test")
pem = serialization.Encoding.PEM
open(f"{work}/ca.pem", "wb").write(ca.public_bytes(pem))
open(f"{work}/leaf.pem", "wb").write(leaf.public_bytes(pem) + key.private_bytes(
    pem, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
PY

cat >"$WORK/https.py" <<'PY'
import http.server, ssl, sys
work, port = sys.argv[1], int(sys.argv[2])
class Handler(http.server.SimpleHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def __init__(self, *a, **k): super().__init__(*a, directory=f"{work}/store", **k)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(f"{work}/leaf.pem")
server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
PY
python3 "$WORK/https.py" "$WORK" "$TLS_PORT" >"$WORK/https.log" 2>&1 &
HTTPS_PID=$!
"$WEBFS" serve "$WORK/store" --listen "127.0.0.1:$HTTP_PORT" --quiet 2>"$WORK/serve.log" &
SERVE_PID=$!
trap 'kill $HTTPS_PID $SERVE_PID 2>/dev/null || true' EXIT
sleep 1

# Host side: the origin server, then the TLS path with the run's CA.
"$WEBFS" cat "http://127.0.0.1:$HTTP_PORT" data/large | cmp - "$WORK/tree/data/large"
"$WEBFS" cat --ca "$WORK/ca.pem" --connect 127.0.0.1 "https://webfs.test:$TLS_PORT" motd | grep -q 'hello from webfs'
if "$WEBFS" cat --connect 127.0.0.1 "https://webfs.test:$TLS_PORT" motd 2>/dev/null; then
  echo "webfs-gate: a certificate from an unknown CA was accepted" >&2
  exit 1
fi
echo "webfs-gate: host checks pass"

# The gate image: the current initfs plus webfs, built beside the normal one
# so build/indy/redox stays as `make indy` left it.
cd "$ROOT"
B=build/indy
cp -r "$B/initfs" "$WORK/initfs"
WEBFS_CA="$WORK/ca.pem" tools/webfs-unit.sh "$WORK/initfs" tools/webfs/target/mips64-sgi-redox/release/webfs \
  "https://webfs.test:$TLS_PORT" --connect "$GATEWAY"
recipes/core/base/source/target/release/redox-initfs-ar "$WORK/initfs" \
  recipes/core/base/source/bootstrap/target/mips64-sgi-redox/release/bootstrap -o "$WORK/initfs.img"
tools/ecoff/target/release/ecoff convert "$B/kernel/kernel" "$WORK/redox" --append "$WORK/initfs.img"
ls -l "$B/redox" "$WORK/redox" "$WORK/initfs/bin/webfs"

WORK="$WORK/gate" SHELL_CHECK=1 SHELL_WAIT="${SHELL_WAIT:-60}" \
  SHELL_EXPECT_CMD='cat /scheme/web/motd /scheme/web/data/large' SHELL_EXPECT='large file ends here' \
  tools/indy-gate.sh "$WORK/redox" "${INDY_MARKER:-addr 192.168}" "${TIMEOUT:-180}"
grep -c 'GET /blob/' "$WORK/https.log" | sed 's/^/webfs-gate: blob requests served over TLS: /'
echo "webfs-gate: PASS"
