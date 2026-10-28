//! Tests for inline image protocol handling

use super::*;
use wezterm_cell::image::ImageDataType;
use wezterm_surface::change::ImageData;

/// A tiny but valid 11x11 PNG, base64 encoded.
/// Taken from the reproduction in <https://github.com/wezterm/wezterm/issues/6344>.
const TINY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAsAAAALCAYAAACprHcmAAAACXBIWXMAAAGKAAABigEzlzBYAAAAOUlEQVQYlZXOwQ0AMAzCQEdi7yaT0xWAN7JuDCac2PQKYxflycOoICOKtPIuqFCg4/LzKxiz6xjyAYh9DR1sLUN1AAAAAElFTkSuQmCC";

/// Feeding a Kitty graphics escape that requests a zero-sized placement (here `r=0,h=0`)
/// must not panic the terminal.
/// Prior to the fix for <https://github.com/wezterm/wezterm/issues/6344> this divided by zero
/// while computing the per-cell pixel deltas and took down the whole pane.
#[test]
fn kitty_zero_dimension_image_does_not_panic() {
    let mut term = TestTerm::new(3, 10, 0);

    // a=T: transmit and display, t=d: data is directly embedded,
    // f=100: PNG, r=0/h=0: zero rows / zero source height.
    let seq = format!("\x1b_Gr=0,h=0,a=T,t=d,f=100;{}\x1b\\", TINY_PNG_BASE64);
    term.print(seq.as_bytes());

    // The image is refused, so the cursor never moved;
    // Printing normal text and observing it confirms we recovered rather than crashing.
    term.print(b"ok");
    assert_visible_contents(&term, file!(), line!(), &["ok", "", ""]);
}

/// A well-formed Kitty graphic with non-zero dimensions should continue to be accepted.
/// The test passes as long as processing the image does not panic and the terminal remains usable.
#[test]
fn kitty_valid_image_is_accepted() {
    let mut term = TestTerm::new(3, 10, 0);

    let seq = format!("\x1b_Ga=T,t=d,f=100;{}\x1b\\", TINY_PNG_BASE64);
    term.print(seq.as_bytes());

    // Printing normal text and observing it shifted confirms the terminal is usable.
    term.print(b"ok");
    assert_visible_contents(&term, file!(), line!(), &["  ok", "", ""]);
}

/// When the pty has no pixel size, `cell_pixel_width`/`cell_pixel_height` are zero.
/// Displaying an image sized in cells (ie: without explicit `c=`/`r=`) must not divide by zero.
/// This is a distinct crash from the zero-dimension image above and is not caught by that guard.
/// See <https://github.com/wezterm/wezterm/issues/6344>.
#[test]
fn kitty_image_with_zero_pixel_dimensions_does_not_panic() {
    let mut term = Terminal::new(
        TerminalSize {
            rows: 3,
            cols: 80,
            // No pixel size!
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        },
        Arc::new(TestTermConfig { scrollback: 0 }),
        "WezTerm",
        "O_o",
        Box::new(Vec::new()),
    );

    // No `c=`/`r=`, so the placement is computed from the (zero) cell pixel
    // size, exercising the divide that previously panicked.
    let seq = format!("\x1b_Ga=T,t=d,f=100;{}\x1b\\", TINY_PNG_BASE64);
    term.advance_bytes(seq.as_bytes());

    // The image is refused, so the cursor never moved;
    // Printing normal text and observing it confirms we recovered rather than crashing.
    term.advance_bytes(b"ok");
    assert_visible_contents(&term, file!(), line!(), &["ok", "", ""]);
}

/// A 2x2 RGBA image, base64 encoded.
const TINY_RGBA_BASE64: &str = "AQID/wQFBv8HCAn/CgsM/w==";

/// The image data attached to the first cell that carries one.
fn first_image(term: &TestTerm) -> Arc<ImageData> {
    for line in term.screen().visible_lines().iter() {
        for cell in line.visible_cells() {
            if let Some(im) = cell
                .attrs()
                .images()
                .and_then(|images| images.into_iter().next())
            {
                return Arc::clone(im.image_data());
            }
        }
    }
    panic!("no image was attached to the screen");
}

