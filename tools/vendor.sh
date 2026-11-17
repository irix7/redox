#!/usr/bin/env bash
# Materialise vendor/<crate> from the cargo registry plus vendor/patches/<crate>.patch.
#
# The bootstrap's Cargo.toml patches `redox_syscall` and `libc` to these
# directories, which carry the mips64 architecture files. The directories are
# not committed; this script recreates them. Run `cargo fetch` in
# recipes/core/base/source/bootstrap first if the registry lacks a crate.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REGISTRY="$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/index.crates.io-* | head -1)"
for patch in "$ROOT"/vendor/patches/*.patch; do
  crate="$(basename "$patch" .patch)"
  src="$REGISTRY/$crate"
  dst="$ROOT/vendor/$crate"
  if [ ! -d "$src" ]; then
    echo "vendor: $src is not in the registry; run cargo fetch first" >&2
    exit 1
  fi
  rm -rf "$dst"
  cp -r "$src" "$dst"
  rm -f "$dst/.cargo_vcs_info.json" "$dst/.cargo-ok"
  patch -s -p1 -d "$dst" < "$patch"
  echo "vendor: $crate ready"
done
