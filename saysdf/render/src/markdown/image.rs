//! Markdown images (design D4): local files only; remote addresses are not loaded —
//! there is no network code here at all. The size comes from the file header (PNG/GIF/JPEG), without decoding.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Where the image comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// http/https/ftp/data:/`//host`: caption only.
    Remote,
    Local(PathBuf),
}

/// Whether the address has a scheme (`http:`, `mailto:`, …). A drive letter `C:` does not count as a scheme.
pub fn has_scheme(url: &str) -> bool {
    if url.starts_with("//") {
        return true;
    }
    match url.split_once(':') {
        Some((s, _)) => {
            s.len() > 1
                && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        None => false,
    }
}

/// Classification of an image address; relative paths are resolved against `base_dir`.
pub fn resolve(url: &str, base_dir: Option<&Path>) -> ImageSource {
    let path = if let Some(rest) = url.strip_prefix("file://") {
        rest.trim_start_matches('/').to_owned()
    } else if has_scheme(url) {
        return ImageSource::Remote;
    } else {
        url.to_owned()
    };
    let p = Path::new(&path);
    match base_dir {
        Some(b) if p.is_relative() => ImageSource::Local(b.join(p)),
        _ => ImageSource::Local(p.to_path_buf()),
    }
}

/// Image size in pixels from the file header.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(256 * 1024).read_to_end(&mut buf).ok()?;
    header_dimensions(&buf)
}

/// Size from the first bytes of the file: PNG, GIF, JPEG.
pub fn header_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?));
    let be16 = |i: usize| Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?) as u32);
    let le16 = |i: usize| Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as u32);
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

/// Size in cells (a cell is ~8×16 px), at most `max_cols` wide and 30 rows high.
pub fn cells(w: u32, h: u32, max_cols: usize) -> (usize, usize) {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let mut cols = w.div_ceil(8).clamp(1, max_cols.max(1));
    let mut rows = (h * cols * 8).div_ceil(w * 16).max(1);
    if rows > 30 {
        rows = 30;
        cols = (w * rows * 16).div_ceil(h * 8).clamp(1, max_cols.max(1));
    }
    (cols, rows)
}
