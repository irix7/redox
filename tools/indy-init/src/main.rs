//! Console reporter for the Indy bring-up image. init starts it as
//! `netinfo`; it prints a marker, then the interface address every two
//! seconds once the network stack has one.

use std::io::Write;
use std::time::Duration;

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn main() {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "REDOX-INDY-INIT: hello from std on the Indy");
    let mut reported = String::new();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let mac = read("/scheme/netcfg/ifaces/eth0/mac").unwrap_or_default();
        let addr = read("/scheme/netcfg/ifaces/eth0/addr/list").unwrap_or_default();
        let line = format!("mac {mac} addr {}", addr.replace('\n', " "));
        if line != reported {
            let _ = writeln!(out, "REDOX-INDY-NET: {line}");
            reported = line;
        }
    }
}
