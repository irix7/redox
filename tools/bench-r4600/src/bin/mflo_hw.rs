#![feature(asm_experimental_arch)]
//! Hardware check for the MFLO padding change. Both loops compute the same
//! chain of 32-bit multiplies. The direct loop puts mflo straight before the
//! next mult, which is the case the r4600 model drops the padding for. The
//! padded loop puts two nops there. A wrong result in the direct loop, or a
//! difference from the padded one, means the R4600 does not interlock HI/LO
//! here. The expected value comes from Python (see the header of the output).

std::arch::global_asm!(
    ".set noreorder",
    ".set nomacro",
    ".set noat",
    ".global mflo_direct",
    "mflo_direct:",          // a0 = x, a1 = k, a2 = iterations
    "sll $8, $4, 0",
    "sll $10, $5, 0",
    "1:",
    "mult $8, $10",
    "mflo $8",
    "mult $8, $10",
    "mflo $8",
    "addiu $6, $6, -1",
    "bnez $6, 1b",
    "nop",
    "sll $2, $8, 0",
    "jr $31",
    "nop",
    ".global mflo_padded",
    "mflo_padded:",
    "sll $8, $4, 0",
    "sll $10, $5, 0",
    "2:",
    "mult $8, $10",
    "mflo $8",
    "nop",
    "nop",
    "mult $8, $10",
    "mflo $8",
    "nop",
    "nop",
    "addiu $6, $6, -1",
    "bnez $6, 2b",
    "nop",
    "sll $2, $8, 0",
    "jr $31",
    "nop",
);

unsafe extern "C" {
    fn mflo_direct(x: u64, k: u64, n: u64) -> u64;
    fn mflo_padded(x: u64, k: u64, n: u64) -> u64;
}

const X0: u64 = 0x1234_5679;
const K: u64 = 0x9e37_79b1;
const N: u64 = 60_000_000;

fn main() {
    let t = std::time::Instant::now();
    let d = unsafe { mflo_direct(X0, K, N) } as u32;
    let d_ns = t.elapsed().as_nanos();
    let t = std::time::Instant::now();
    let p = unsafe { mflo_padded(X0, K, N) } as u32;
    let p_ns = t.elapsed().as_nanos();
    println!("direct result {d:#010x} total_ns {d_ns}");
    println!("padded result {p:#010x} total_ns {p_ns}");
}
