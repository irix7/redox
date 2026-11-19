//! Compiled benchmark for the R4600 model. The same source is built once
//! for mips3 and once with -C target-cpu=r4600. Each kernel prints a
//! checksum, so the two builds can be checked for the same results.

use std::hint::black_box;
use std::time::Instant;

const N: u64 = 4_000_000;

/// Integer multiply and divide chain: mult, mflo, and a dependent divide.
fn int_chain(seed: u64) -> u64 {
    let mut x = black_box(seed) | 1;
    let mut acc: u64 = 0;
    for i in 0..N {
        let m = (x as u32).wrapping_mul((i as u32) | 3);
        acc = acc.wrapping_add(m as u64);
        x = x.wrapping_add(acc | 1);
        let d = (acc as u32 | 1) / ((x as u32 & 0xff) | 3);
        acc ^= d as u64;
    }
    acc
}

/// Floating-point chain: multiply and add on changing values.
fn fp_chain(seed: f32) -> f32 {
    let mut a = black_box(seed);
    let mut b = 1.000_1f32;
    for _ in 0..N {
        a = a * b + 0.5;
        b = b * 0.999_999 + 1.0e-7;
    }
    a
}

fn main() {
    let t = Instant::now();
    let c1 = int_chain(12345);
    let int_ns = t.elapsed().as_nanos();
    let t = Instant::now();
    let c2 = fp_chain(0.25);
    let fp_ns = t.elapsed().as_nanos();
    println!("int checksum {c1:#018x} total_ns {int_ns}");
    println!("fp checksum {:#010x} total_ns {fp_ns}", c2.to_bits());
}