/// An Rgba8 stores the hash of its pixels and `compute_hash` reports it without
/// recomputing, so a kitty frame transmission that edits those pixels in place
/// must leave the stored hash describing what is now there.
#[test]
fn kitty_frame_edit_keeps_the_stored_hash_current() {
    let mut term = TestTerm::new(3, 10, 0);

    // a=T: transmit and display, f=32: RGBA, s/v: 2x2 pixels.
    let seq = format!("\x1b_Ga=T,t=d,f=32,s=2,v=2,i=1;{}\x1b\\", TINY_RGBA_BASE64);
    term.advance_bytes(seq.as_bytes());
    let transmitted_hash = first_image(&term).data().compute_hash();

    // a=f: transmit a frame, r=1: edit frame 1 in place, painting the top left
    // pixel opaque red over the 0x01,0x02,0x03 it arrived with.
    term.advance_bytes(b"\x1b_Ga=f,t=d,f=32,s=1,v=1,i=1,r=1,x=0,y=0;/wAA/w==\x1b\\");

    let image = first_image(&term);
    let image = image.data();
    let edited_hash = image.compute_hash();

    match &*image {
        ImageDataType::Rgba8 { data, .. } => {
            // Asserted on the pixels rather than on the hash, so a blit that
            // did nothing cannot be mistaken for a hash that went stale.
            k9::assert_equal!(&data[0..4], &[0xff, 0x00, 0x00, 0xff][..]);
            k9::assert_equal!(edited_hash, ImageDataType::hash_bytes(data));
        }
        other => panic!("expected Rgba8, got {:?}", other),
    }

    assert_ne!(edited_hash, transmitted_hash);
}

// saysdfTerminal: inline-graphics — tests for iTerm2/Sixel, image limits, kitty transfer media

/// Test config with adjustable image limits.
#[derive(Debug)]
struct LimitConfig {
    max_pixels: u32,
    kitty_budget: usize,
    allow_files: bool,
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            max_pixels: 7680 * 4320,
            kitty_budget: 320 * 1024 * 1024,
            allow_files: false,
        }
    }
}

impl TerminalConfiguration for LimitConfig {
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
    fn enable_kitty_graphics(&self) -> bool {
        true
    }
    fn max_image_pixels(&self) -> u32 {
        self.max_pixels
    }
    fn kitty_image_memory_budget(&self) -> usize {
        self.kitty_budget
    }
    fn allow_kitty_file_transfer(&self) -> bool {
        self.allow_files
    }
}

