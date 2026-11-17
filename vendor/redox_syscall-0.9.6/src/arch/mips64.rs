//! MIPS n32 (SGI Indy) system call ABI.
//!
//! Number in `$v0`, arguments in `$a0`..`$a5`, the demultiplexed result in
//! `$v0`. The `syscall` instruction traps; the kernel skips it on return.
//! The thread pointer lives in `$gp` (`$28`): the kernel and libc are static
//! and use no GOT, so the register is free for TLS, which is the layout the
//! n32 libc work on this machine already uses.

use core::{
    mem,
    ops::{Deref, DerefMut},
    slice,
};

#[cfg(feature = "userspace")]
use core::arch::asm;

use super::error::{Error, Result};

pub const PAGE_SIZE: usize = 4096;
/// Size of the metadata region used to transfer information from the kernel to the bootstrapper.
pub const KERNEL_METADATA_SIZE: usize = 4 * PAGE_SIZE;

#[cfg(feature = "userspace")]
macro_rules! syscall {
    ($($name:ident($a:ident, $($b:ident, $($c:ident, $($d:ident, $($e:ident, $($f:ident, $($g:ident, )?)?)?)?)?)?);)+) => {
        $(
            pub unsafe fn $name(mut $a: usize, $($b: usize, $($c: usize, $($d: usize, $($e: usize, $($f: usize, $($g: usize)?)?)?)?)?)?) -> Result<usize> {
                asm!(
                    "syscall",
                    inout("$2") $a,
                    $(
                        in("$4") $b,
                        $(
                            in("$5") $c,
                            $(
                                in("$6") $d,
                                $(
                                    in("$7") $e,
                                    $(
                                        in("$8") $f,
                                        $(
                                            in("$9") $g,
                                        )?
                                    )?
                                )?
                            )?
                        )?
                    )?
                    lateout("$3") _,
                    options(nostack),
                );

                Error::demux($a)
            }
        )+
    };
}

#[cfg(feature = "userspace")]
syscall! {
    syscall0(a,);
    syscall1(a, b,);
    syscall2(a, b, c,);
    syscall3(a, b, c, d,);
    syscall4(a, b, c, d, e,);
    syscall5(a, b, c, d, e, f,);
    syscall6(a, b, c, d, e, f, g,);
}

/// General registers as the `proc:` scheme reads and writes them. Register
/// values are 64 bits wide on the R4400; this view keeps the low 32 bits.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct IntRegisters {
    pub pc: usize,
    pub at: usize,
    pub v0: usize,
    pub v1: usize,
    pub a0: usize,
    pub a1: usize,
    pub a2: usize,
    pub a3: usize,
    pub a4: usize,
    pub a5: usize,
    pub a6: usize,
    pub a7: usize,
    pub t0: usize,
    pub t1: usize,
    pub t2: usize,
    pub t3: usize,
    pub s0: usize,
    pub s1: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub t8: usize,
    pub t9: usize,
    // gp ($28) is the thread pointer and lives in EnvRegisters
    pub sp: usize,
    pub fp: usize,
    pub ra: usize,
    pub hi: usize,
    pub lo: usize,
}

impl Deref for IntRegisters {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                self as *const IntRegisters as *const u8,
                mem::size_of::<IntRegisters>(),
            )
        }
    }
}

impl DerefMut for IntRegisters {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(
                self as *mut IntRegisters as *mut u8,
                mem::size_of::<IntRegisters>(),
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C, packed)]
pub struct FloatRegisters {
    pub fregs: [u64; 32],
    pub fcsr: u32,
}

impl Deref for FloatRegisters {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                self as *const FloatRegisters as *const u8,
                mem::size_of::<FloatRegisters>(),
            )
        }
    }
}

impl DerefMut for FloatRegisters {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(
                self as *mut FloatRegisters as *mut u8,
                mem::size_of::<FloatRegisters>(),
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(packed)]
pub struct EnvRegisters {
    /// Thread pointer: `$gp` (`$28`).
    pub tp: usize,
}

impl Deref for EnvRegisters {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                self as *const EnvRegisters as *const u8,
                mem::size_of::<EnvRegisters>(),
            )
        }
    }
}

impl DerefMut for EnvRegisters {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(
                self as *mut EnvRegisters as *mut u8,
                mem::size_of::<EnvRegisters>(),
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C, packed)]
pub struct Exception {
    /// CP0 Cause ExcCode.
    pub code: usize,
    /// CP0 BadVAddr, or 0 when the exception has none.
    pub address: usize,
}

impl Exception {
    pub fn pack(&self) -> (usize, usize) {
        (self.code, self.address)
    }
    pub fn unpack(code: usize, address: usize) -> Self {
        Exception { code, address }
    }
}

impl Deref for Exception {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                self as *const Exception as *const u8,
                mem::size_of::<Exception>(),
            )
        }
    }
}

impl DerefMut for Exception {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(
                self as *mut Exception as *mut u8,
                mem::size_of::<Exception>(),
            )
        }
    }
}
