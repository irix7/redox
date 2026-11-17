# redox_syscall 0.9.6 for the Indy port

Copy of `syscall/` from redox-os/kernel commit 7824a967b630d4a81c0dbe75a3e50a2a3657cacd
(the version relibc 08c786e1 locks), plus `src/arch/mips64.rs` and the
`mips64` arch selection in `src/lib.rs`. It is committed whole because its
source is a git checkout, not a registry crate; `vendor/patches` covers the
registry crates. The kernel itself builds its own `syscall/` (0.10.0) from the
kernel patch; the two agree on every syscall number the port uses.