/// Collects what the terminal writes back to the pty.
#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Replies {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Replies {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).to_string()
    }

    /// Replies reach the writer from another thread: wait for a complete APC reply.
    fn wait_apc(&self) -> String {
        let start = std::time::Instant::now();
        loop {
            let text = self.text();
            if text.ends_with("\x1b\\") || start.elapsed() > std::time::Duration::from_secs(2) {
                return text;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// 24x80 terminal, 8x16 pixel cells.
fn limit_term(config: LimitConfig, replies: Replies) -> Terminal {
    Terminal::new(
        TerminalSize {
            rows: 24,
            cols: 80,
            pixel_width: 80 * 8,
            pixel_height: 24 * 16,
            dpi: 0,
        },
        Arc::new(config),
        "WezTerm",
        "O_o",
        Box::new(replies),
    )
}

/// All distinct image data attached to visible cells.
fn images_in(term: &Terminal) -> Vec<Arc<ImageData>> {
    let mut out: Vec<Arc<ImageData>> = vec![];
    for line in term.screen().visible_lines().iter() {
        for cell in line.visible_cells() {
            if let Some(images) = cell.attrs().images() {
                for im in images {
                    let data = im.image_data();
                    if !out.iter().any(|o| Arc::ptr_eq(o, data)) {
                        out.push(Arc::clone(data));
                    }
                }
            }
        }
    }
    out
}

/// A real PNG of `w`x`h` pixels, encoded with the `image` crate.
fn tiny_png(w: u32, h: u32) -> Vec<u8> {
    let img = ::image::RgbaImage::from_pixel(w, h, ::image::Rgba([10, 20, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ::image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A PNG with only the signature and IHDR (claims `w`x`h`, carries no pixel data).
fn png_header_only(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = b"IHDR".to_vec();
    chunk.extend_from_slice(&w.to_be_bytes());
    chunk.extend_from_slice(&h.to_be_bytes());
    chunk.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&chunk).to_be_bytes());
    out
}

/// Standard base64 (the test crate has no base64 dependency).
fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn iterm_seq(args: &str, png: &[u8]) -> String {
    format!("\x1b]1337;File=inline=1{}:{}\x07", args, b64(png))
}

#[test]
fn saysdf_b64_matches_known_value() {
    k9::assert_equal!(b64(b"Man"), "TWFu".to_string());
    k9::assert_equal!(b64(b"Ma"), "TWE=".to_string());
    k9::assert_equal!(b64(b"M"), "TQ==".to_string());
}

#[test]
fn iterm2_small_png_is_attached() {
    let mut term = limit_term(LimitConfig::default(), Replies::default());
    term.advance_bytes(iterm_seq("", &tiny_png(2, 2)).as_bytes());
    k9::assert_equal!(images_in(&term).len(), 1);
}

#[test]
fn sixel_small_image_is_attached() {
    let mut term = limit_term(LimitConfig::default(), Replies::default());
    // color 0 = red, two sixel columns of 6 pixels
    term.advance_bytes(b"\x1bPq#0;2;100;0;0#0~~\x1b\\");
    k9::assert_equal!(images_in(&term).len(), 1);
}

#[test]
fn iterm2_huge_png_header_is_rejected() {
    let mut term = limit_term(LimitConfig::default(), Replies::default());
    term.advance_bytes(iterm_seq("", &png_header_only(20000, 20000)).as_bytes());
    term.advance_bytes(b"ok");
    assert!(images_in(&term).is_empty());
    let mut expect = vec!["ok"];
    expect.resize(24, "");
    assert_visible_contents(&term, file!(), line!(), &expect);
}

#[test]
fn max_image_pixels_from_config() {
    let png = tiny_png(1200, 1000);
    let mut small = limit_term(
        LimitConfig {
            max_pixels: 1_000_000,
            ..Default::default()
        },
        Replies::default(),
    );
    small.advance_bytes(iterm_seq("", &png).as_bytes());
    assert!(images_in(&small).is_empty());

    let mut default = limit_term(LimitConfig::default(), Replies::default());
    default.advance_bytes(iterm_seq("", &png).as_bytes());
    k9::assert_equal!(images_in(&default).len(), 1);
}

/// Fuzzing found that `width=99999;height=auto` stretched an 11x11 PNG over ~99999x50000
/// cells and hung the terminal; now the placement is refused quickly.
#[test]
fn iterm2_huge_cell_width_is_refused_quickly() {
    let start = std::time::Instant::now();
    let mut term = limit_term(LimitConfig::default(), Replies::default());
    term.advance_bytes(iterm_seq(";width=99999;height=auto", &tiny_png(11, 11)).as_bytes());
    term.advance_bytes(b"ok");
    assert!(images_in(&term).is_empty());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn kitty_huge_cell_count_is_refused_quickly() {
    let start = std::time::Instant::now();
    let mut term = limit_term(LimitConfig::default(), Replies::default());
    let seq = format!(
        "\x1b_Ga=T,t=d,f=32,s=2,v=2,c=99999,r=99999;{}\x1b\\",
        TINY_RGBA_BASE64
    );
    term.advance_bytes(seq.as_bytes());
    assert!(images_in(&term).is_empty());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn kitty_memory_budget_refuses_image_that_does_not_fit() {
    // one 2x2 RGBA image is 16 bytes; the budget fits one, not two
    let mut term = limit_term(
        LimitConfig {
            kitty_budget: 24,
            ..Default::default()
        },
        Replies::default(),
    );
    term.advance_bytes(
        format!("\x1b_Ga=T,t=d,f=32,s=2,v=2,i=1;{}\x1b\\", TINY_RGBA_BASE64).as_bytes(),
    );
    k9::assert_equal!(images_in(&term).len(), 1);
    // different pixels, so the image cache does not dedupe it
    let other = b64(&[0xff, 0, 0, 0xff, 0, 0xff, 0, 0xff, 0, 0, 0xff, 0xff, 1, 1, 1, 0xff]);
    term.advance_bytes(format!("\x1b_Ga=T,t=d,f=32,s=2,v=2,i=2;{}\x1b\\", other).as_bytes());
    let images = images_in(&term);
    k9::assert_equal!(images.len(), 1);
    k9::assert_equal!(images[0].len(), 16);
}

fn kitty_file_query(config: LimitConfig, path: &str) -> String {
    let replies = Replies::default();
    let mut term = limit_term(config, replies.clone());
    let seq = format!("\x1b_Ga=q,i=31,t=f;{}\x1b\\", b64(path.as_bytes()));
    term.advance_bytes(seq.as_bytes());
    replies.wait_apc()
}

#[test]
fn kitty_file_transfer_is_denied_by_default_without_revealing_files() {
    let existing = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let missing = concat!(env!("CARGO_MANIFEST_DIR"), "/definitely-missing-file");
    let ok = kitty_file_query(LimitConfig::default(), existing);
    let err = kitty_file_query(LimitConfig::default(), missing);
    // the reply must not depend on whether the file exists
    k9::assert_equal!(ok.clone(), err);
    assert!(ok.contains("EPERM"), "{:?}", ok);

    // t=t and t=s are refused the same way, and nothing is displayed
    for medium in ["t", "s"] {
        let replies = Replies::default();
        let mut term = limit_term(LimitConfig::default(), replies.clone());
        let seq = format!(
            "\x1b_Ga=T,i=32,t={medium},f=100;{}\x1b\\",
            b64(existing.as_bytes())
        );
        term.advance_bytes(seq.as_bytes());
        let text = replies.wait_apc();
        assert!(text.contains("EPERM"), "t={} {:?}", medium, text);
        assert!(images_in(&term).is_empty());
    }
}

#[test]
fn kitty_file_transfer_works_when_allowed() {
    let existing = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let reply = kitty_file_query(
        LimitConfig {
            allow_files: true,
            ..Default::default()
        },
        existing,
    );
    assert!(reply.contains(";OK"), "{:?}", reply);
}
