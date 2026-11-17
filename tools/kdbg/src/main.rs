//! kdbg: a GDB remote protocol server for looking inside the running Indy.
//!
//! It listens on TCP port 2159 and answers GDB from two kernel files,
//! `/scheme/sys/kdbg_ctx` (every context with its saved registers) and
//! `/scheme/sys/kdbg_req` + `kdbg_mem` (memory through a context's page
//! table). On the build host:
//!
//! ```sh
//! tools/indy-gdb.sh            # kernel symbols, target remote 10.1.1.112:2159
//! (gdb) info threads
//! (gdb) thread 5
//! (gdb) bt
//! ```
//!
//! Each context is a GDB thread showing where it sits in the kernel: the
//! registers `switch_to` saved, with the PC at its return address, so `bt`
//! unwinds the kernel stack. A context that has a trap frame also appears as
//! a second thread, numbered 1000 higher, with its user-mode registers; load
//! the program's symbols with `add-symbol-file` to unwind those.
//!
//! This is inspection only. The kernel and every program keep running while
//! GDB looks, nothing can be stopped, stepped or written, and a context that
//! is running at the moment of a read shows stale kernel registers.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

const PORT: u16 = 2159;
/// GDB thread numbers for the user-mode view of a context start here.
const USER_BASE: usize = 1000;
/// Bytes of memory per kernel request.
const MEM_CHUNK: usize = 4096;

/// GDB's register order for 64-bit MIPS: r0..r31, status, lo, hi, badvaddr,
/// cause, pc.
const REGISTERS: usize = 38;

/// One line of `/scheme/sys/kdbg_ctx`.
struct Context {
    id: usize,
    pid: usize,
    running: bool,
    table: usize,
    /// sp, ra, fp, gp, s0..s7, status.
    kernel: [u64; 13],
    /// `$1`..`$31`, pc, status, cause, badvaddr, hi, lo.
    user: Option<[u64; 37]>,
    name: String,
}

fn contexts() -> Vec<Context> {
    let text = fs::read_to_string("/scheme/sys/kdbg_ctx").unwrap_or_default();
    text.lines().filter_map(parse_context).collect()
}

fn parse_context(line: &str) -> Option<Context> {
    let mut fields = line.split(' ');
    let mut number = || u64::from_str_radix(fields.next()?, 16).ok();
    let id = number()? as usize;
    let pid = number()? as usize;
    let running = number()? != 0;
    let table = number()? as usize;
    let mut kernel = [0u64; 13];
    for slot in &mut kernel {
        *slot = number()?;
    }
    let user = if number()? != 0 {
        let mut regs = [0u64; 37];
        for slot in &mut regs {
            *slot = number()?;
        }
        Some(regs)
    } else {
        None
    };
    let name = fields.collect::<Vec<_>>().join(" ");
    Some(Context { id, pid, running, table, kernel, user, name })
}

/// The register block GDB reads for a thread.
fn registers(context: &Context, user_view: bool) -> [u64; REGISTERS] {
    let mut r = [0u64; REGISTERS];
    match (user_view, &context.user) {
        (true, Some(user)) => {
            r[1..32].copy_from_slice(&user[..31]);
            r[37] = user[31]; // pc
            r[32] = user[32]; // status
            r[36] = user[33]; // cause
            r[35] = user[34]; // badvaddr
            r[34] = user[35]; // hi
            r[33] = user[36]; // lo
        }
        _ => {
            let k = &context.kernel;
            r[29] = k[0]; // sp
            r[31] = k[1]; // ra
            r[30] = k[2]; // fp
            r[28] = k[3]; // gp
            r[16..24].copy_from_slice(&k[4..12]); // s0..s7
            r[32] = k[12]; // status
            // switch_to returns through ra: that is where the context is.
            r[37] = k[1];
        }
    }
    r
}

