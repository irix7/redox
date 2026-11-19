#!/usr/bin/env bash
# Build tools/webfs for the Indy target: the scheme daemon in the initfs.
#
# Needs the static relibc sysroot (tools/relibc-indy.sh). The host build of
# the same crate (`webfs pack`, `webfs serve`) is a plain `cargo build`, so
# the target flags are passed here and not in a cargo config.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SYSROOT="${INDY_SYSROOT:-$ROOT/build/indy/sysroot}"
unset RUSTUP_TOOLCHAIN
. "$ROOT/tools/env.sh"
. "$ROOT/tools/indy-toolchain.sh"
export __CARGO_TESTS_ONLY_SRC_ROOT="${RUST_SRC:-/mnt/europa/redox-rust-src/library}"
# cargo does not track libc.a (a link argument): its hash in the flags makes
# a rebuilt libc relink the program.
LIBC_HASH="$(sha256sum "$SYSROOT/lib/libc.a" | cut -c1-16)"
export CARGO_TARGET_MIPS64_SGI_REDOX_RUSTFLAGS="--cfg indy_libc=\"$LIBC_HASH\" -C link-arg=-e -C link-arg=_start -C link-arg=$SYSROOT/lib/crt0.o -C link-arg=$SYSROOT/lib/crti.o -C link-arg=-L$SYSROOT/lib -C link-arg=--gc-sections -C link-arg=-z -C link-arg=max-page-size=4096"
cd "$ROOT/tools/webfs"
cargo build --release \
  --target "$ROOT/targets/mips64-sgi-redox.json" \
  -Zbuild-std=std,panic_abort -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec
