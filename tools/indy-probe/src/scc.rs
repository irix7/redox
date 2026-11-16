//! Polled output on the Z85C30 ESCC in the IOC2, channel B.
//!
//! Channel B is the rear-panel "Serial Port 1" and the PROM's `serial0`.
//! Registers are reached through byte lane 3: write the register number to
//! the control port, then the value to the same port. The data port is the
//! next word.

use core::ptr::{read_volatile, write_volatile};

const CHANNEL_B_CONTROL: *mut u8 = 0xBFBD_9833 as *mut u8;
const CHANNEL_B_DATA: *mut u8 = 0xBFBD_9837 as *mut u8;

/// RR0 bit 2: transmit buffer empty.
const RR0_TX_EMPTY: u8 = 0x04;

/// 9600 baud from the 3.672 MHz PCLK with the x16 clock mode:
/// baud = PCLK / (2 * 16 * (tc + 2)), so tc = 10 gives 9562.
const BAUD_TIME_CONSTANT: u8 = 10;

fn wr(reg: u8, value: u8) {
    unsafe {
        write_volatile(CHANNEL_B_CONTROL, reg);
        write_volatile(CHANNEL_B_CONTROL, value);
    }
}

fn rr0() -> u8 {
    unsafe { read_volatile(CHANNEL_B_CONTROL) }
}

/// Program channel B for 9600 8N1, interrupts off.
pub fn init() {
    wr(9, 0x40); // channel reset B
    wr(4, 0x44); // x16 clock, 1 stop bit, no parity
    wr(3, 0xc0); // rx 8 bits per character, receiver off
    wr(5, 0x60); // tx 8 bits per character, transmitter off
    wr(9, 0x00); // no interrupts
    wr(11, 0x50); // tx and rx clocks from the baud rate generator
    wr(12, BAUD_TIME_CONSTANT);
    wr(13, 0x00);
    wr(14, 0x02); // baud rate generator source PCLK
    wr(14, 0x03); // baud rate generator enabled
    wr(3, 0xc1); // receiver on
    wr(5, 0xea); // DTR, tx 8 bits, transmitter on, RTS
}

pub fn write_byte(byte: u8) {
    while rr0() & RR0_TX_EMPTY == 0 {
        core::hint::spin_loop();
    }
    unsafe { write_volatile(CHANNEL_B_DATA, byte) }
}

pub fn write_str(s: &str) {
    for byte in s.bytes() {
        write_byte(byte);
    }
}

pub fn write_hex(value: u32) {
    write_str("0x");
    for shift in (0..8).rev() {
        let nibble = ((value >> (shift * 4)) & 0xf) as u8;
        write_byte(if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 });
    }
}
