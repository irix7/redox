//! Bring-up shell for the Indy.
//!
//! One command interpreter serves the serial console (stdin/stdout, the
//! kernel's debug scheme) and every TCP connection to port 23, so the machine
//! can be driven without a serial cable. A reporter prints the interface
//! address on the console when it changes.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;
use std::time::Duration;

const TCP_PORT: u16 = 23;
const BIN_DIR: &str = "/scheme/initfs/bin";

fn read_trim(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn net_line() -> String {
    let mac = read_trim("/scheme/netcfg/ifaces/eth0/mac").unwrap_or_default();
    let addr = read_trim("/scheme/netcfg/ifaces/eth0/addr/list").unwrap_or_default();
    format!("mac {mac} addr {}", addr.replace('\n', " "))
}

fn help(out: &mut dyn Write) {
    let _ = writeln!(
        out,
        "commands:\r\n  help               this list\r\n  echo TEXT          print TEXT\r\n  ls [DIR]           list a directory (default {BIN_DIR})\r\n  cat FILE           print a file\r\n  ps                 contexts (/scheme/sys/context)\r\n  mem                memory (/scheme/sys/memory... if present)\r\n  net                interface address\r\n  uname              kernel identification\r\n  dmesg              kernel log\r\n  reboot             reset to the PROM\r\n  PROGRAM [ARGS]     run {BIN_DIR}/PROGRAM\r"
    );
}

fn cat(out: &mut dyn Write, path: &str) {
    match fs::read(path) {
        Ok(data) => {
            let text = String::from_utf8_lossy(&data).replace('\n', "\r\n");
            let _ = write!(out, "{text}");
            if !text.ends_with("\r\n") {
                let _ = write!(out, "\r\n");
            }
        }
        Err(err) => {
            let _ = writeln!(out, "cat: {path}: {err}\r");
        }
    }
}

/// Run one command line. Returns false when the session should end.
fn run(line: &str, out: &mut dyn Write) -> bool {
    let mut words = line.split_whitespace();
    let Some(cmd) = words.next() else { return true };
    let args: Vec<&str> = words.collect();
    match cmd {
        "help" | "?" => help(out),
        "echo" => {
            let _ = writeln!(out, "{}\r", args.join(" "));
        }
        "ls" => {
            let dir = args.first().copied().unwrap_or(BIN_DIR);
            match fs::read_dir(dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let _ = writeln!(out, "{}\r", entry.file_name().to_string_lossy());
                    }
                }
                Err(err) => {
                    let _ = writeln!(out, "ls: {dir}: {err}\r");
                }
            }
        }
        "cat" => {
            for path in &args {
                cat(out, path);
            }
        }
        "ps" => cat(out, "/scheme/sys/context"),
        "mem" => cat(out, "/scheme/sys/stat"),
        "uname" => cat(out, "/scheme/sys/uname"),
        "dmesg" => cat(out, "/scheme/sys/log"),
        "net" => {
            let _ = writeln!(out, "{}\r", net_line());
        }
        "reboot" => {
            let _ = writeln!(out, "rebooting to the PROM\r");
            let _ = out.flush();
            thread::sleep(Duration::from_millis(200));
            if let Err(err) = fs::write("/scheme/sys/kstop", "reset") {
                let _ = writeln!(out, "reboot: {err}\r");
            }
        }
        "exit" | "quit" => return false,
        program => {
            let path = if program.contains('/') {
                program.to_string()
            } else {
                format!("{BIN_DIR}/{program}")
            };
            match Command::new(&path).args(&args).output() {
                Ok(result) => {
                    let text = String::from_utf8_lossy(&result.stdout).replace('\n', "\r\n");
                    let _ = write!(out, "{text}");
                    let text = String::from_utf8_lossy(&result.stderr).replace('\n', "\r\n");
                    let _ = write!(out, "{text}");
                    if !result.status.success() {
                        let _ = writeln!(out, "{program}: {}\r", result.status);
                    }
                }
                Err(err) => {
                    let _ = writeln!(out, "{program}: {err}\r");
                }
            }
        }
    }
    true
}

/// Read lines from `input`, run them, write results to `output`.
fn session(input: &mut dyn Read, output: &mut dyn Write, echo: bool) {
    let _ = write!(output, "\r\nRedox on the SGI Indy. Type help.\r\nindy# ");
    let _ = output.flush();
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut last = 0u8;
    loop {
        match input.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let previous = last;
        last = byte[0];
        match byte[0] {
            // The second half of a CR LF pair.
            b'\n' if previous == b'\r' => {}
            b'\r' | b'\n' => {
                if echo {
                    let _ = write!(output, "\r\n");
                }
                let text = String::from_utf8_lossy(&line).to_string();
                line.clear();
                if !run(&text, output) {
                    return;
                }
                let _ = write!(output, "indy# ");
            }
            0x08 | 0x7f => {
                if line.pop().is_some() && echo {
                    let _ = write!(output, "\x08 \x08");
                }
            }
            c if c >= 0x20 => {
                line.push(c);
                if echo {
                    let _ = output.write_all(&[c]);
                }
            }
            _ => {}
        }
        let _ = output.flush();
    }
}

fn serve(stream: TcpStream) {
    let Ok(mut reader) = stream.try_clone() else { return };
    let mut writer = stream;
    session(&mut reader, &mut writer, false);
}

fn tcp_server() {
    // The network stack may not be up yet: keep trying to bind.
    let listener = loop {
        match TcpListener::bind(("0.0.0.0", TCP_PORT)) {
            Ok(listener) => break listener,
            Err(_) => thread::sleep(Duration::from_secs(2)),
        }
    };
    println!("REDOX-INDY-SH: listening on tcp port {TCP_PORT}\r");
    for stream in listener.incoming().flatten() {
        thread::spawn(move || serve(stream));
    }
}

fn reporter() {
    let mut reported = String::new();
    loop {
        thread::sleep(Duration::from_secs(2));
        let line = net_line();
        if line != reported {
            println!("REDOX-INDY-NET: {line}\r");
            reported = line;
        }
    }
}

fn main() {
    println!("REDOX-INDY-SH: hello from std on the Indy\r");
    thread::spawn(reporter);
    thread::spawn(tcp_server);
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    loop {
        session(&mut stdin, &mut stdout, true);
        // Console input ended or failed: wait and offer the prompt again.
        thread::sleep(Duration::from_secs(5));
    }
}
