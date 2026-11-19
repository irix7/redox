//! Targeted MFLO benchmark. Each multiply's result feeds the next multiply
//! at once, so the mflo is followed directly by a mult. The mips3 build pads
//! that with a nop, and the r4600 build does not.

use std::hint::black_box;
use std::time::Instant;

const N: u64 = 20_000_000;

fn chain(seed: u32) -> u32 {
    let mut x = black_box(seed) | 1;
    let k = black_box(0x9e37_79b1u32) | 1;
    for _ in 0..N {
        x = x.wrapping_mul(k).wrapping_mul(k);
    }
    x
}

fn main() {
    let t = Instant::now();
    let c = chain(0x1234_5678);
    let ns = t.elapsed().as_nanos();
    println!("mflo chain checksum {c:#010x} total_ns {ns}");
}
