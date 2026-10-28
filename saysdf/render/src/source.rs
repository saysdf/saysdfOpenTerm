//! File loading (design D7): binary detection, a small file in full, a large one via a line index
//! and windowed reads, hex view in 16-byte windows. `memmap2` is deliberately not used (D7).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// How many bytes to inspect for binary detection.
pub const SNIFF_LEN: usize = 8 * 1024;
/// Default "small file" threshold (`--max-size`).
pub const DEFAULT_MAX_SIZE: u64 = 16 * 1024 * 1024;
/// Read limit for one line of a large file: longer lines are truncated (a window is O(screen)).
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// Bytes per hex view row.
pub const HEX_ROW: u64 = 16;

/// Whether the buffer is binary: `\0` or more than 30% control/undecodable bytes (first 8 KiB inspected).
pub fn is_binary(data: &[u8]) -> bool {
    let data = &data[..data.len().min(SNIFF_LEN)];
    if data.is_empty() {
        return false;
    }
    if data.contains(&0) {
        return true;
    }
    let mut bad = 0usize;
    let mut seen = 0usize;
    for chunk in data.utf8_chunks() {
        bad += chunk
            .valid()
            .bytes()
            .filter(|b| matches!(b, 0x01..=0x08 | 0x0E..=0x1A | 0x1C..=0x1F | 0x7F))
            .count();
        seen += chunk.valid().len() + chunk.invalid().len();
        // A multibyte character cut at the 8 KiB boundary is not counted
        if seen < data.len() {
            bad += chunk.invalid().len();
        }
    }
    bad * 10 > data.len() * 3
}

/// Line from bytes: strips a trailing `\r` (CRLF), invalid UTF-8 becomes U+FFFD.
fn decode_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Splits text into lines on `\n`; a trailing newline does not produce an empty last line.
pub fn split_lines(data: &[u8]) -> Vec<String> {
    let data = data.strip_suffix(b"\n").unwrap_or(data);
    if data.is_empty() {
        return Vec::new();
    }
    data.split(|&b| b == b'\n').map(decode_line).collect()
}

/// Result of opening a file.
pub enum Opened {
    /// Small text file, fully in memory.
    Text(Vec<String>),
    /// Binary: show as hex.
    Binary(HexFile),
    /// Above the threshold: windowed reading without highlighting.
    Large(LargeFile),
}

/// Opens a file and decides how to display it.
pub fn open(path: &Path, max_size: u64) -> io::Result<Opened> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = Vec::with_capacity(SNIFF_LEN);
    (&mut file).take(SNIFF_LEN as u64).read_to_end(&mut head)?;
    if is_binary(&head) {
        return Ok(Opened::Binary(HexFile::from_file(file, len)));
    }
    if len > max_size {
        return Ok(Opened::Large(LargeFile::from_file(file)?));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut data = Vec::with_capacity(len as usize);
    // take: the file may have grown after metadata()
    file.take(max_size).read_to_end(&mut data)?;
    Ok(Opened::Text(split_lines(&data)))
}

/// Large text file: an index of line starts and on-demand window reads.
pub struct LargeFile {
    file: File,
    len: u64,
    /// Line start offsets; 8 bytes per line.
    offsets: Vec<u64>,
}

impl LargeFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::from_file(File::open(path)?)
    }

    /// A single streaming pass over the file builds the line index.
    pub fn from_file(mut file: File) -> io::Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let mut offsets = vec![0u64];
        let mut buf = vec![0u8; 64 * 1024];
        let mut pos = 0u64;
        loop {
            let n = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            offsets.extend(
                buf[..n]
                    .iter()
                    .enumerate()
                    .filter(|(_, &b)| b == b'\n')
                    .map(|(i, _)| pos + i as u64 + 1),
            );
            pos += n as u64;
        }
        // A trailing \n (or an empty file) does not produce an extra empty line
        if offsets.last() == Some(&pos) {
            offsets.pop();
        }
        Ok(Self {
            file,
            len: pos,
            offsets,
        })
    }

    pub fn line_count(&self) -> usize {
        self.offsets.len()
    }

    /// File size at the time the index was built.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Lines `[from, to)` (clamped to the line count); a line longer than `MAX_LINE_BYTES` is truncated.
    pub fn read_lines(&mut self, from: usize, to: usize) -> io::Result<Vec<String>> {
        let to = to.min(self.line_count());
        let mut out = Vec::with_capacity(to.saturating_sub(from));
        let mut buf = Vec::new();
        for i in from..to {
            let start = self.offsets[i];
            let end = self.offsets.get(i + 1).copied().unwrap_or(self.len);
            let want = (end - start).min(MAX_LINE_BYTES as u64);
            self.file.seek(SeekFrom::Start(start))?;
            buf.clear();
            (&mut self.file).take(want).read_to_end(&mut buf)?;
            let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
            out.push(decode_line(line));
        }
        Ok(out)
    }
}

