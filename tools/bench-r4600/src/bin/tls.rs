//! TLS access benchmark. Each thread-local access reads the thread pointer
//! through rdhwr, which the R4600 does not implement and the kernel
//! emulates. Compare with a plain global counter.

use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

thread_local! {
    static COUNTER: Cell<u64> = const { Cell::new(0) };
}

const N: u64 = 2_000_000;

fn main() {
    let t = Instant::now();
    for i in 0..N {
        COUNTER.with(|c| c.set(c.get().wrapping_add(black_box(i) | 1)));
    }
    let tls_ns = t.elapsed().as_nanos();
    let tls_total = COUNTER.with(|c| c.get());

    let mut plain: u64 = 0;
    let t = Instant::now();
    for i in 0..N {
        plain = plain.wrapping_add(black_box(i) | 1);
    }
    let plain_ns = t.elapsed().as_nanos();
    println!("tls total {tls_total:#018x} total_ns {tls_ns}");
    println!("plain total {:#018x} total_ns {plain_ns}", black_box(plain));
}
