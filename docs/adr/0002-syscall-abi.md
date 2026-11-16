# ADR 0002: MIPS n32 system call ABI

Status: accepted, 2026-11-17. Decides D3 in bootstrap.md and the kernel half
of D2.

## Decision

- Trap: the `syscall` instruction. The kernel adds 4 to EPC on return.
- Number in `$v0`; arguments in `$a0`..`$a5` (n32 has eight argument
  registers, so six arguments need no stack).
- Result in `$v0` as Redox's demultiplexed `usize`: an error is
  `usize::MAX - errno + 1`, decoded by `Error::demux`. `$v1` is clobbered.
- Thread pointer in `$gp` (`$28`). Kernel and userspace are static with no
  GOT (`+noabicalls`), so the register is free; the n32 libc work in
  `irix7/os` proved a 16-byte TCB header there.
- Trap frame: `$1`..`$31` as 64-bit values, then EPC, Status, Cause,
  BadVAddr, HI, LO and a pad word: 304 bytes.

## Reasons

Redox's `syscall` crate, `proc:` scheme and signal delivery all assume a
single result register and `Error::demux`. The `$v0`/`$v1` status channel
the sibling uses would need a second decode path in every consumer. Taking
Redox's convention keeps relibc's Redox PAL as the model for the MIPS PAL
(D2 option 1: a new PAL against this ABI).

## Consequences

`syscall/src/arch/mips64.rs` defines `IntRegisters`, `EnvRegisters { tp }`,
`FloatRegisters` and `Exception { code, address }` (Cause ExcCode and
BadVAddr). Soft float: the FPU stays disabled (`Status.CU1 = 0`) and
`get_fx_regs` returns zeros until a float ABI is decided.
