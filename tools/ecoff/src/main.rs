//! ELF32 big-endian MIPS to ECOFF, for `boot -f bootp()redox` on the SGI Indy.
//!
//! The Indy PROM loads an ECOFF `OMAGIC` executable with three sections:
//! `.text`, `.data` and `.bss`. Every `PT_LOAD` segment of the ELF file is
//! placed into one contiguous image starting at the lowest load address.
//! Executable segments form `.text`; the rest form `.data`. Memory past the
//! file-backed bytes becomes `.bss`.
//!
//! Usage:
//!   ecoff convert KERNEL.elf OUT [--append PAYLOAD]
//!   ecoff inspect OUT
//!
//! `--append` places a payload (the initfs) after the kernel: `.bss` is
//! written out as zeros so the file bytes stay contiguous, then a 4 KiB header
//! page (`REDOXIFS`, payload length as big-endian u32) and the payload, padded
//! to 4 KiB. The kernel finds the header at the first page boundary after its
//! `__end` symbol.
//!
//! Field widths and the header order follow the ECOFF `filehdr`, `aouthdr`
//! and `scnhdr` definitions. All fields are big-endian.

use std::{env, fmt, fs, process};

const FILE_HEADER_LEN: usize = 20;
const AOUT_HEADER_LEN: usize = 56;
const SECTION_HEADER_LEN: usize = 40;
const SECTION_COUNT: usize = 3;
/// `N_TXTOFF(OMAGIC)`: the headers rounded up to 16 bytes.
const TEXT_FILE_OFFSET: usize =
    (FILE_HEADER_LEN + AOUT_HEADER_LEN + SECTION_COUNT * SECTION_HEADER_LEN + 15) & !15;

/// `MIPSEBMAGIC`: big-endian MIPS object.
const MIPSEB_MAGIC: u16 = 0x0160;
/// `OMAGIC`: text is writable and not demand paged; the PROM copies it as-is.
const OMAGIC: u16 = 0x0107;
/// `f_flags`: F_RELFLG | F_EXEC | F_LNNO | F_LSYMS | F_MIPS_NO_SHARED.
const FILE_FLAGS: u16 = 0x100f;
const STYP_TEXT: u32 = 0x20;
const STYP_DATA: u32 = 0x40;
const STYP_BSS: u32 = 0x80;

const ELF_PT_LOAD: u32 = 1;
const ELF_PF_X: u32 = 1;
const ELF_EM_MIPS: u16 = 8;

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error(message.into()))
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u16(&self, at: usize) -> Result<u16, Error> {
        match self.0.get(at..at + 2) {
            Some(b) => Ok(u16::from_be_bytes([b[0], b[1]])),
            None => err(format!("file is truncated at offset {at:#x}")),
        }
    }

    fn u32(&self, at: usize) -> Result<u32, Error> {
        match self.0.get(at..at + 4) {
            Some(b) => Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            None => err(format!("file is truncated at offset {at:#x}")),
        }
    }
}

/// One `PT_LOAD` segment.
#[derive(Debug, Clone, Copy)]
struct Segment {
    vaddr: u32,
    file_offset: usize,
    file_size: usize,
    mem_size: u32,
    executable: bool,
}

/// The loadable image as the ECOFF headers describe it.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    /// Virtual address of the first byte.
    pub base: u32,
    pub entry: u32,
    /// Bytes of `.text`; `.data` is the rest of `bytes`.
    pub text_len: usize,
    pub bytes: Vec<u8>,
    pub bss_len: u32,
}

fn parse_segments(elf: &[u8]) -> Result<(u32, Vec<Segment>), Error> {
    let r = Reader(elf);
    if elf.len() < 52 || &elf[..4] != b"\x7fELF" {
        return err("input is not an ELF file");
    }
    if elf[4] != 1 {
        return err("input is not ELF32");
    }
    if elf[5] != 2 {
        return err("input is not big-endian");
    }
    if r.u16(18)? != ELF_EM_MIPS {
        return err("input is not a MIPS executable");
    }
    let entry = r.u32(24)?;
    let phoff = r.u32(28)? as usize;
    let phentsize = r.u16(42)? as usize;
    let phnum = r.u16(44)? as usize;

    let mut segments = Vec::new();
    for index in 0..phnum {
        let at = phoff + index * phentsize;
        if r.u32(at)? != ELF_PT_LOAD {
            continue;
        }
        let mem_size = r.u32(at + 20)?;
        if mem_size == 0 {
            continue;
        }
        segments.push(Segment {
            file_offset: r.u32(at + 4)? as usize,
            vaddr: r.u32(at + 8)?,
            file_size: r.u32(at + 16)? as usize,
            mem_size,
            executable: r.u32(at + 24)? & ELF_PF_X != 0,
        });
    }
    segments.sort_by_key(|s| s.vaddr);
    Ok((entry, segments))
}

