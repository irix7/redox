#!/usr/bin/env bash
# Build std programs from the base workspace for the Indy target.
#
#   tools/base-indy.sh PACKAGE...      e.g. tools/base-indy.sh randd netstack
#
# Needs the static relibc sysroot (tools/relibc-indy.sh). The link flags are
# passed in the environment: a cargo config in the base checkout would also
# apply to the bootstrap, which has its own entry and linker script.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SYSROOT="${INDY_SYSROOT:-$ROOT/build/indy/sysroot}"
unset RUSTUP_TOOLCHAIN
. "$ROOT/tools/env.sh"
. "$ROOT/tools/indy-toolchain.sh"
export __CARGO_TESTS_ONLY_SRC_ROOT="${RUST_SRC:-/mnt/europa/redox-rust-src/library}"
# cargo does not track libc.a (a link argument): put its hash in the flags so
# a rebuilt libc relinks every program.
LIBC_HASH="$(sha256sum "$SYSROOT/lib/libc.a" | cut -c1-16)"
export CARGO_TARGET_MIPS64_SGI_REDOX_RUSTFLAGS="--cfg indy_libc=\"$LIBC_HASH\" -C link-arg=-e -C link-arg=_start -C link-arg=$SYSROOT/lib/crt0.o -C link-arg=$SYSROOT/lib/crti.o -C link-arg=-L$SYSROOT/lib -C link-arg=--gc-sections -C link-arg=-z -C link-arg=max-page-size=4096"
# Size first: every program is static and resident in the initfs, and the PROM
# refuses a netboot image much over 7 MiB at 0x8804_0000. INDY_OPT_LEVEL=3
# builds for speed instead: opt-level z forbids inlining, which costs code
# like the TCP stack several times its speed (mk/indy.mk uses it for netstack).
export CARGO_PROFILE_RELEASE_OPT_LEVEL="${INDY_OPT_LEVEL:-z}" CARGO_PROFILE_RELEASE_PANIC=abort
export CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP="${INDY_STRIP:-symbols}" CARGO_PROFILE_RELEASE_DEBUG=0

packages=()
for p in "$@"; do packages+=(-p "$p"); done
cd "$ROOT/recipes/core/base/source"
cargo build --release "${packages[@]}" \
  --target "$ROOT/targets/mips64-sgi-redox.json" \
  -Zbuild-std=std,panic_abort -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec
