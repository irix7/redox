# ADR 0004: Byte order between kernel, libc and schemes

Status: accepted, 2026-11-17. First part of D4 in bootstrap.md.

## Decision

Values that cross the syscall and scheme boundaries on one machine are in
native byte order. The Indy is big-endian, so code that assumed
little-endian is fixed where the value is produced or declared, and the
consumers stay unchanged.

Fixed so far:

| Where | What was wrong | Fix |
|---|---|---|
| `syscall::StdFsCallMeta` | `kind: u8` first, read by the kernel and by `redox-scheme` as `word0 as u8`; on big-endian that byte is reserved padding, so every `fstat`, `getdents`, rename, link and unlink returned EOPNOTSUPP | on big-endian the seven reserved bytes come first, so word 0 equals the kind as a `u64` |
| bootstrap `initnsmgr` | numbers decoded with `from_le_bytes`, encoded by redox-rt with `to_ne_bytes`; dup kind 2 arrived as `0x02000000` and scheme registration failed | decode with `from_ne_bytes` |
| kernel trap frame | 32-bit values written zero-extended into 64-bit registers | sign-extend (ADR 0002) |

Explicitly little-endian and left alone: the initfs image (`redox-initfs`
wraps every integer in a little-endian type) and the bootstrap entry the
kernel reads from it.

## Related: alignment

MIPS traps on unaligned loads. relibc's `readdir` returned a pointer into a
packed record buffer; `dirent` starts with a 64-bit member, and `std`'s
`ReadDir` faulted with an address error. `readdir` now copies the record
into an aligned buffer owned by the `DIR`. The kernel does not emulate
unaligned access; code that needs it must use unaligned reads.

## Still open

On-disk formats (RedoxFS, pkgar): not touched yet; the rule in bootstrap.md
§6.4 stands (explicit little-endian accessors, one format).