/// Build the ECOFF image from an ELF32 big-endian MIPS executable.
pub fn parse_elf(elf: &[u8]) -> Result<Image, Error> {
    let (entry, segments) = parse_segments(elf)?;
    let first = match segments.first() {
        Some(s) => *s,
        None => return err("ELF has no loadable segment"),
    };
    let base = first.vaddr;

    let mut file_end = base as u64;
    let mut mem_end = base as u64;
    let mut text_end = base as u64;
    for s in &segments {
        let seg_file_end = s.vaddr as u64 + s.file_size as u64;
        let seg_mem_end = s.vaddr as u64 + s.mem_size as u64;
        if s.file_size != 0 {
            file_end = file_end.max(seg_file_end);
        }
        mem_end = mem_end.max(seg_mem_end);
        if s.executable && s.file_size != 0 {
            text_end = text_end.max(seg_file_end);
        }
    }
    if file_end == base as u64 {
        return err("ELF has no file-backed bytes");
    }
    if mem_end > u32::MAX as u64 + 1 {
        return err("image runs past the 32-bit address space");
    }

    let mut bytes = vec![0u8; (file_end - base as u64) as usize];
    for s in &segments {
        let source = match elf.get(s.file_offset..s.file_offset + s.file_size) {
            Some(source) => source,
            None => return err(format!("segment at {:#010x} runs past the end of the file", s.vaddr)),
        };
        let start = (s.vaddr - base) as usize;
        bytes[start..start + s.file_size].copy_from_slice(source);
    }

    if (entry as u64) < base as u64 || entry as u64 >= file_end {
        return err(format!("entry {entry:#010x} is outside the loaded image"));
    }

    // Both sections must be non-empty for the PROM loader. If there is no
    // executable segment, or everything is executable, split in the middle.
    let mut text_len = (text_end - base as u64) as usize;
    if text_len == 0 || text_len >= bytes.len() {
        text_len = (bytes.len() / 2) & !15;
    }
    if text_len == 0 || text_len >= bytes.len() {
        return err("image is too small to split into .text and .data");
    }

    Ok(Image {
        base,
        entry,
        text_len,
        bytes,
        bss_len: (mem_end - file_end) as u32,
    })
}

struct Writer(Vec<u8>);

impl Writer {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn section(&mut self, name: &[u8], addr: u32, size: u32, file_offset: u32, flags: u32) {
        let mut field = [0u8; 8];
        field[..name.len()].copy_from_slice(name);
        self.0.extend_from_slice(&field);
        self.u32(addr); // s_paddr
        self.u32(addr); // s_vaddr
        self.u32(size); // s_size
        self.u32(file_offset); // s_scnptr
        self.u32(0); // s_relptr
        self.u32(0); // s_lnnoptr
        self.u16(0); // s_nreloc
        self.u16(0); // s_nlnno
        self.u32(flags); // s_flags
    }
}

/// Serialise the image as an ECOFF executable.
pub fn write_ecoff(image: &Image) -> Vec<u8> {
    let text_len = image.text_len as u32;
    let data_len = (image.bytes.len() - image.text_len) as u32;
    let data_start = image.base.wrapping_add(text_len);
    let bss_start = data_start.wrapping_add(data_len);

    let mut w = Writer(Vec::with_capacity(TEXT_FILE_OFFSET + image.bytes.len()));
    // struct filehdr
    w.u16(MIPSEB_MAGIC); // f_magic
    w.u16(SECTION_COUNT as u16); // f_nscns
    w.u32(0); // f_timdat
    w.u32(0); // f_symptr
    w.u32(0); // f_nsyms
    w.u16(AOUT_HEADER_LEN as u16); // f_opthdr
    w.u16(FILE_FLAGS); // f_flags
    // struct aouthdr
    w.u16(OMAGIC); // magic
    w.u16(0); // vstamp
    w.u32(text_len); // tsize
    w.u32(data_len); // dsize
    w.u32(image.bss_len); // bsize
    w.u32(image.entry); // entry
    w.u32(image.base); // text_start
    w.u32(data_start); // data_start
    w.u32(bss_start); // bss_start
    w.u32(0); // gprmask
    for _ in 0..4 {
        w.u32(0); // cprmask
    }
    w.u32(0); // gp_value
    // struct scnhdr * 3
    w.section(b".text", image.base, text_len, TEXT_FILE_OFFSET as u32, STYP_TEXT);
    w.section(b".data", data_start, data_len, TEXT_FILE_OFFSET as u32 + text_len, STYP_DATA);
    w.section(b".bss", bss_start, image.bss_len, 0, STYP_BSS);
    debug_assert!(w.0.len() <= TEXT_FILE_OFFSET);
    w.0.resize(TEXT_FILE_OFFSET, 0);
    w.0.extend_from_slice(&image.bytes);
    w.0
}

