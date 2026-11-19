# Sourced by the Indy build scripts. Picks the Rust toolchain and CPU flags.
#
#   INDY_RUST_TOOLCHAIN   rustup toolchain name (default: r4600, the stage-1
#                         build with the R4600 LLVM model; see the
#                         mips-dev skill, toolchains.md). Set it to a channel
#                         such as nightly-2026-10-03 to use the rustup nightly.
#   INDY_RUSTFLAGS        extra rustc flags for Indy targets (default: the
#                         R4600 CPU flag when the r4600 toolchain is used).
#
# The r4600 toolchain needs libstdc++ at run time, which is copied under
# /mnt/europa/rust-r4600/lib so the path does not depend on the nix store.

: "${INDY_RUST_TOOLCHAIN:=r4600}"
export INDY_RUST_TOOLCHAIN
export RUSTUP_TOOLCHAIN="$INDY_RUST_TOOLCHAIN"
if [ "$INDY_RUST_TOOLCHAIN" = "r4600" ]; then
  export LD_LIBRARY_PATH="/mnt/europa/rust-r4600/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  : "${INDY_RUSTFLAGS:=-C target-cpu=r4600}"
fi
export INDY_RUSTFLAGS="${INDY_RUSTFLAGS:-}"
