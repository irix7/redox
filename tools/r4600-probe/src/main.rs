#![feature(asm_experimental_arch)]
//! Latency probe for the R4600 on the Indy.
//!
//! Each chain is a loop of dependent instructions, written in assembly so
//! the registers are fixed. Every chain's operands stay at 1 (or 0.0 and
//! 1.0 for floats), so the values do not overflow. A chain of ALU adds has
//! a 1-cycle latency (manual Table 1.23), which calibrates the clock.

use std::time::Instant;

std::arch::global_asm!(
    ".set noreorder",
    ".set nomacro",
    ".global probe_fpregs",
    "probe_fpregs:",
    "ldc1 $f0, 0($4)",
    "ldc1 $f1, 8($4)",
    "ldc1 $f2, 16($4)",
    "ldc1 $f3, 24($4)",
    "ldc1 $f4, 32($4)",
    "ldc1 $f5, 40($4)",
    "sdc1 $f0, 0($5)",
    "sdc1 $f1, 8($5)",
    "sdc1 $f2, 16($5)",
    "sdc1 $f3, 24($5)",
    "sdc1 $f4, 32($5)",
    "sdc1 $f5, 40($5)",
    "jr $31",
    "nop",
);
std::arch::global_asm!(
    ".set noreorder",
    ".set nomacro",
    ".set noat",
    ".global probe_alu",
    "probe_alu:",
    "li $8, 1",
    "1:",
    ".rept 8",
    "addu $8, $8, $9",
    ".endr",
    "addiu $4, $4, -1",
    "bnez $4, 1b",
    "nop",
    "move $2, $4",
    "jr $31",
    "nop",
    ".global probe_mult",
    "probe_mult:",
    "li $8, 1",
    "li $9, 1",
    "2:",
    ".rept 4",
    "mult $8, $9",
    "mflo $8",
    ".endr",
    "addiu $4, $4, -1",
    "bnez $4, 2b",
    "nop",
    "move $2, $4",
    "jr $31",
    "nop",
    ".global probe_div",
    "probe_div:",
    "li $8, 1",
    "li $9, 1",
    "3:",
    ".rept 2",
    "div $zero, $8, $9",
    "mflo $8",
    ".endr",
    "addiu $4, $4, -1",
    "bnez $4, 3b",
    "nop",
    "move $2, $4",
    "jr $31",
    "nop",
    ".global probe_add_s",
    "probe_add_s:",
    "mtc1 $zero, $f0",
    "li $8, 1",
    "mtc1 $8, $f2",
    "cvt.s.w $f2, $f2",
    "4:",
    ".rept 8",
    "add.s $f0, $f0, $f2",
    ".endr",
    "addiu $4, $4, -1",
    "bnez $4, 4b",
    "nop",
    "move $2, $4",
    "jr $31",
    "nop",
    ".global probe_mul_s",
    "probe_mul_s:",
    "li $8, 1",
    "mtc1 $8, $f0",
    "cvt.s.w $f0, $f0",
    "mov.s $f2, $f0",
    "5:",
    ".rept 8",
    "mul.s $f0, $f0, $f2",
    ".endr",
    "addiu $4, $4, -1",
    "bnez $4, 5b",
    "nop",
    "move $2, $4",
    "jr $31",
    "nop",
);

unsafe extern "C" {
    fn probe_fpregs(src: *const u64, dst: *mut u64);
    fn probe_alu(n: u64) -> u64;
    fn probe_mult(n: u64) -> u64;
    fn probe_div(n: u64) -> u64;
    fn probe_add_s(n: u64) -> u64;
    fn probe_mul_s(n: u64) -> u64;
}

const ITER: u64 = 2_000_000;

// Each chain is called directly. Passing the functions through a fn pointer
// made the code load the target address from memory, so the loop never ran.
fn time_alu() -> (f64, u64) {
    let start = Instant::now();
    let left = unsafe { probe_alu(ITER) };
    (start.elapsed().as_nanos() as f64 / ITER as f64, left)
}

fn time_mult() -> f64 {
    let start = Instant::now();
    unsafe { probe_mult(ITER) };
    start.elapsed().as_nanos() as f64 / ITER as f64
}

fn time_div() -> f64 {
    let start = Instant::now();
    unsafe { probe_div(ITER) };
    start.elapsed().as_nanos() as f64 / ITER as f64
}

fn time_add_s() -> f64 {
    let start = Instant::now();
    unsafe { probe_add_s(ITER) };
    start.elapsed().as_nanos() as f64 / ITER as f64
}

fn time_mul_s() -> f64 {
    let start = Instant::now();
    unsafe { probe_mul_s(ITER) };
    start.elapsed().as_nanos() as f64 / ITER as f64
}

fn main() {
    // Clock check: a one-second sleep, in integer nanoseconds.
    let t0 = Instant::now();
    std::thread::sleep(std::time::Duration::from_secs(1));
    println!("clock check ns {}", t0.elapsed().as_nanos());

    // Raw totals: nanoseconds for each chain and the counter left after it.
    let run = |name: &str, f: unsafe extern "C" fn(u64) -> u64, n: u64| {
        let t = Instant::now();
        let left = unsafe { f(n) };
        println!("{name} iter {n} total_ns {} left {left}", t.elapsed().as_nanos());
    };
    run("alu", probe_alu, ITER);
    run("mult", probe_mult, ITER);
    run("div", probe_div, ITER);
    run("add_s", probe_add_s, ITER);
    run("mul_s", probe_mul_s, ITER);
}