/// Read memory through the kernel. `table` selects a context's address
/// space for user addresses; 0 is the kernel's own view.
fn read_memory(table: usize, addr: usize, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let want = (len - out.len()).min(MEM_CHUNK);
        let request = format!("{:x} {:x} {:x}", table, addr + out.len(), want);
        if fs::write("/scheme/sys/kdbg_req", request).is_err() {
            break;
        }
        match fs::read("/scheme/sys/kdbg_mem") {
            Ok(bytes) if !bytes.is_empty() => {
                let short = bytes.len() < want;
                out.extend_from_slice(&bytes);
                if short {
                    break;
                }
            }
            _ => break,
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// One GDB connection.
struct Session {
    stream: TcpStream,
    contexts: Vec<Context>,
    /// The thread `g`, `p` and `m` refer to, as GDB numbers it.
    selected: usize,
}

impl Session {
    /// GDB thread numbers: context `i` is `i + 1` (kernel view) and
    /// `USER_BASE + i + 1` (user view).
    fn thread_ids(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = (1..=self.contexts.len()).collect();
        for (i, context) in self.contexts.iter().enumerate() {
            if context.user.is_some() {
                ids.push(USER_BASE + i + 1);
            }
        }
        ids
    }

    fn lookup(&self, thread: usize) -> Option<(&Context, bool)> {
        let user_view = thread > USER_BASE;
        let index = if user_view { thread - USER_BASE } else { thread }.checked_sub(1)?;
        self.contexts.get(index).map(|context| (context, user_view))
    }

    fn selected_registers(&self) -> [u64; REGISTERS] {
        match self.lookup(self.selected) {
            Some((context, user_view)) => registers(context, user_view),
            None => [0; REGISTERS],
        }
    }

    fn reply(&mut self, body: &str) -> std::io::Result<()> {
        let checksum = body.bytes().fold(0u8, |sum, byte| sum.wrapping_add(byte));
        self.stream.write_all(format!("${body}#{checksum:02x}").as_bytes())
    }

    fn handle(&mut self, packet: &str) -> std::io::Result<bool> {
        let answer: String = if packet == "?" {
            self.contexts = contexts();
            format!("T05thread:{:x};", self.selected)
        } else if packet.starts_with("qSupported") {
            // No target description: this GDB rejects one with 64-bit
            // registers for an n32 file and its built-in layout fits.
            "PacketSize=2000".into()
        } else if packet == "qAttached" {
            "1".into()
        } else if packet == "qC" {
            format!("QC{:x}", self.selected)
        } else if packet == "qfThreadInfo" {
            self.contexts = contexts();
            let ids: Vec<String> = self.thread_ids().iter().map(|id| format!("{id:x}")).collect();
            format!("m{}", ids.join(","))
        } else if packet == "qsThreadInfo" {
            "l".into()
        } else if let Some(id) = packet.strip_prefix("qThreadExtraInfo,") {
            let thread = usize::from_str_radix(id, 16).unwrap_or(0);
            let text = match self.lookup(thread) {
                Some((context, user_view)) => format!(
                    "{} pid {} id {}{}{}",
                    context.name,
                    context.pid,
                    context.id,
                    if user_view { " user mode" } else { " kernel" },
                    if context.running { " (running, stale)" } else { "" }
                ),
                None => "gone".into(),
            };
            hex(text.as_bytes())
        } else if let Some(id) = packet.strip_prefix("Hg").or_else(|| packet.strip_prefix("Hc")) {
            if let Ok(thread) = usize::from_str_radix(id, 16) {
                if thread > 0 && self.lookup(thread).is_some() {
                    self.selected = thread;
                }
            }
            "OK".into()
        } else if let Some(id) = packet.strip_prefix('T') {
            let thread = usize::from_str_radix(id, 16).unwrap_or(0);
            if self.lookup(thread).is_some() { "OK".into() } else { "E01".into() }
        } else if packet == "g" {
            let regs = self.selected_registers();
            regs.iter().map(|r| format!("{r:016x}")).collect()
        } else if let Some(n) = packet.strip_prefix('p') {
            let n = usize::from_str_radix(n, 16).unwrap_or(usize::MAX);
            match self.selected_registers().get(n) {
                Some(r) => format!("{r:016x}"),
                None => "E01".into(),
            }
        } else if let Some(rest) = packet.strip_prefix('m') {
            let (addr, len) = rest.split_once(',').unwrap_or(("0", "0"));
            // GDB sends 64-bit addresses; kernel addresses arrive
            // sign-extended.
            let addr = u64::from_str_radix(addr, 16).unwrap_or(0) as u32 as usize;
            let len = usize::from_str_radix(len, 16).unwrap_or(0).min(0x800);
            let table = self.lookup(self.selected).map_or(0, |(context, _)| context.table);
            let bytes = read_memory(table, addr, len);
            if bytes.is_empty() { "E14".into() } else { hex(&bytes) }
        } else if packet == "c" || packet == "s" || packet.starts_with("vCont;") {
            // Nothing stops or runs on request: report the same stop again.
            self.contexts = contexts();
            format!("T05thread:{:x};", self.selected)
        } else if packet == "D" || packet == "k" {
            self.reply("OK")?;
            return Ok(false);
        } else if packet.starts_with('M') || packet.starts_with('X') || packet.starts_with('G') {
            "E01".into() // read-only
        } else {
            String::new() // unsupported
        };
        self.reply(&answer)?;
        Ok(true)
    }

    fn run(&mut self) -> std::io::Result<()> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = self.stream.read(&mut chunk)?;
            if n == 0 {
                return Ok(());
            }
            buffer.extend_from_slice(&chunk[..n]);
            // Packets are `$body#xx`; `+`, `-` and interrupt bytes between
            // them are dropped.
            while let Some(start) = buffer.iter().position(|&b| b == b'$') {
                let Some(hash) = buffer[start..].iter().position(|&b| b == b'#') else {
                    break;
                };
                let end = start + hash;
                if buffer.len() < end + 3 {
                    break;
                }
                let body = String::from_utf8_lossy(&buffer[start + 1..end]).into_owned();
                buffer.drain(..end + 3);
                self.stream.write_all(b"+")?;
                if !self.handle(&body)? {
                    return Ok(());
                }
            }
            if !buffer.contains(&b'$') {
                buffer.clear();
            }
        }
    }
}

fn main() {
    let listener = loop {
        match TcpListener::bind(("0.0.0.0", PORT)) {
            Ok(listener) => break listener,
            Err(_) => thread::sleep(Duration::from_secs(2)),
        }
    };
    println!("kdbg: GDB remote protocol on tcp port {PORT}\r");
    for stream in listener.incoming().flatten() {
        let mut session = Session { stream, contexts: contexts(), selected: 1 };
        let _ = session.run();
    }
}
