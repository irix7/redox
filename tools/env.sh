# Source this before building the Indy port.
#
# Puts rust-lld, llvm-objcopy and llvm-readobj from the pinned toolchain on
# PATH (they live beside the toolchain, not in its bin/), and a host C compiler
# and GNU make for build scripts and host tools when none is on PATH.
# Resolve the fork's pin even when sourced from a nested checkout (the
# cookbook's kernel source carries upstream's own rust-toolchain.toml).
pin="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$(dirname "${BASH_SOURCE:-$0}")/../rust-toolchain.toml")"
sysroot="$(rustc "+$pin" --print sysroot)"
host="$(rustc "+$pin" -vV | sed -n 's/^host: //p')"
export PATH="$sysroot/lib/rustlib/$host/bin:$PATH"
if ! command -v cc >/dev/null 2>&1; then
  for candidate in /nix/store/*-gcc-wrapper-*/bin; do
    if [ -x "$candidate/cc" ]; then
      export PATH="$candidate:$PATH"
      break
    fi
  done
fi
if ! command -v make >/dev/null 2>&1; then
  for candidate in /nix/store/*-gnumake-*/bin; do
    if [ -x "$candidate/make" ]; then
      export PATH="$candidate:$PATH"
      break
    fi
  done
fi
unset sysroot host pin
