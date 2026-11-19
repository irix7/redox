#!/usr/bin/env bash
# Build a cargo project outside the base workspace for the Indy target.
#
#   tools/userland-indy.sh DIR [CARGO ARGS...]
#   e.g. tools/userland-indy.sh recipes/core/ion/source --bin ion
#
# Needs the static relibc sysroot (tools/relibc-indy.sh) and vendor/
# (tools/vendor.sh). The libc and redox_syscall patches and the release profile
# are passed on the command line, so the upstream checkout needs no cargo
# config of its own. Programs land in DIR/target/mips64-sgi-redox/release.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SYSROOT="${INDY_SYSROOT:-$ROOT/build/indy/sysroot}"
DIR="${1:?usage: userland-indy.sh DIR [CARGO ARGS...]}"
shift
unset RUSTUP_TOOLCHAIN
. "$ROOT/tools/env.sh"
. "$ROOT/tools/indy-toolchain.sh"
export __CARGO_TESTS_ONLY_SRC_ROOT="${RUST_SRC:-/mnt/europa/redox-rust-src/library}"
# cargo does not track libc.a (a link argument): its hash in the flags makes
# a rebuilt libc relink every program.
LIBC_HASH="$(sha256sum "$SYSROOT/lib/libc.a" | cut -c1-16)"
export CARGO_TARGET_MIPS64_SGI_REDOX_RUSTFLAGS="--cfg indy_libc=\"$LIBC_HASH\" -C link-arg=-e -C link-arg=_start -C link-arg=$SYSROOT/lib/crt0.o -C link-arg=$SYSROOT/lib/crti.o -C link-arg=-L$SYSROOT/lib -C link-arg=--gc-sections -C link-arg=-z -C link-arg=max-page-size=4096 ${INDY_RUSTFLAGS:-}"
# Size first: a program is read whole over the network before it starts.
export CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_PANIC=abort
export CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=symbols CARGO_PROFILE_RELEASE_DEBUG=0
# C sources in build scripts (cc crate) compile with the Bootlin n32 GCC. The
# sysroot has no relibc headers, so the code sees the toolchain's uClibc
# headers: prototypes match, but a crate that depends on a libc struct layout
# or on errno must not be built this way.
SDK="${INDY_MIPS_SDK:-/mnt/europa/toolchains/mips64-n32--uclibc--stable-2026.08-1}"
export CC_mips64_sgi_redox="$SDK/bin/mips64-buildroot-linux-uclibc-gcc"
export AR_mips64_sgi_redox="$SDK/bin/mips64-buildroot-linux-uclibc-ar"
export CFLAGS_mips64_sgi_redox="-mabi=n32 -EB -march=mips3 -mhard-float -mno-abicalls -fno-pic -G0 -ffreestanding -Os -ffunction-sections -fdata-sections"
cd "$ROOT/$DIR"
cargo build --release "$@" \
  --config "patch.crates-io.libc.path=\"$ROOT/vendor/libc-0.2.191\"" \
  --config "patch.crates-io.redox_syscall.path=\"$ROOT/vendor/redox_syscall-0.9.6\"" \
  --config "patch.crates-io.redox_syscall_05.path=\"$ROOT/vendor/redox_syscall-0.5.11\"" \
  --config "patch.crates-io.redox_syscall_05.package=\"redox_syscall\"" \
  --target "$ROOT/targets/mips64-sgi-redox.json" \
  -Zbuild-std=std,panic_abort -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec
