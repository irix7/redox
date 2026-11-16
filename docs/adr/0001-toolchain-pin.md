# ADR 0001: Toolchain pin

Status: accepted, 2026-11-17. Decides D1 in bootstrap.md.

## Decision

The fork pins `nightly-2026-10-03` with `rust-src`, `rustfmt`, `clippy` and
`llvm-tools-preview` in `rust-toolchain.toml`. The kernel checkout under
`recipes/core/kernel/source` keeps upstream's `nightly-2026-05-24`; the Indy
build overrides it with `RUSTUP_TOOLCHAIN` from `mk/indy.mk`.

## Reasons

- `-Zbuild-std` for a custom target JSON needs `rust-src`, and the link step
  needs `rust-lld`, `llvm-objcopy` and `llvm-readobj`, which only
  `llvm-tools-preview` ships. Redox's own pin lacks that component.
- `irix7/os` builds the same n32 target on this channel, so its target JSON
  field values and its `-Zbuild-std` fixes carry over unchanged.
- One channel for every crate in this fork keeps `core` and `alloc` built
  once per target.

## Consequences

`tools/env.sh` resolves the pin from the fork's `rust-toolchain.toml`, so the
toolchain binaries are found from any directory. The cookbook's binary prefix
is not used for MIPS: `PREFIX_USE_UPSTREAM_RUST_COMPILER=1` is the path when
userspace recipes are built.