/// Magic at the start of the payload header page.
pub const PAYLOAD_MAGIC: &[u8; 8] = b"REDOXIFS";
const PAGE: usize = 4096;

/// Append `payload` after the image: zero-fill `.bss`, add the header page and
/// the payload padded to a page.
pub fn append_payload(image: &mut Image, payload: &[u8]) -> Result<(), Error> {
    let total = image.bytes.len() + image.bss_len as usize;
    let header_at = (total + PAGE - 1) & !(PAGE - 1);
    image.bytes.resize(header_at, 0);
    let mut header = [0u8; PAGE];
    header[..8].copy_from_slice(PAYLOAD_MAGIC);
    let len = u32::try_from(payload.len()).map_err(|_| Error("payload is larger than 4 GiB".into()))?;
    header[8..12].copy_from_slice(&len.to_be_bytes());
    image.bytes.extend_from_slice(&header);
    image.bytes.extend_from_slice(payload);
    let padded = (image.bytes.len() + PAGE - 1) & !(PAGE - 1);
    image.bytes.resize(padded, 0);
    image.bss_len = 0;
    if image.bytes.len() as u64 + image.base as u64 > u32::MAX as u64 + 1 {
        return err("image with payload runs past the 32-bit address space");
    }
    Ok(())
}

fn convert(input: &str, output: &str, payload: Option<&str>) -> Result<(), Error> {
    let elf = fs::read(input).map_err(|e| Error(format!("{input}: {e}")))?;
    let mut image = parse_elf(&elf)?;
    if let Some(path) = payload {
        let data = fs::read(path).map_err(|e| Error(format!("{path}: {e}")))?;
        append_payload(&mut image, &data)?;
        println!("{output}: payload {path} ({} bytes) appended", data.len());
    }
    let ecoff = write_ecoff(&image);
    fs::write(output, &ecoff).map_err(|e| Error(format!("{output}: {e}")))?;
    println!(
        "{output}: {} bytes; text {:#010x}+{:#x}, data {:#010x}+{:#x}, bss {:#x}, entry {:#010x}",
        ecoff.len(),
        image.base,
        image.text_len,
        image.base as usize + image.text_len,
        image.bytes.len() - image.text_len,
        image.bss_len,
        image.entry,
    );
    Ok(())
}