/// File for the hex view: read in windows, memory is O(screen).
pub struct HexFile {
    file: File,
    len: u64,
}

impl HexFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self::from_file(file, len))
    }

    fn from_file(file: File, len: u64) -> Self {
        Self { file, len }
    }

    /// Number of 16-byte rows.
    pub fn rows(&self) -> u64 {
        self.len.div_ceil(HEX_ROW)
    }

    /// Hex view rows `[first, first + count)`.
    pub fn window(&mut self, first: u64, count: usize) -> io::Result<Vec<String>> {
        let start = first.saturating_mul(HEX_ROW).min(self.len);
        let want = (count as u64)
            .saturating_mul(HEX_ROW)
            .min(self.len - start);
        self.file.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::with_capacity(want as usize);
        (&mut self.file).take(want).read_to_end(&mut buf)?;
        Ok(buf
            .chunks(HEX_ROW as usize)
            .enumerate()
            .map(|(i, row)| hex_line(start + i as u64 * HEX_ROW, row))
            .collect())
    }
}

/// `00000010  41 42 …  |AB..|`: non-printable bytes in the ASCII column become `.`.
pub fn hex_line(offset: u64, row: &[u8]) -> String {
    let mut s = format!("{offset:08x} ");
    for i in 0..HEX_ROW as usize {
        if i == 8 {
            s.push(' ');
        }
        match row.get(i) {
            Some(b) => s.push_str(&format!(" {b:02x}")),
            None => s.push_str("   "),
        }
    }
    s.push_str("  |");
    s.extend(row.iter().map(|&b| {
        if b.is_ascii_graphic() || b == b' ' {
            b as char
        } else {
            '.'
        }
    }));
    s.push('|');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn binary_detection() {
        assert!(is_binary(b"abc\0def"));
        assert!(!is_binary(
            "fn main() { println!(\"Привет\"); }\r\n".as_bytes() // i18n-allow-cyrillic
        ));
        assert!(is_binary(&[0xffu8; 100]));
        assert!(!is_binary(b""));
    }

    #[test]
    fn split_crlf_and_invalid_utf8() {
        assert_eq!(split_lines(b"a\r\nb\n"), vec!["a", "b"]);
        assert_eq!(split_lines(b"a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(split_lines(b"x\xffy"), vec!["x\u{FFFD}y"]);
        assert!(split_lines(b"").is_empty());
    }

    fn temp_with(data: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(data).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn large_file_window() {
        let mut data = String::new();
        for i in 0..1000 {
            data.push_str(&format!("line {i}\r\n"));
        }
        data.push_str("last");
        let tmp = temp_with(data.as_bytes());
        let mut lf = LargeFile::open(tmp.path()).unwrap();
        assert_eq!(lf.line_count(), 1001);
        assert_eq!(
            lf.read_lines(500, 502).unwrap(),
            vec!["line 500", "line 501"]
        );
        assert_eq!(lf.read_lines(1000, 5000).unwrap(), vec!["last"]);
        assert!(lf.read_lines(2000, 2001).unwrap().is_empty());

        let tmp = temp_with(b"a\nb\n");
        assert_eq!(LargeFile::open(tmp.path()).unwrap().line_count(), 2);
        let tmp = temp_with(b"");
        assert_eq!(LargeFile::open(tmp.path()).unwrap().line_count(), 0);
    }

    #[test]
    fn hex_window_offsets() {
        let data: Vec<u8> = (0u8..40).collect();
        let tmp = temp_with(&data);
        let mut hf = HexFile::open(tmp.path()).unwrap();
        assert_eq!(hf.rows(), 3);
        let rows = hf.window(1, 2).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].starts_with("00000010  10 11 12"), "{}", rows[0]);
        assert!(rows[1].starts_with("00000020  20 21"), "{}", rows[1]);
        assert!(rows[1].ends_with("| !\"#$%&'|"), "{}", rows[1]);
    }

    #[test]
    fn open_picks_kind() {
        let tmp = temp_with("привет\nмир\n".as_bytes()); // i18n-allow-cyrillic
        match open(tmp.path(), DEFAULT_MAX_SIZE).unwrap() {
            Opened::Text(lines) => assert_eq!(lines, ["привет", "мир"]), // i18n-allow-cyrillic
            _ => panic!("expected text"),
        }
        assert!(matches!(open(tmp.path(), 4).unwrap(), Opened::Large(_)));
        let tmp = temp_with(b"\x00\x01\x02");
        assert!(matches!(
            open(tmp.path(), DEFAULT_MAX_SIZE).unwrap(),
            Opened::Binary(_)
        ));
    }
}
