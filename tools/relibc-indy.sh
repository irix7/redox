#!/usr/bin/env bash
# Build relibc for the Indy target and install a static sysroot.
#
#   tools/relibc-indy.sh [SYSROOT]      default: build/indy/sysroot
#
# Produces lib/libc.a (relibc plus the Rust libm), lib/crt0.o, crti.o,
# crtn.o and empty libm, libdl, libpthread, librt and libgcc_s archives
# (std links -lgcc_s on Redox; the port has no unwinder). No headers (cbindgen is not installed) and no
# libc.so: the port links statically. C needs the Bootlin n32 SDK at
# INDY_MIPS_SDK (default /mnt/europa/toolchains/mips64-n32--uclibc--stable-2026.08-1)
# for relibc's one C file.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SYSROOT="${1:-$ROOT/build/indy/sysroot}"
SDK="${INDY_MIPS_SDK:-/mnt/europa/toolchains/mips64-n32--uclibc--stable-2026.08-1}"
SRC="$ROOT/recipes/core/relibc/source"
unset RUSTUP_TOOLCHAIN
. "$ROOT/tools/env.sh"

unset RUSTUP_TOOLCHAIN
export RUSTUP_TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT/rust-toolchain.toml")"
export TARGET=mips64-sgi-redox
export TARGET_SPEC="$ROOT/targets/mips64-sgi-redox.json"
export USE_RUST_LIBM=1
export CARGOFLAGS="-Z build-std=core,alloc,compiler_builtins -Zjson-target-spec"
export CC_mips64_sgi_redox="$SDK/bin/mips64-buildroot-linux-uclibc-gcc"
export CFLAGS_mips64_sgi_redox="-mabi=n32 -EB -march=mips3 -msoft-float -mno-abicalls -fno-pic -G0 -ffreestanding"
export AR="$SDK/bin/mips64-buildroot-linux-uclibc-ar"
export NM="$SDK/bin/mips64-buildroot-linux-uclibc-nm"
export OBJCOPY="$SDK/bin/mips64-buildroot-linux-uclibc-objcopy"

BUILD="$SRC/target/$TARGET/release"
make -C "$SRC" "$BUILD/librelibc.a" "$BUILD/crt0.o" "$BUILD/crti.o" "$BUILD/crtn.o"

mkdir -p "$SYSROOT/lib"
cp "$BUILD/librelibc.a" "$SYSROOT/lib/libc.a"
cp "$BUILD/crt0.o" "$BUILD/crti.o" "$BUILD/crtn.o" "$SYSROOT/lib/"
ln -sf crt0.o "$SYSROOT/lib/crt1.o"
rm -f "$SYSROOT"/lib/lib{m,dl,pthread,rt,gcc_s}.a
for lib in m dl pthread rt gcc_s; do "$AR" -rcs "$SYSROOT/lib/lib$lib.a"; done
ls -l "$SYSROOT/lib"
