//! The first `std` program on the Indy. The bootstrap execs it as
//! `/scheme/initfs/bin/init`; it prints a marker and idles.

use std::io::Write;

fn main() {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "REDOX-INDY-INIT: hello from std on the Indy");
    let v: Vec<u32> = (1..=10).collect();
    let _ = writeln!(out, "REDOX-INDY-INIT: sum 1..10 = {}", v.iter().sum::<u32>());
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let _ = writeln!(out, "REDOX-INDY-INIT: alive");
    }
}
