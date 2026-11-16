# ADR 0003: Address space, page tables and TLB refill

Status: accepted, 2026-11-17.

## Decision

Virtual layout (32-bit addresses, `Status.KX/SX/UX = 0`):

| Range | Use |
|---|---|
| `0x0000_0000`..`0x8000_0000` | useg: user, TLB mapped (`USER_END_OFFSET`) |
| `0x8000_0000`..`0xA000_0000` | kseg0: cached direct map, `PHYS_OFFSET = 0x8000_0000`; the kernel image and all RAM |
| `0xA000_0000`..`0xC000_0000` | kseg1: uncached direct map; device registers |
| `0xC000_0000`.. | kseg2: TLB mapped kernel memory; heap at `0xC000_0000`, 1 MiB |

Page tables (`rmm/src/arch/mips.rs`): two levels of 1024 four-byte entries,
4 KiB pages. A leaf entry is shaped like EntryLo: frame number at bits 6..25,
`V` (present) bit 1, `D` (writable) bit 2. Software bits above the frame
number: 29 uncached, 30 no-execute, 31 user. The refill handler builds
EntryLo from the frame number and the cache bit (`3` cacheable noncoherent,
`2` uncached); bits 30 and 31 never reach the hardware.

TLB: 48 entries, one ASID (0), `PageMask` 0, `Wired` 0. `set_table` stores
the root in a static and flushes every entry. Refill is software: all three
vectors (`0x8000_0000`, `0x8000_0080`, `0x8000_0180`) jump to one entry that
saves the frame, clears `EXL`, and walks the table from Rust; a miss in the
table goes to Redox's generic `page_fault_handler`.

## Reasons

- The R4400 has no hardware page walker and no page-table base register.
- Keeping the leaf format close to EntryLo makes the refill a mask and an or.
- The Indy has 32-bit physical addresses, so 20 frame-number bits suffice and
  six bits are free for software flags inside `ENTRY_FLAGS_MASK`.
- One ASID with a full flush per switch is the smallest correct scheme; ASID
  reuse is an optimisation for after the first userspace runs.

## Consequences

- kseg0 and kseg1 never consult the TLB, so kernel text, data, page tables
  and device registers need no mappings; the physmap `rmm` builds for kseg0
  is unused by hardware and costs 32 KiB of tables per 32 MiB of RAM.
- Exceptions must not use 64-bit addressing; `la` yields sign-extended
  kseg0 addresses, which the CPU accepts in 32-bit mode.
- Physical `0x0000_0000`..`0x0008_0000` aliases the first 512 KiB of RAM on
  the Indy, which is how the vectors land in RAM. The first 256 KiB of RAM
  (`0x0800_0000`..`0x0804_0000`) is reserved for the PROM and the vectors.
