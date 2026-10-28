//! Shared entry points for parser fuzzing (change `parser-fuzzing`).
//!
//! The same `feed_*` functions are used by the cargo-fuzz targets in `fuzz/`
//! (Linux/WSL/CI, nightly) and by the deterministic garbage tests in `tests/`
//! (stable, any OS). Every function must return for any input without panicking.

use std::sync::Arc;
use wezterm_escape_parser::parser::Parser;
use wezterm_escape_parser::OperatingSystemCommand;
use wezterm_term::color::ColorPalette;
use wezterm_term::{Terminal, TerminalConfiguration, TerminalSize};

/// Deterministic xorshift64 generator: no external crates, fixed seeds give reproducible inputs.
#[derive(Debug, Clone)]
pub struct XorShift64(u64);

impl XorShift64 {
    pub fn new(seed: u64) -> Self {
        // the state must never be zero
        Self((seed ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform-ish value in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u64() as u8).collect()
    }
}

/// Split `bytes` into chunks of varying length (derived from the data itself), so that
/// sequences and multi-byte UTF-8 are cut at different boundaries.
pub fn chunks(bytes: &[u8]) -> Vec<&[u8]> {
    let seed = bytes
        .iter()
        .take(8)
        .fold(bytes.len() as u64, |acc, b| acc.rotate_left(8) ^ *b as u64);
    let mut rng = XorShift64::new(seed);
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let n = 1 + rng.below(rest.len().min(64));
        let (a, b) = rest.split_at(n);
        out.push(a);
        rest = b;
    }
    out
}

/// Escape-sequence parser (vtparse + wezterm-escape-parser: OSC/APC/DCS/Sixel/kitty parse),
/// actions are discarded.
pub fn feed_vt(bytes: &[u8]) {
    let mut parser = Parser::new();
    for chunk in chunks(bytes) {
        parser.parse(chunk, |action| drop(action));
    }
    // raw state machine with an empty actor, whole buffer at once
    let mut vt = vtparse::VTParser::new();
    vt.parse(bytes, &mut NullActor);
}

/// OSC payload (`8;...`, `52;...`, `1337;File=...` etc.): `OperatingSystemCommand::parse`
/// directly on `;`-split params, then the same payload framed as `ESC ] ... BEL` and `ESC ] ... ST`.
pub fn feed_osc(bytes: &[u8]) {
    let params: Vec<&[u8]> = bytes.split(|b| *b == b';').collect();
    let _ = OperatingSystemCommand::parse(&params);

    let mut framed = Vec::with_capacity(bytes.len() + 6);
    framed.extend_from_slice(b"\x1b]");
    framed.extend_from_slice(bytes);
    framed.push(0x07);
    framed.extend_from_slice(b"\x1b]");
    framed.extend_from_slice(bytes);
    framed.extend_from_slice(b"\x1b\\");
    feed_vt(&framed);
}

/// Full terminal model with kitty graphics enabled (kitty/iTerm2/Sixel image paths).
pub fn feed_term(bytes: &[u8]) {
    let mut term = new_terminal();
    for chunk in chunks(bytes) {
        term.advance_bytes(chunk);
    }
}

/// Terminal used by `feed_term` and by tests that need to inspect the result.
pub fn new_terminal() -> Terminal {
    new_terminal_with_writer(Box::new(std::io::sink()))
}

/// Same as `new_terminal`, but replies of the terminal (to the pty) go to `writer`.
pub fn new_terminal_with_writer(writer: Box<dyn std::io::Write + Send>) -> Terminal {
    Terminal::new(
        TerminalSize {
            rows: 24,
            cols: 80,
            pixel_width: 80 * 8,
            pixel_height: 24 * 16,
            dpi: 0,
        },
        Arc::new(FuzzTermConfig),
        "saysdfTerm",
        "fuzz",
        writer,
    )
}

/// Test configuration: small scrollback, kitty graphics on.
#[derive(Debug)]
pub struct FuzzTermConfig;

impl TerminalConfiguration for FuzzTermConfig {
    fn scrollback_size(&self) -> usize {
        100
    }

    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }

    fn enable_kitty_graphics(&self) -> bool {
        true
    }
}

struct NullActor;

impl vtparse::VTActor for NullActor {
    fn print(&mut self, _b: char) {}
    fn execute_c0_or_c1(&mut self, _control: u8) {}
    fn dcs_hook(&mut self, _b: u8, _p: &[i64], _i: &[u8], _x: bool) {}
    fn dcs_put(&mut self, _byte: u8) {}
    fn dcs_unhook(&mut self) {}
    fn esc_dispatch(&mut self, _p: &[i64], _i: &[u8], _x: bool, _b: u8) {}
    fn csi_dispatch(&mut self, _p: &[vtparse::CsiParam], _x: bool, _b: u8) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]]) {}
    fn apc_dispatch(&mut self, _data: Vec<u8>) {}
}

/// Map a fuzz target / corpus directory name to its feed function.
pub fn feed_for_target(target: &str) -> Option<fn(&[u8])> {
    match target {
        "vt_parser" => Some(feed_vt),
        "osc" => Some(feed_osc),
        "kitty" | "iterm" | "sixel" => Some(feed_term),
        _ => None,
    }
}
