# Bootstrapping Redox on the SGI Indy — 32 MB

**Audience:** agents and engineers working in `irix7/redox`.
**Status:** T1 and T3 to T7 pass on iris (2026-11-17): `make indy-gate` netboots kernel plus
initfs on a 32 MiB `indy_ip24`, base's `init` starts `randd`, the Seeq driver, `netstack` and
`dhcpd` and a shell reachable on TCP port 23; the gate waits for the DHCP lease, runs a
command over TCP and reboots to the PROM. Nothing has run on real hardware yet. T2 (the
x86_64 floor) is not done. This file says
where the pieces are, what has been proved on this machine, and what the 32 MB budget allows.
**Fork base:** `redox-os/redox` mirror at `f71658dfca2b978bee55a4c19d95ab926a948c21`
(2026-11-16). See [Provenance](#provenance-and-rebasing).

Read this file end to end before your first change. It is deliberately concrete: paths,
versions, commands and byte counts, not principles.

---

## Contents

1. [The target and the one constraint](#1-the-target-and-the-one-constraint)
2. [The sibling project](#2-the-sibling-project)
3. [Toolchains](#3-toolchains)
4. [The emulator gate](#4-the-emulator-gate)
5. [Boot path: `bootp()redox`](#5-boot-path-bootpredox)
6. [Where the MIPS work lands in this fork](#6-where-the-mips-work-lands-in-this-fork)
7. [The 32 MB budget](#7-the-32-mb-budget)
8. [The strip list](#8-the-strip-list)
9. [Research map](#9-research-map)
10. [Rules for agents](#10-rules-for-agents)
11. [First tasks](#11-first-tasks)
12. [Open decisions](#12-open-decisions)
13. [Provenance and rebasing](#13-provenance-and-rebasing)
14. [Appendix: paths and commands](#14-appendix-paths-and-commands)

---

## 1. The target and the one constraint

| Item | Value |
|---|---|
| Machine | SGI Indy (IP24), R4400SC, 75–100 MHz |
| CPU | MIPS III, 32-bit, **big-endian** |
| ABI | n32 (`-mabi=n32`), soft float, no abicalls, `-G0` |
| RAM | **32 MiB** |
| Boot | `boot -f bootp()redox` at the PROM Command Monitor; TFTP from the network |
| Display | Newport (REX3). No GPU acceleration; software rendering only |
| Console | Z85C30 serial in the IOC, channel B |
| Reference hosts | iris emulator (IP24 with an embedded PROM fallback), CI on `x86_64-linux` |

**The one constraint is 32 MiB.** Every other decision in this plan is downstream of it.

A second limit comes from the PROM (measured 2026-11-17 on iris's embedded IP24 PROM): a
netboot image loaded at `0x8804_0000` must fit its free-memory area. A 6,865,104-byte image
loads; a 7,684,304-byte one fails with `Text start 0x8040000, size 0x754000 doesn't fit in a
FreeMemory area`. Keep the TFTP file under 6.8 MB.
Redox's QEMU defaults are 1–2 GiB: `mk/qemu.mk` sets `QEMU_MEM` to 1024 on one architecture
and 2048 on the others. The default desktop configuration ships COSMIC, fonts and a browser.
The kernel's release profile carries `debug = "full"`, and LTO is commented out
(`redox-os/kernel` `Cargo.toml`). None of that survives a 32 MiB boot unchanged.

The second constraint is the ABI: n32 big-endian. Redox has **no MIPS architecture layer**.
Its kernel contains `src/arch/{aarch64,riscv64,x86,x86_64,x86_shared}`. The bootloader builds
only for BIOS (`i586`/`x86_64`) and UEFI (`aarch64`/`x86_64`/`riscv64gc`). The `mips` support
in this port is new work, not configuration.

Do not treat this as a port of Redox to "MIPS" in the abstract. It is a port to **one machine,
one ABI, one memory size, one boot command**. Widen the target only after the first boot.

---

## 2. The sibling project

[`irix7/os`](https://github.com/irix7/os) is a Rust-native operating system for the same
machine. It is not a source to copy from — its architecture is different — but it has already
answered the machine questions this port will meet:

| Already proved in `irix7/os` | Where |
|---|---|
| MIPS III n32 big-endian boot on the Indy, from the PROM, in iris | `docs/internals/boot-protocol.md`, `tools/indy-netboot.sh` |
| An ECOFF image the Indy PROM accepts, with the ARCS entry | `tools/sgi-netboot/src/ecoff.rs` |
| Rust builds for a custom n32 target with `-Zbuild-std` and a target JSON | `targets/`, `docs/research/rust-std-build-spike.md` |
| relibc on n32: `$28` TCB, errno, `stat`, pthreads, futex, fork/exec | `irix7/relibc` branch `irix7-pal`; `docs/research/relibc-pal-bringup.md` |
| A deterministic boot gate with markers and serial logs on iris | `docs/project/testing.md`, `tools/relibc-pal/boot-probes.sh` |
| Driver-level detail for the IOC serial, HPC3, SCSI, Seeq, DS1386, REX3 | `docs/research/indy-hardware.md`; `irix7/iris` |
| A 32 MiB machine slice: module load from ATA, gated | `docs/hardware/powerpc-module-slice.md`, `docs/research/powerbook-g3.md` |

**Local checkout on this workstation:** `/mnt/europa/os` (the `docs/` links above are relative
to it). `irix7/iris` is at `/mnt/europa/iris`. `irix7/relibc` builds from
`/tmp/opencode/relibc-pal-src` by default.

The two rules:

1. **Link, do not copy.** Cite the document and the pinned commit. If a fact matters, verify
   it against the source in `irix7/os` and then re-state it here with a date.
2. **Do not fork their code into this repository.** `irix7/os` is GPL-3.0; Redox is MIT.
   Copying kernel or driver code across that boundary would force a licence change on this
   fork. Read for understanding, write your own.

---

## 3. Toolchains

There are three toolchains in play. Know which one you are using.

### 3.1 Rust — the pinned nightly

| Toolchain | Channel | Components | Used for |
|---|---|---|---|
| This fork | `nightly-2026-05-24` (`rust-toolchain.toml`) | `rust-src`, `rustfmt`, `clippy` | Redox's own build |
| `irix7/os` | `nightly-2026-10-03` (`rust-toolchain.toml`) | `rust-src`, `rustfmt`, `clippy`, `llvm-tools-preview` | the MIPS n32 target |

The two channels differ. **Decide one pin for this fork and record it** (see
[First tasks](#11-first-tasks)). The MIPS target spec and `-Zbuild-std` need `rust-src` and
`llvm-tools-preview`; Redox's current pin has the first and not the second.

A custom target JSON cannot use a prebuilt `core`/`alloc`/`std`. Every build of a MIPS target
goes through:

```sh
cargo build --release \
  -Z build-std=core,alloc \
  -Zjson-target-spec \
  --target targets/mips64-sgi-redox.json \
  -p kernel
```

`rust-lld` and `llvm-objcopy` live beside the toolchain, not in its `bin/`. The sibling's Nix
shell exports the path explicitly; do the same in any shell you write:

```sh
sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | sed -n 's/^host: //p')"
export PATH="$sysroot/lib/rustlib/$host/bin:$PATH"
```

**Do not add a second Nix flake before reading `irix7/os/flake.nix`.** It is the reference for
the pinning discipline: one `rust-toolchain.toml`, one overlay, one `shellHook` that puts
`rust-lld` on `PATH`, and a separate devshell for a second architecture. Copy the shape, not
the file.

### 3.2 The MIPS C toolchain

Redox needs a target C compiler even though it is Rust-first: `recipes/core/relibc` builds C
(`compiler = "gcc-no-lto"`), and C dependencies appear in recipes. On MIPS the tuple is new, so
`mk/prefix.mk` — which normally builds `$(GNU_TARGET)` GCC for the target — has to be taught
it, or pointed at an existing cross compiler.

We already have one.

| Item | Value |
|---|---|
| Compiler | Bootlin `mips64-n32--uclibc--stable-2026.08-1` |
| GCC / binutils | GCC 15.3.0, binutils 2.45.1 |
| Env vars used by the sibling | `MIPS_CC`, `MIPS_AR` |
| Newer series | `irix7/project` — GCC 16.2 patch series, rig and stock-rebuild scripts |
| Reconstruction notes | `/mnt/europa/sgi-toolchain-scratch` (GCC 15.3, GCC 16.2, binutils 2.23 patches) |

Working invocation, from `irix7/os/tools/relibc-pal/build.sh`:

```sh
export MIPS_CC=/path/to/mips64-buildroot-linux-uclibc-gcc
export MIPS_AR=/path/to/mips64-buildroot-linux-uclibc-ar
tools/relibc-pal/build.sh /tmp/opencode/relibc-pal-work/out
```

For this fork the question is narrower: does `recipes/core/relibc` need a full GCC, or only a
`gcc` that can compile `crt` and a handful of C files for the target? Answer it by building
relibc for MIPS outside the cookbook first, exactly as the sibling does. Do not start by
extending `mk/prefix.mk`.

### 3.3 The Redox build system toolchain

The cookbook normally downloads a pinned Rust and GCC into `$(PREFIX)` and cross-compiles
everything against relibc. `PREFIX_BINARY=1` is the default (binary prefix, much faster);
`PREFIX_USE_UPSTREAM_RUST_COMPILER=1` is the escape hatch for a Tier 3 target, and its comment
already anticipates `-Zbuild-std`. For MIPS that escape hatch is not an experiment — it is the
only path. Expect to make it the default for the MIPS config.

### 3.4 Target specification

Two target JSONs exist in the sibling and are the starting point:

| File | Use |
|---|---|
| `targets/mips3-sgi-irix7-n32.json` | freestanding kernel: `relocation-model: static`, `panic-strategy: abort`, `crt-objects-fallback: false` |
| `targets/mips3-sgi-irix7-native-n32.json` | native userland with a libc |

Copy the field values, not the vendor. The load-bearing ones for this port:

| Field | Value | Why |
|---|---|---|
| `arch` | `mips64` | LLVM's MIPS backend; 32-bit pointers below |
| `llvm-abiname` | `n32` | the ABI the PROM and the machine expect |
| `data-layout` | `E-m:e-p:32:32-...-n32:64-S128` | big-endian, 32-bit pointers, 64-bit `i64` alignment |
| `target-endian` | `big` | MIPS on the Indy is big-endian |
| `cpu` | `mips3` | R4400; do not emit MIPS IV |
| `target-pointer-width` | `32` | |
| `max-atomic-width` | `64` | |
| `features` | `+mips3,+soft-float,+noabicalls` | the Indy has an FPU, but the sibling's ABI is soft float; match it until you have a reason not to |
| `linker` | `rust-lld` | no GNU ld in the target path |
| `relocation-model` | `static` | no PIC in the kernel |
| `panic-strategy` | `abort` | Redox already does this; keep it |

Redox's kernel is `no_std`, so `core` and `alloc` are the whole story for the kernel. Userspace
needs `std`, and that is a separate, later project (see `docs/research/rust-std-irix7.md`).

---

## 4. The emulator gate

There is no MIPS hardware in CI. `iris` is the gate, and it is good enough to boot IRIX 6.5.

| Piece | Value |
|---|---|
| Emulator | `iris`, an IP24/IP22/IP28 emulator; IP24 has an embedded PROM fallback |
| Pinned CI version used by the sibling | `v0.1.13+irix7` |
| CI driver | `iris-ci` from the same release |
| Local checkout | `/mnt/europa/iris` |
| Machine profile | `indy_ip24` |
| RAM | `banks = [32, 0, 0, 0]` — bank sizes in MiB |

The pattern to copy is marker-based, not screen-scraping. `irix7/os` starts the emulator with a
CI socket and a serial-log mirror, waits for readiness, issues `start`, and asserts exact
strings with a timeout. Read `docs/project/testing.md` §0 and
`tools/relibc-pal/boot-probes.sh`; then copy the structure, not the file.

Minimal shape of a run:

```toml
banks = [32, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[network]
tftp_dir = "/tmp/opencode/redox-netboot"
```

```sh
IRIS_NO_IDLE=1 iris --config redox.toml --ci \
  --serial-log redox-serial.log \
  --monitor-port 18893 \
  --ci-socket /tmp/opencode/redox.sock

iris-ci --socket /tmp/opencode/redox.sock status --wait-ready 30
iris-ci --socket /tmp/opencode/redox.sock start
```

Two rules that are not obvious:

- **`IRIS_NO_IDLE=1`.** Without it the emulator may idle a halted guest and a dead guest looks
  like a slow one.
- **Assert a marker and an exit, not silence.** A gate passes when a specific string appears
  *and* the machine reaches the expected state. "No error printed" is not a pass.

`iris` is also the fastest way to check a hypothesis about the PROM, the IOC or the memory
controller before writing kernel code: it is a Rust program, and the device models are in
`src/`.

---

## 5. Boot path: `bootp()redox`

The Indy PROM loads `redox` from a BOOTP/TFTP server and jumps to it. There is no disk
bootloader in the path, and none needs to be written first.

```
PROM  ──BOOTP──▶  server (gateway or lab host)
      ◀──TFTP───  file named "redox", ECOFF, big-endian MIPS
      ──▶ jump to entry, ARCS still alive, RAM already sized
```

This is the same path the sibling already uses with the file named `irix7`:

```sh
# irix7/os
tools/indy-netboot.sh /var/lib/sgi-netboot
# at the PROM:
boot -f bootp()irix7
```

For this fork, `make` the image `redox` and point the same TFTP directory at it.

### 5.1 The image format

The PROM does not load ELF. It loads **ECOFF**. The sibling's converter is small and complete:
`irix7/os/tools/sgi-netboot/src/ecoff.rs` (ELF32 big-endian MIPS → ECOFF, section headers laid
out the way Linux's `elf2ecoff` writes them for the same PROM). It validates that the entry is
inside the loaded image and reports `load ... , bss ... , entry ...`.

Build it as a standalone Rust tool in this fork. It has no dependencies worth inheriting.

### 5.2 What the PROM hands over

Verified in `irix7/os/docs/internals/boot-protocol.md` and in the `factory-prom` kernel build:

- CPU in kernel mode, big-endian, caches on, TLB off, interrupts disabled;
- DRAM already sized and mapped by the PROM through `MEMCFG0`/`MEMCFG1`;
- ARCS alive and usable until the kernel takes over the TLB and caches;
- no `BootInfo` structure — the handoff is registers, and a loader may add a record later;
- NVRAM must be left alone: the PROM owns it (`irix7/os` `kernel/src/nvram.rs`, the
  `factory-prom` build).

Redox's kernel entry expects `KernelArgs` instead: `kernel_base`, `kernel_size`, `stack_base`,
`stack_size`, `env_base`, `env_size`, and `hwdesc_base`/`hwdesc_size` (an RSDP or a device
tree). On the Indy there is neither ACPI nor a device tree, so `hwdesc` is null and the MIPS
arch layer must discover devices the way the machine does: fixed IOC/HPC3 addresses, not
enumeration. `redox-os/kernel/src/startup/mod.rs` is the contract to adapt.

**Read before writing the entry stub:** `irix7/os/docs/internals/boot-protocol.md` §2–§3. The
register handoff and the 256-byte `BootInfo` (`"I7BI"`, CRC32C, 8-aligned) are the only
written record of how this machine is entered from the PROM, and the failure modes listed
there are the ones you will hit.

### 5.3 Two different boot philosophies — pick one deliberately

| | `irix7/os` | this fork |
|---|---|---|
| Loader | its own ROM, `bootp()irix7`, then decompress and jump to `0x8804_0000` | the SGI PROM, `bootp()redox`, direct ECOFF load |
| RAM sizing | the ROM probes and programs the memory controller | the PROM has already done it |
| NVRAM | the ROM owns and rewrites it | the PROM owns it; leave it alone |
| Kernel entry | assembly `_start`, register handoff | `KernelArgs`, arch `start` |

Do **not** port the sibling's ROM. The PROM is the loader here, which removes the RAM-sizing,
decompression and serial-init work entirely — and removes the need to reimplement them wrongly.

---

## 6. Where the MIPS work lands in this fork

Four surfaces, in dependency order.

### 6.1 The build system

| Surface | Change |
|---|---|
| `mk/config.mk` | `ARCH` accepts `mips64` (or `mips`); `GNU_TARGET` becomes the MIPS tuple |
| `config/mips64/` | new directory; an image configuration for the Indy |
| `recipes/core/kernel` | builds with the custom target JSON and `-Zbuild-std` |
| `recipes/core/bootloader` | the arch gate has no MIPS branch; replace it with an ECOFF/TFTP image step |
| `recipes/core/relibc` | C compiler and target tuple |
| `mk/qemu.mk` | **not applicable**. Do not add MIPS to QEMU; the gate is iris |

`mk/disk.mk` builds a `harddrive.img` with a bootloader written into it. On the Indy the
deliverable is a **file named `redox` in a TFTP directory**, not a disk image. Do not contort
`disk.mk` to produce it; add a separate target and leave the disk path for a later SCSI root.

### 6.2 The kernel arch layer

`redox-os/kernel` currently selects an arch with `#[cfg(target_arch = ...)]` and re-exports it
from `src/arch/mod.rs`. A `mips64` module must provide the same surface the other arches do.
Read one small arch end to end before starting — `src/arch/x86_64/mod.rs` and
`src/arch/x86_shared/` are the shortest complete pair — and list what it provides:

- startup and stop, kernel entry and per-CPU bring-up;
- paging: the RMM implementation for the TLB (MIPS `TLBWI`/`TLBWR`, not page tables);
- interrupts and exceptions, including the XTLB vector;
- time: the CP0 Count/Compare pair, not the PIT/APIC;
- user copy (`arch_copy_to_user`), which on MIPS needs cache maintenance on the destination;
- context switch and FPU state.

The sibling has already debugged the parts that bite on this CPU: `Status.KSU`/`UX`, the XTLB
vector, LL/SC-safe cacheable user pages, R4000 `PageMask` encodings, the 64-bit trap frame and
LL-bit clearing on traps (see `irix7/os/AGENTS.md`, "Userland and POSIX"; and
`docs/research/m2-remediation.md`). Those are the invariants to keep, whatever the code looks
like.

### 6.3 The syscall ABI

`redox_syscall` defines 32 syscall numbers (`syscall/src/number.rs`), with classes encoded in
the number (`SYS_CLASS_*`, `SYS_RET_FILE`). The x86 path enters through the `syscall`
instruction and a fixed register block. MIPS enters through `syscall` too, but with a
different convention, and n32 is not o32.

There is a working precedent. `irix7/relibc` branch `irix7-pal` uses:

- syscall number in `$v0`;
- arguments in `$a0`–`$a3`;
- **status in `$v0`, value in `$v1`** on return, mapped by the libc through `STATUS_ERRNO` to
  `Err(Errno(...))`.

That negative-errno-style channel is deliberate: it keeps a single trap entry for every call
and avoids overloading a return register. Redox's `Result`-returning syscall surface maps to
it cleanly. **Write this down as an ADR before implementing it** — the sibling wrote it twice
(once provisional, once real) and paid for the first one.

Related, already solved there: the n32 `struct stat` is 136 bytes and is passed to the kernel
directly rather than copied through an intermediate; the auxiliary vector arrives in `$a3`;
the TCB is `$28` with a 16-byte header (`self`, `dtv`, `errno`, libc-private). Read
`docs/programming/native-libc.md` and `docs/research/c-probe-bringup.md` before inventing a
different layout.

### 6.4 relibc and the filesystem

Two hard problems that are easy to miss.

**relibc.** Upstream relibc has Redox and Linux PALs. `irix7/relibc` adds an n32 PAL against
IRIX 7 syscalls. For this fork there are three options, and they are not equivalent:

1. a new `mips`/`redox-mips` PAL in relibc that talks to the Redox kernel's n32 syscalls;
2. make the Redox kernel adopt the sibling's n32 syscall channel so the existing PAL is close
   to reusable;
3. leave relibc alone and stub a C library — not viable for anything beyond a probe.

Option 1 is the honest one. Option 2 buys a lot but couples the two projects. **Decide it
explicitly**; do not drift into it.

**On-disk byte order.** RedoxFS is the default filesystem, and its on-disk structures are
little-endian and were written for little-endian machines. On a big-endian Indy, every
structure read from disk is wrong unless the format is byte-order-explicit. This project has
already answered the equivalent question for EROFS and the answer was
**do not fork the format to big-endian**: `docs/research/erofs-big-endian-research.md`. Apply
the same conclusion here: make the *accessors* endian-explicit (read `u32` little-endian,
convert on use), keep one on-disk format, and test with a fixture image built on the host. Do
not add a big-endian variant of RedoxFS, and do not byte-swap whole blocks.

The same applies to `pkgar`/`pkgutils` package formats and to any manifest or index the image
uses.

---

## 7. The 32 MB budget

### 7.1 What the machine actually gives you

The sibling's layout, which is a good sizing reference even though Redox's will differ:

| Region | Extent | Size |
|---|---|---|
| Reserved low window (kernel, direct map, framebuffer shadow) | `0x0800_0000`–`0x08B0_0000` | 11 MiB |
| Framebuffer shadow (CPU-only, 1280×1024×24) | `0x0870_0000` | 3.75 MiB |
| Kernel stack top | `0x8830_0000` | — |
| First byte the frame allocator may hand out | `0x08B0_0000` | — |

On a 32 MiB machine that leaves **21 MiB** for everything userspace. The 3.75 MiB shadow is
the single largest line item and it exists only because the sibling mirrors a 24-bit
1280×1024 framebuffer. At 8-bit 640×480 the same shadow is **300 KiB**. Do not inherit the
bigger number.

### 7.2 A budget that fits

| Item | Budget | Note |
|---|---:|---|
| Kernel image (text + rodata + data) | 1 MiB | release, stripped, `opt-level=z` |
| Kernel heap and RMM structures | 1 MiB | fixed, no growth policy |
| Bootstrap/initfs | 2 MiB | **resident**: the kernel receives it as a contiguous physical region |
| Framebuffer shadow | 0.5 MiB | 8 bpp, 640×480 |
| Page cache | 4 MiB | capped; evict, never OOM |
| Base schemes and drivers | 4 MiB | only what the machine has |
| Shell and userland | 4 MiB | one shell, coreutils on demand |
| Slack | 15 MiB | headroom for a bad day |
| **Total** | **32 MiB** | |

The initfs line is the one people get wrong. Redox hands the bootstrap filesystem to the kernel
as a contiguous physical region (`KernelArgs`, `redox-os/kernel/src/startup/mod.rs`) — it is
**RAM**, unlike a disk root. Every megabyte of driver and utility in the initfs is a megabyte
the machine does not have. A 20 MiB initfs is not a slow boot; it is no boot.

### 7.3 The rules

1. **Every change that can affect memory reports bytes.** Before and after: kernel image size,
   initfs size, peak resident after boot. Put the numbers in the commit message.
2. **A change that raises a budget line needs a matching cut.** The table is not advisory.
3. **No unbounded caches.** Every cache has a cap and an eviction path, decided at review time.
4. **No allocation policy that can fail the boot.** Boot-path allocation is fixed-size or
   pre-reserved.
5. **Measure on the machine, not on the host.** A 32 MiB guest in iris, booted, with the
   accounting printed. Host `size` is not a measurement of RAM.

---

## 8. The strip list

Ordered by bytes returned per unit of risk. Do the top of the list first; it is almost free.

### 8.1 Image and build settings — free

| Change | Where | Why |
|---|---|---|
| Do not include the desktop configuration | `config/desktop.toml` (COSMIC, fonts, `netsurf`) | hundreds of MiB; the Indy has no GPU |
| Start from `minimal.toml`, then cut it | `config/minimal.toml` | it still pulls `pkgutils`, `ion`, `kibi`, `uutils-procps` |
| `opt-level = "z"` | kernel and recipes | size over speed is the right trade on a 75 MHz machine with 32 MiB |
| `panic = "abort"` | already set | keeps unwinding out of the image |
| Strip all binaries | `REPO_DEBUG=0` is the default; verify | debug info in a resident initfs is pure waste |
| Re-enable LTO where it is disabled | `redox-os/kernel` `Cargo.toml` (`#lto = true`); the last mirror merge is `no-lto` | find out why it went before turning it back on; then measure the size win |
| Drop `debug = "full"` from the kernel release profile | same file | full debug info does not belong in a 32 MiB kernel |

### 8.2 Kernel features — cheap

Redox's kernel defaults are `acpi`, `multi_core`, `numa`, `serial_debug`, `self_modifying`,
`x86_kvm_pv`. On the Indy, none of the last five apply, and ACPI does not exist:

- single core to start; `multi_core` off;
- no ACPI, no PCI enumeration — the Indy's bus is HPC3/GIO, at fixed addresses;
- no NUMA;
- `serial_debug` off in release;
- no `x86_kvm_pv`, no `self_modifying`.

A MIPS feature set should be a short list of what the machine has, not a subset of an x86 list.

### 8.3 The initfs and userspace — the big one

| Cut | Reason |
|---|---|
| `pkgutils`, package manager on the boot image | the image is built offline; packages are a later layer |
| `bash`, `git`, `curl`, `findutils`, `diffutils`, `gettext` | not needed to boot or to prove the port |
| `uutils-procps` | `ps` can come later; it is not boot-critical |
| Every driver for hardware the Indy does not have | x86 drivers, USB, audio, virtio |
| The installer | netboot only in v1 |
| Locale and iconv data in relibc | several MiB for a single-language machine |

Then add back one package at a time, measuring. The reverse order — start from desktop and cut
— never converges.

### 8.4 Runtime and services — architectural

| Risk | Why it matters on 32 MiB | Mitigation |
|---|---|---|
| Async runtimes (`tokio`) in daemons | several MiB of code and a runtime each | blocking threads, one process per service |
| TLS stacks (`rustls`), HTTP clients (`reqwest`), JSON (`serde_json`) | the sibling measured this combination as "a large fraction of RAM" on the Indy | keep them off the boot image; fetch with the PROM or a host-side tool |
| A Wasm runtime | the sibling's wasmi module is 1.26 MB and wants a 16 MiB heap | **do not** ship one in v1 |
| Per-process address-space quota | the sibling's default is 32 MiB per cell — more than this machine | 2–4 MiB per process, enforced |
| Page cache growth | RedoxFS caches; unbounded, it eats the machine | cap at 4 MiB, evict aggressively |
| Console graphics | a 24-bit shadow is 3.75 MiB | 8 bpp 640×480 = 300 KiB, or serial first |

The last row is a sequencing decision: **boot to serial, add the framebuffer second.** The
sibling did the same and its graphics path is the more expensive half.

---

## 9. Research map

Everything below is in `irix7/os`, reachable at `https://github.com/irix7/os/blob/main/...`.
Read only what your task needs; the index is `docs/research/README.md`.

### Machine and ABI

| Document | Read it for |
|---|---|
| `docs/research/indy-hardware.md` | CPU, caches, TLB, HPC3, SCSI, Seeq, HAL2, Newport, Vino, DS1386, GIO, PROM |
| `docs/internals/boot-protocol.md` | the PROM handoff, `BootInfo`, the memory map, failure behaviour |
| `docs/programming/native-abi.md` | n32 calling convention, capability ABI, quotas, trap frame |
| `docs/programming/native-libc.md` | `stat`, `dirent`, `$28` TCB, auxv, the binary libc contract |
| `docs/programming/posix.md` | the POSIX.1-2024 baseline the native libc targets |
| `docs/hardware/indy.md` | (placeholder today; the research file above is the real source) |

### Rust on a new target

| Document | Read it for |
|---|---|
| `docs/research/rust-std-irix7.md` | what `std` requires, the native-vs-compat split, the PR plan |
| `docs/research/rust-std-build-spike.md` | the measured method: patched `rust-src`, target JSON, linker script, exact patch set |
| `docs/research/std-hello-bringup.md` | the first linked static `std` image, the 42-symbol shim, the gaps that remained |
| `docs/research/c-probe-bringup.md` | the 136-byte `stat`, `CELL_E_*` status channel, auxv, `$28`, serial transcript |
| `docs/research/relibc-n32-bringup.md` | relibc on n32: TLS, `setjmp`, pthreads, the PAL scaffold |
| `docs/research/relibc-pal-bringup.md` | the full PAL: errno channel, `stat`, `$28`, auxv, ramfs, fork/exec/wait |
| `docs/research/relibc-pthread-bringup.md`, `relibc-sleep-bringup.md` | futex, scheduling and sleep on a cooperative kernel |

### Design and constraints

| Document | Read it for |
|---|---|
| `docs/research/compressed-ram.md` | why compression does not rescue 32 MiB, and the 64 MiB "full stack" floor this port has to beat |
| `docs/research/native-window-system-research.md` | framebuffer arithmetic, software rendering budgets at 32 MB, what a window system costs |
| `docs/research/erofs-big-endian-research.md` | the byte-order decision: do not fork a format to big-endian |
| `docs/research/rust-repository-sourcing.md` | licence/dependency audit, and the "does it fit in 32 MiB?" gate |
| `docs/research/rust-os-design-research.md` | a survey of Rust OS design choices, useful when Redox's choice looks arbitrary |
| `docs/research/plan9-rust-research.md` | schemes, namespaces and capabilities — the closest thing to Redox's model |
| `docs/research/bulk-batching.md`, `wire-efficiency.md` | transport cost on a 1.1 MB/s Ethernet |
| `docs/hardware/powerpc-devshell.md`, `powerpc-module-slice.md`, `docs/research/powerbook-g3.md` | a second 32 MiB machine: the frame-allocator sizing, the 26 MiB minimum for a 2 MiB slot, the boot-image constraints |

`docs/project/testing.md` is the gate philosophy: markers, timeouts, evidence bundles, and the
difference between a test and a benchmark.

---

## 10. Rules for agents

1. **Upstream does not take agent-generated code.** `CONTRIBUTING.md` states that Redox OS
   does not accept LLM-generated contributions. All work stays in this fork. Do not open merge
   requests, issues or discussions upstream, and do not present fork work as upstream work.
2. **This fork is MIT.** Do not copy GPL-3.0 code from `irix7/os` into it. Cite and reimplement.
3. **Rust first.** A new component, tool or dependency is Rust unless there is no maintained
   Rust option. If you must bind a non-Rust one, record its licence, target assumptions, build
   requirements and replacement plan. This mirrors the sibling's standing rule.
4. **Australian English in prose.** `behaviour`, `colour`, `centre`, `organise`, `licence` as a
   noun. Code identifiers and upstream quotations are exempt.
5. **Memory is a reviewed property.** See §7.3. A change without before/after bytes is
   incomplete.
6. **Pre-release freedom.** There are no deployed users. Replace an internal interface
   outright instead of adding a compatibility path, a version negotiation or a migration.
   Update every producer and consumer in the same change, and delete the old path and its
   tests. This is the sibling's standing policy and it is the right one here.
7. **No generated YAML in place of a real change.** If the build system needs a new rule, add
   the rule.
8. **Commit sign-off.** `git commit -s`, per `CONTRIBUTING.md`.
9. **One machine, one ABI.** Do not generalise the MIPS port to "all MIPS" or add a second
   board before the Indy boots.
10. **Record what you measured.** If you discover a hardware fact, put it in this file or a
    linked document with a date and the evidence. If you discover a trap, write it down where
    the next agent will look.

---

## 11. First tasks

In order. Each one ends with evidence, not an opinion.

### T1 — Pin the toolchain and prove a MIPS object builds in this repository

**Done 2026-11-17** ([ADR 0001](docs/adr/0001-toolchain-pin.md)). `tools/indy-probe` builds
with `-Zbuild-std=core` for `targets/mips64-sgi-redox.json`; `llvm-readobj` shows ELF32,
big-endian, `EF_MIPS_ABI2`, `EF_MIPS_ARCH_3`, entry `0x88040000`; 887 bytes of text.

- Add `nightly-2026-10-03` (or a justified single pin) with `rust-src` and
  `llvm-tools-preview`; reconcile `rust-toolchain.toml`.
- Add `targets/mips64-sgi-redox.json` (copy the field values in §3.4).
- Build a `no_std` hello object for it with `-Zbuild-std=core` and link with `rust-lld`.
- **Evidence:** the target JSON, the build command, `llvm-readobj` output showing ELF32, big
  endian, n32 flags, and the object size.

### T2 — Find Redox's real 32 MiB floor on x86_64, before touching MIPS

- Build `config/minimal` for `x86_64` and boot it in QEMU with `-m 32`.
- If it fails, find where: kernel image, initfs load, page cache, or a service. Record the
  first failure point and the sizes.
- Repeat at 48, 64 and 96 MiB to get the curve.
- **Evidence:** a table of memory size → first failure, plus kernel and initfs sizes. This is
  the cheapest experiment in the project and it tells you which strip item matters most.

### T3 — Make `redox` the TFTP boot file

**Done 2026-11-17.** `tools/ecoff` converts the ELF; `make indy` writes `build/indy/redox`;
`tools/indy-gate.sh` drives the PROM. Trace: `BOOTP asks for "redox"`, `RRQ "redox"
(octet)`, `complete: 975056 bytes, 1905 blocks, 0 retransmits`, PROM `entry: 0x88040000`.

- Build the ECOFF converter (or port the sibling's, as a standalone Rust tool).
- Add a make target that produces `redox` in a directory served by `iris --tftp-dir`.
- At the emulated PROM: `boot -f bootp()redox`.
- **Evidence:** `IRIS_TFTP_TRACE=1` output showing the read request, the transfer, and the
  PROM jumping to the image. A hang after the jump is success at this stage.

### T4 — A MIPS kernel `_start` that prints one line and stops

**Done 2026-11-17.** The patch `recipes/core/kernel/indy-mips64.patch` adds
`src/arch/mips64`. `kstart` builds `KernelArgs` itself (no bootloader; `hwdesc` null, one
memory area from `MEMCFG0/1`, no initfs). Serial line in `build/indy/gate/serial.log`:
`kernel::arch::mips64::start:INFO -- Redox OS starting...`. The run continues through
`startup::memory::init`, `init_mm`, percpu, `allocator::init` (the first TLB refills) and
stops at `Indy: no initfs in the image, stopping`, which is the gate marker.

- Adapt `KernelArgs` for the PROM handoff; no ACPI, no device tree.
- Serial output through the IOC's Z85C30, channel B (`0xBFBD_9833`; write the register number
  to the control port, then the data through the same port).
- **Evidence:** the line in `redox-serial.log`, from a 32 MiB guest, in CI.

### T5 — The first memory measurement

**Done 2026-11-17**, printed by the kernel on a 32 MiB guest:

| Item | Measured |
|---|---:|
| RAM reported by `MEMCFG0` (`0x6720_0000`) | 32768 KiB at `0x0800_0000` |
| Reserved below the kernel (PROM, vectors) | 256 KiB |
| Kernel image (text 896, data 56, bss 84) | 1036 KiB |
| Permanently used after `init_mm` (page tables, frame metadata) | 2052 KiB |
| Kernel heap, fixed | 1024 KiB |
| Free after boot | 28392 KiB (7098 frames) |

The kernel image line in §7.2 holds with 12 KiB to spare; the "heap and RMM structures"
line is 3 MiB measured against 1 MiB budgeted, mostly the per-frame `PageInfo` array and
the 1024 pre-allocated kernel directory tables `rmm` creates. That is the first line to cut.

- Print the frame allocator's start, the total RAM the PROM reported, the initfs region and
  the kernel image size at boot.
- Compare with §7.2 and correct the table.
- **Evidence:** the boot transcript and an updated budget table in this file.

Do not start on relibc, redoxfs or the shell until T4 passes in CI. Each of those is a project,
and none of them is on the critical path to "the machine runs our code".

### Next, in order

- **T6 — initfs, relibc and std.** Done 2026-11-17. `ecoff convert --append` places the initfs
  after the kernel; the `bootstrap` program forks the initfs scheme, the process manager and
  the namespace manager; relibc builds as a static `libc.a` (`tools/relibc-indy.sh`); Rust
  `std` builds with `-Zbuild-std` from a copy of rust-src whose libc is the fork's patched
  one; base's real `init` runs unit files from `config/indy/init.d`. The kernel is pinned to
  `7824a967`, the commit whose syscall crate relibc locks.
- **T7 — Seeq 8003 Ethernet.** Done in the emulator 2026-11-17: `seeqd`
  (`base/drivers/net/seeqd`, in the base patch) drives the Seeq through the HPC3 DMA
  engines, `netstack` and `dhcpd` run on it, and the gate marker is the DHCP lease:
  `REDOX-INDY-NET: mac 08-00-69-12-34-56 addr 192.168.0.2/24`. It polls on a 10 ms timer.
  Not done: the HPC3/INT3 interrupt path in the kernel (`arch::interrupt::irq` claims no
  IRQs), the IOC2 twisted-pair select, collision and error counters, and any run on real
  hardware.
- **T8 — interrupts, a shell, remote access.** Done in the emulator 2026-11-17. The kernel
  demultiplexes INT3 (local 0 on IP2, local 1 on IP3) into `irq:0..15`; `seeqd` takes
  `irq:3` and keeps a 250 ms timer only to restart a stopped ring. The console's receive
  side is polled from the timer tick. `tools/indy-sh` is a bring-up shell on the serial
  console and on TCP port 23 (`help`, `ls`, `cat`, `ps`, `net`, `uname`, `dmesg`, `reboot`,
  or a program from the initfs). `reboot` writes `reset` to `/scheme/sys/kstop`; the kernel
  jumps to the PROM. `make indy-gate` drives all of it: lease, a command over TCP, reboot,
  PROM menu.
- **Unattended boot.** With `SystemPartition=bootp()`, `OSLoader=redox` and `AutoLoad=Yes`
  in the PROM, power-on and every `reboot` netboot Redox without typing (two cycles proved
  on iris). The PROM refuses to autoload when its console cannot open
  (`NoAutoLoad=CONSOLE OPEN FAILED.`), which is the headless-emulator case; there
  `console=d` is also needed.
- **T8a — PROM console and cache faults.** 2026-11-17. The first boot on the real Indy hung
  after `entry: 0x88040000` with nothing on screen. The kernel now logs through the ARCS
  console (the Newport screen) and takes its memory map from the PROM's descriptors, which
  leaves 768 KiB to the PROM. iris hid a cache fault: gate with `IRIS_ARGS="--cache full"`
  as well. VCED/VCEI are handled in the exception entry. Cache aliasing on R4600/R5000,
  which do not trap, is open.
- **webfs.** 2026-11-17, in the emulator: `tools/webfs` mounts a read-only tree served
  over HTTP or HTTPS at `/scheme/web` and packs and serves it on the host
  ([docs/webfs.md](docs/webfs.md)). `make indy-webfs-gate` reads a 200 KiB file over TLS on
  a 32 MiB guest. The daemon is 1,067,900 bytes and enters the image only with
  `INDY_WEBFS_URL` set.
- **T8b — real hardware and a root filesystem.** 2026-11-18. Redox runs on the real Indy
  (R4600PC 133 MHz, PROM 5.3 Rev B7): kernel log and keyboard through the PROM console,
  TCP shell, `reboot` through ARCS `Reboot`, unattended netboot. The timer rate comes from
  the PROM's `cpufreq` and the first address from its `netaddr` (`NETADDR` in
  `/scheme/sys/env`). The root filesystem is a webfs store over HTTP mounted as scheme
  `file` (docs/webfs.md): the netboot image is 5.39 MB and holds only init, randd, seeqd,
  netstack and webfs; the shell, dhcpd and zerod load from the store. Programs spawn from
  `/usr/bin`. `poll()` works (upstream's event kcall is applied on the kernel pin) and
  signals are delivered through a real trampoline (the kernel preserves `$k1` for it);
  `sigtest` and `polltest` in the shell check both. Cache handling is by page colour since 2026-11-18
  (`arch/mips64/cache.rs`): frames are allocated on the colour of the page they back,
  borrowed mappings keep their source's colour, off-colour pages are written back at traps,
  and an assembly handler serves the refills that need no cache work. A kernel-scheme
  syscall takes about 12 to 20 microseconds and webfs reads about 90 KiB/s. A call to a
  user scheme still takes about 1.8 ms, nearly all of it in the kernel's scheme path.
  A debugger exists since 2026-11-18: `tools/indy-gdb.sh` attaches GDB to the running
  machine through `kdbg` on TCP port 2159 (read-only: every context is a thread, kernel
  stacks unwind with source lines, memory and statics can be read). Its first use found
  the timer stopping for a minute after a late tick.
  Open: that IPC cost, no ASIDs (the TLB is flushed on every switch), the machine does not
  answer ping, the store server is started by hand.
- **T8c — userland.** 2026-11-18, in the emulator. The root store holds 109 programs
  (126 MB): `ion`, 64 uutils programs, Redox coreutils (`ps`, `free`, `df`), `login`,
  `su`, `find`, `grep`, `sed`, `less`, `tar`, `gzip`, `kibi`, `ping`, `nc`, `dns`,
  `ifconfig`. `tools/indy-userland.sh` fetches each upstream at a pinned revision,
  applies `recipes/<group>/<name>/indy-mips64.patch`, builds and stages it
  (`make indy-userland`; about an hour from nothing). The console runs `getty` and
  `login` (users `root` and `user`, no password) with `ion` on a PTY; `/tmp`, `/root`,
  `/home` and `/var` are links into a RAM filesystem at `/scheme/tmp`. The bring-up
  shell is `indy-sh` on TCP port 23 only, and `kdbg` starts on demand
  (`tools/indy-gdb.sh`). Found on the way: relibc's `epoll_event` on 32-bit targets was
  12 bytes against 24 in the `libc` crate, which broke every `mio` user; the kernel woke
  console readers on every tick; an out-of-frames page fault panicked the kernel
  (`todo!("oom")`) and now fails the faulting process.
  Memory, first build: after login 3.1 MiB of 24.3 MiB was free and `ls` could exhaust it.
- **T8d — memory.** 2026-11-18, in the emulator: 12.3 MiB of 28.6 MiB is free after
  login (was 3.1 of 24.3). The initfs existed three times: the image's copy, reserved
  for ever; the copy the kernel makes for the first process; and the loaded programs.
  The kernel now gives the image's copy PageInfos and frees it after the copy
  (`memory::release_late_area`, 4.2 MiB). The initfs daemon unmaps its archive when
  `/scheme/initfs/image` is unlinked, which the root unit `20_initfs-release.service`
  does once the root is mounted (4.2 MiB; `/scheme/initfs` answers ENOENT afterwards,
  so nothing can be started from it again). The root mount runs two webfs workers with
  a 256 KiB blob cache (was four and 1 MiB). `ls -l /usr/bin | sort -k5 -n | tail -3`
  (8.5 MB of programs) runs.
- **T8e — the kernel's own console.** 2026-11-18. The kernel no longer calls the PROM
  for console input or output; ARCS is used for the memory map, environment variables
  and `Reboot` only. With `ConsoleOut` on the graphics board the kernel draws a 160 by
  64 text console on the Newport through REX3 (`arch/mips64/device/newport.rs`:
  characters by Z pattern, fills and screen-to-screen scroll, colour map indices 0 to 7
  as the PROM left them, a VT100/xterm subset with the cursor position report) and reads
  the IOC2 keyboard controller in scancode set 2 from the timer tick
  (`device/kbd.rs`; the mouse port is switched off). With a serial console it drives
  Serial Port 1 as before. In iris with the board enabled: login, `ls --color`, pipes,
  history and `kibi` work from the emulated keyboard. On the real Indy the kernel takes
  the graphics path and the machine boots to the TCP shell; what the screen shows and
  whether the keyboard acknowledged has not been read back yet.
  Later the same day the owner logged in at the real keyboard and ran commands, so the
  screen and the keyboard work on the hardware.
  Real hardware, same day: T8c and T8d run. 13.1 MiB of 28.5 MiB is free at the login
  prompt, and the TCP shell answers 160 s after `reboot`.
- **T8f — console interrupts.** 2026-11-18, in the emulator. The keyboard controller
  and the serial port interrupt through INT3's mappable lines 4 and 5, gathered by map
  mask 0 into local 0 bit 7, which the kernel serves itself (`interrupt/irq.rs`; it is
  no longer an `irq:` line). The Z85C30 interrupts on received characters; with the
  screen as the console its output is a 2 KiB queue sent by the transmit interrupt, so
  printing no longer waits 1 ms per character for the 9600 baud copy. The tick keeps a
  once-a-second look at both devices; `cachestat` in `indy-sh` prints the interrupt
  counts and how many keyboard bytes that fallback found (0 in iris). `ping` works:
  netutils built a `TimeSpec` in an unaligned byte array, which `libredox` asserts on
  where `time_t` needs 8-byte alignment.
- **T9 — next.** T8f on the real machine (`cachestat` after typing); a faster scroll
  (TOPSCAN). Then: each process is still a
  private static image read whole (no shared or demand-paged text), `zerod` runs twice,
  an entropy source for `randd`, a clock (`date` reads 1970), the serial interrupt
  (INT3 mappable lines) instead of polling.
- **T2** stays worth doing: it tells which userspace cut matters most before T6 pulls in
  packages.

---

## 12. Open decisions

Each needs an ADR before the code it governs. The sibling keeps its ADRs in `docs/adr/`; use
the same shape.

| # | Decision | Why it cannot be deferred |
|---|---|---|
| D1 | **Decided** ([ADR 0001](docs/adr/0001-toolchain-pin.md)): `nightly-2026-10-03` | `-Zbuild-std` requires `rust-src`; Redox's pin lacks `llvm-tools-preview` |
| D2 | relibc strategy: new MIPS PAL, or adopt the sibling's n32 syscall channel | it decides the kernel ABI and the libc's shape |
| D3 | **Decided** ([ADR 0002](docs/adr/0002-syscall-abi.md)): `$v0` number, `$a0`..`$a5` args, demuxed result in `$v0`, thread pointer in `$gp` | every syscall and the whole libc depend on it |
| D4 | RedoxFS byte order: endian-explicit accessors (recommended) vs a big-endian variant | it decides whether existing images are readable |
| D5 | Init source: initfs only, or initfs plus SCSI root | it decides the boot-time RAM floor |
| D6 | Graphics: serial only in v1, or REX3 from the start | 3.75 MiB vs 300 KiB, and weeks of work |
| D7 | **Decided**: off; the kernel builds with `--no-default-features` and `ipi` is a no-op | the Indy is single-CPU; SMP is pure cost |
| D8 | Package format and index for the first image | the sibling's index format exists but is not a Redox format |
| D4a | **Decided** ([ADR 0004](docs/adr/0004-byte-order.md)): native byte order across syscall and scheme boundaries; fix producers | every syscall with packed metadata depends on it |
| D9 | **Decided** ([ADR 0003](docs/adr/0003-paging-and-tlb.md)): two-level 4-byte tables, EntryLo-shaped leaves, one ASID, software refill | every mapping and the TLB handler depend on it |

---

## 13. Provenance and rebasing

- Forked from the GitHub mirror `redox-os/redox`, default branch `master`, at
  `f71658dfca2b978bee55a4c19d95ab926a948c21` (2026-11-16).
- **The canonical upstream is GitLab** (`gitlab.redox-os.org/redox-os/redox`). The GitHub
  repository is a mirror and may lag. Rebase onto the mirror; never merge GitLab history into
  this fork.
- Our changes are patch files next to the recipe that builds the component
  (`recipes/core/kernel/indy-mips64.patch`, listed in that `recipe.toml` under
  `[source] patches`), not a divergent copy of upstream source in this repository. This keeps
  a rebase mechanical. The kernel patch applies to `redox-os/kernel` `8f0e965e`.
- Every patch series records: the upstream commit it applies to, the reason, and the memory
  effect. A patch that cannot say why it exists is deleted at the next rebase.
- Upstream's `CONTRIBUTING.md` AI policy means these patches are not candidates for upstream
  merge. Do not spend time making them upstreamable.
- Record the rebase in `CHANGELOG.md` with the old and new base commits.

---

## 14. Appendix: paths and commands

### Paths on this workstation

| Path | What |
|---|---|
| `/mnt/europa/redox` | this fork |
| `/mnt/europa/os` | `irix7/os`: research, toolchain, targets, ROM, gates |
| `/mnt/europa/iris` | `irix7/iris`: the emulator |
| `/mnt/europa/os-sdk` | module ABI and SDK (not used by this port) |
| `/mnt/europa/rust-toolchain` | a downloaded toolchain plus `rust-src` |
| `/mnt/europa/sgi-toolchain-scratch` | GCC 15.3/16.2 and binutils patch work |
| `/tmp/opencode/relibc-pal-src` | default relibc checkout for the sibling's build script |

### Commands worth knowing

```sh
# The sibling's dev shell (Rust pin, rust-lld on PATH, cross tools)
cd /mnt/europa/os && nix develop

# Build a MIPS kernel and convert it to ECOFF (sibling, for reference)
cd /mnt/europa/os && tools/indy-netboot.sh /tmp/opencode/redox-netboot

# Build the relibc n32 probes (sibling, for reference)
export MIPS_CC=/path/to/mips64-buildroot-linux-uclibc-gcc
export MIPS_AR=/path/to/mips64-buildroot-linux-uclibc-ar
cd /mnt/europa/os && tools/relibc-pal/build.sh /tmp/opencode/relibc-pal-work/out

# Inspect what the PROM asked for in the emulator
IRIS_TFTP_TRACE=1 iris --config redox.toml --tftp-dir /tmp/opencode/redox-netboot

# This fork's build system documentation
#   https://doc.redox-os.org/book/build-system-reference.html
#   https://doc.redox-os.org/book/coding-and-building.html
```

### Glossary

| Term | Meaning here |
|---|---|
| n32 | the 32-bit MIPS ABI with 64-bit registers and 64-bit `off_t`/`time_t` |
| ECOFF | the executable format the Indy PROM loads |
| ARCS | the PROM's runtime services, alive until the kernel takes the TLB |
| initfs | the bootstrap filesystem handed to the kernel as a contiguous physical region; resident RAM |
| scheme | Redox's namespace-provider protocol; the closest analogue is a Plan 9 file server |
| iris | the SGI Indy/Indigo2 emulator used as the gate |
| cell / job | the sibling's process/container terms; Redox's equivalents are process and context |

---

*Last updated: 2026-11-17 (T1, T3, T4, T5 evidence from `make indy-gate` on iris). If you
change a fact in this file, change the date and say what proved it.*