fn inspect(path: &str) -> Result<(), Error> {
    let data = fs::read(path).map_err(|e| Error(format!("{path}: {e}")))?;
    let r = Reader(&data);
    let magic = r.u16(0)?;
    if magic != MIPSEB_MAGIC {
        return err(format!("{path}: f_magic {magic:#06x} is not MIPSEBMAGIC"));
    }
    let nscns = r.u16(2)? as usize;
    let opthdr = r.u16(16)? as usize;
    println!("f_magic  {magic:#06x}  f_nscns {nscns}  f_opthdr {opthdr}  f_flags {:#06x}", r.u16(18)?);
    let a = FILE_HEADER_LEN;
    println!("magic    {:#06x}  vstamp {}", r.u16(a)?, r.u16(a + 2)?);
    println!("tsize    {:#010x}  dsize {:#010x}  bsize {:#010x}", r.u32(a + 4)?, r.u32(a + 8)?, r.u32(a + 12)?);
    println!("entry    {:#010x}", r.u32(a + 16)?);
    println!("text     {:#010x}  data  {:#010x}  bss   {:#010x}", r.u32(a + 20)?, r.u32(a + 24)?, r.u32(a + 28)?);
    for i in 0..nscns {
        let s = FILE_HEADER_LEN + opthdr + i * SECTION_HEADER_LEN;
        let name = data.get(s..s + 8).unwrap_or(&[]);
        let name: String = name.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
        println!(
            "{name:<8} vaddr {:#010x} size {:#010x} scnptr {:#010x} flags {:#06x}",
            r.u32(s + 12)?,
            r.u32(s + 16)?,
            r.u32(s + 20)?,
            r.u32(s + 36)?
        );
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [_, "convert", input, output] => convert(input, output, None),
        [_, "convert", input, output, "--append", payload] => convert(input, output, Some(payload)),
        [_, "inspect", path] => inspect(path),
        _ => err("usage: ecoff convert KERNEL.elf OUT [--append PAYLOAD] | ecoff inspect FILE"),
    };
    if let Err(e) = result {
        eprintln!("ecoff: {e}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ELF with an executable segment of 48 bytes and a data segment of
    /// 16 file bytes plus 32 bytes of bss, both at 0x8804_0000 upward.
    fn sample_elf() -> Vec<u8> {
        let mut elf = vec![0u8; 52 + 2 * 32];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1;
        elf[5] = 2;
        elf[18..20].copy_from_slice(&ELF_EM_MIPS.to_be_bytes());
        elf[24..28].copy_from_slice(&0x8804_0010u32.to_be_bytes());
        elf[28..32].copy_from_slice(&52u32.to_be_bytes());
        elf[42..44].copy_from_slice(&32u16.to_be_bytes());
        elf[44..46].copy_from_slice(&2u16.to_be_bytes());
        let payload = 52 + 64;
        let ph = |elf: &mut Vec<u8>, i: usize, off: u32, vaddr: u32, filesz: u32, memsz: u32, flags: u32| {
            let at = 52 + i * 32;
            elf[at..at + 4].copy_from_slice(&ELF_PT_LOAD.to_be_bytes());
            elf[at + 4..at + 8].copy_from_slice(&off.to_be_bytes());
            elf[at + 8..at + 12].copy_from_slice(&vaddr.to_be_bytes());
            elf[at + 16..at + 20].copy_from_slice(&filesz.to_be_bytes());
            elf[at + 20..at + 24].copy_from_slice(&memsz.to_be_bytes());
            elf[at + 24..at + 28].copy_from_slice(&flags.to_be_bytes());
        };
        ph(&mut elf, 0, payload as u32, 0x8804_0000, 48, 48, 5);
        ph(&mut elf, 1, payload as u32 + 48, 0x8804_0030, 16, 48, 6);
        elf.extend((0..64u8).map(|b| b + 1));
        elf
    }

    #[test]
    fn segments_form_one_image_split_at_the_data_segment() {
        let image = parse_elf(&sample_elf()).unwrap();
        assert_eq!(image.base, 0x8804_0000);
        assert_eq!(image.entry, 0x8804_0010);
        assert_eq!(image.text_len, 48);
        assert_eq!(image.bytes.len(), 64);
        assert_eq!(image.bytes[0], 1);
        assert_eq!(image.bytes[63], 64);
        assert_eq!(image.bss_len, 32);
    }

    #[test]
    fn headers_describe_the_image() {
        let image = parse_elf(&sample_elf()).unwrap();
        let out = write_ecoff(&image);
        let r = Reader(&out);
        assert_eq!(TEXT_FILE_OFFSET, 208);
        assert_eq!(out.len(), 208 + 64);
        assert_eq!(r.u16(0).unwrap(), MIPSEB_MAGIC);
        assert_eq!(r.u16(20).unwrap(), OMAGIC);
        assert_eq!(r.u32(24).unwrap(), 48); // tsize
        assert_eq!(r.u32(28).unwrap(), 16); // dsize
        assert_eq!(r.u32(32).unwrap(), 32); // bsize
        assert_eq!(r.u32(36).unwrap(), 0x8804_0010); // entry
        assert_eq!(r.u32(40).unwrap(), 0x8804_0000); // text_start
        assert_eq!(r.u32(44).unwrap(), 0x8804_0030); // data_start
        assert_eq!(r.u32(48).unwrap(), 0x8804_0040); // bss_start
        let data_hdr = FILE_HEADER_LEN + AOUT_HEADER_LEN + SECTION_HEADER_LEN;
        assert_eq!(&out[data_hdr..data_hdr + 5], b".data");
        assert_eq!(r.u32(data_hdr + 20).unwrap(), 208 + 48); // s_scnptr
        assert_eq!(&out[208..], &image.bytes[..]);
    }

    #[test]
    fn payload_follows_bss_on_a_page_boundary() {
        let mut image = parse_elf(&sample_elf()).unwrap();
        append_payload(&mut image, b"initfs").unwrap();
        // 64 file bytes + 32 bss -> header page at 4096, payload at 8192, padded to 12288.
        assert_eq!(image.bss_len, 0);
        assert_eq!(image.bytes.len(), 12288);
        assert_eq!(&image.bytes[4096..4104], PAYLOAD_MAGIC);
        assert_eq!(&image.bytes[4104..4108], &6u32.to_be_bytes());
        assert_eq!(&image.bytes[8192..8198], b"initfs");
        assert_eq!(image.bytes[64], 0);
    }

    #[test]
    fn entry_outside_the_image_is_rejected() {
        let mut elf = sample_elf();
        elf[24..28].copy_from_slice(&0x8900_0000u32.to_be_bytes());
        assert!(parse_elf(&elf).is_err());
    }

    #[test]
    fn non_elf_input_is_rejected() {
        assert!(parse_elf(b"MZ not an elf").is_err());
    }
}
