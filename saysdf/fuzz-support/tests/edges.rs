//! parser-fuzzing 2.4: edge behaviour of the upstream terminal, pinned by tests.
//! Divergences from what we want are recorded in the fork's tech-debt list.

use saysdf_fuzz_support::new_terminal_with_writer;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Feed `bytes`, then wait (replies go through a writer thread) until `done` or timeout.
fn replies(bytes: &[u8], done: impl Fn(&str) -> bool) -> String {
    let cap = Capture::default();
    let mut term = new_terminal_with_writer(Box::new(cap.clone()));
    term.advance_bytes(bytes);
    let start = Instant::now();
    loop {
        let out = String::from_utf8_lossy(&cap.0.lock().unwrap()).into_owned();
        if done(&out) || start.elapsed() > Duration::from_secs(2) {
            return out;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn kitty_file_query(path: &str) -> String {
    let seq = format!("\x1b_Gi=7,a=q,t=f,s=1,v=1,f=24;{}\x1b\\", b64(path.as_bytes()));
    replies(seq.as_bytes(), |out| out.contains("\x1b\\"))
}

/// kitty `t=f` from program output is refused by default (change inline-graphics,
/// `allow_kitty_file_transfer = false`): the reply to `a=q` is the same for an existing and a
/// missing file, so output cannot probe the file system.
#[test]
fn kitty_t_f_is_refused_without_revealing_files() {
    let existing = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let missing = concat!(env!("CARGO_MANIFEST_DIR"), "/definitely-missing-file");
    let ok = kitty_file_query(existing);
    let err = kitty_file_query(missing);
    assert_eq!(ok, err);
    assert!(ok.contains("EPERM"), "reply: {ok:?}");
}

/// OSC 52 read request (`?`) is parsed as QuerySelection and ignored: no clipboard
/// contents are sent back to the pty.
#[test]
fn osc52_query_does_not_reply() {
    let out = replies(b"\x1b]52;c;?\x07\x1b[5n", |out| out.contains('n'));
    // only the DSR reply (CSI 0 n) to the marker request is expected
    assert_eq!(out, "\x1b[0n");
}

/// Found by the corpus test: iTerm2 `width=99999` (cells) with `height=auto` stretched an 11x11
/// PNG over ~99999 x 50000 cells and hung the terminal. Fixed in change inline-graphics
/// (target size and placement cells are bounded).
#[test]
fn iterm_huge_cell_width_does_not_hang() {
    const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAsAAAALCAYAAACprHcmAAAACXBIWXMAAAGKAAABigEzlzBYAAAAOUlEQVQYlZXOwQ0AMAzCQEdi7yaT0xWAN7JuDCac2PQKYxflycOoICOKtPIuqFCg4/LzKxiz6xjyAYh9DR1sLUN1AAAAAElFTkSuQmCC";
    let start = Instant::now();
    let seq = format!("\x1b]1337;File=inline=1;width=99999;height=auto:{PNG_B64}\x07ok");
    let mut term = saysdf_fuzz_support::new_terminal();
    term.advance_bytes(seq.as_bytes());
    assert!(start.elapsed() < Duration::from_secs(5));
}
