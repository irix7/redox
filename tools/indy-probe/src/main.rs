//! Boot probe for the SGI Indy: entered by the PROM after `boot -f bootp()redox`,
//! prints a marker on serial port 1 and stops.
//!
//! The PROM enters with the CPU in kernel mode, caches on, interrupts off and
//! ARCS alive. `_start` ignores the ARCS argument registers, switches to its
//! own stack, zeroes `.bss` and calls `probe_main`.
#![no_std]
#![feature(asm_experimental_arch)]
#![no_main]

use core::arch::global_asm;

mod scc;

global_asm!(
    r#"
    .set noreorder
    .set noat
    .section .text._start, "ax", @progbits
    .globl _start
    .ent _start
_start:
    la      $sp, __stack_top
    la      $t0, __bss_start
    la      $t1, __bss_end
1:
    sltu    $t2, $t0, $t1
    beqz    $t2, 2f
    nop
    sw      $zero, 0($t0)
    b       1b
    addiu   $t0, $t0, 4
2:
    jal     {main}
    nop
3:
    b       3b
    nop
    .end _start
    .set at
    .set reorder
    "#,
    main = sym probe_main,
);

/// Memory controller registers (kseg1), used only to report the RAM size.
const MC_MEMCFG0: *const u32 = 0xBFA0_00C4 as *const u32;
const MC_MEMCFG1: *const u32 = 0xBFA0_00CC as *const u32;
const MC_SYSID: *const u32 = 0xBFA0_001C as *const u32;

/// Sum the valid DRAM banks the PROM programmed into MEMCFG0/1.
///
/// Each 16-bit bank field has bit 13 valid, bits 12..8 size in units minus one
/// and bits 7..0 base in units. The unit is 4 MiB before MC revision 5 and
/// 16 MiB from revision 5.
fn ram_bytes() -> u32 {
    let (sysid, cfg0, cfg1) = unsafe {
        (
            core::ptr::read_volatile(MC_SYSID),
            core::ptr::read_volatile(MC_MEMCFG0),
            core::ptr::read_volatile(MC_MEMCFG1),
        )
    };
    let shift = if sysid & 0x0f >= 5 { 24 } else { 22 };
    let mut total = 0u32;
    for field in [cfg0 >> 16, cfg0 & 0xffff, cfg1 >> 16, cfg1 & 0xffff] {
        if field & 0x2000 != 0 {
            total += (((field >> 8) & 0x1f) + 1) << shift;
        }
    }
    total
}

#[unsafe(no_mangle)]
extern "C" fn probe_main() -> ! {
    scc::init();
    scc::write_str("REDOX-PROBE: hello from the Indy PROM path\r\n");
    scc::write_str("REDOX-PROBE: ram=");
    scc::write_hex(ram_bytes());
    scc::write_str(" memcfg0=");
    scc::write_hex(unsafe { core::ptr::read_volatile(MC_MEMCFG0) });
    scc::write_str(" memcfg1=");
    scc::write_hex(unsafe { core::ptr::read_volatile(MC_MEMCFG1) });
    scc::write_str("\r\nREDOX-PROBE-OK\r\n");
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    scc::write_str("REDOX-PROBE: panic\r\n");
    loop {}
}
