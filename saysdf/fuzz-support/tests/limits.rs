//! Length caps added by `parser-fuzzing` (vtparse MAX_SEQUENCE_BYTES), checked end-to-end
//! through the terminal: an oversized OSC/APC is dropped and the following text is printed.

use saysdf_fuzz_support::new_terminal;

fn first_line(term: &wezterm_term::Terminal) -> String {
    let screen = term.screen();
    let lines = screen.lines_in_phys_range(0..1);
    lines[0].as_str().trim_end().to_string()
}

fn oversized(prefix: &[u8], fill: u8, terminator: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    // == vtparse::MAX_SEQUENCE_BYTES (literal, so the test also builds against unpatched vtparse)
    v.resize(prefix.len() + 64 * 1024 * 1024 + 1, fill);
    v.extend_from_slice(terminator);
    v.extend_from_slice(b"ok");
    v
}

/// 64 MiB through a debug-build parser takes a while, so this is opt-in:
/// `cargo test -p saysdf-fuzz-support --release -- --ignored`
#[test]
#[ignore]
fn oversized_osc_is_dropped_and_text_printed() {
    let mut term = new_terminal();
    term.advance_bytes(oversized(b"\x1b]2;", b'a', b"\x07"));
    assert_eq!(first_line(&term), "ok");
    // unpatched vtparse dispatched it: the title became the 64 MiB string
    assert!(term.get_title().len() < 1024, "oversized OSC was dispatched");
}

#[test]
#[ignore]
fn oversized_apc_is_dropped_and_text_printed() {
    let mut term = new_terminal();
    term.advance_bytes(oversized(b"\x1b_Ga=T,f=32,s=1,v=1;", b'A', b"\x1b\\"));
    assert_eq!(first_line(&term), "ok");
}

#[test]
fn cap_matches_vtparse() {
    assert_eq!(vtparse::MAX_SEQUENCE_BYTES, 64 * 1024 * 1024);
}

#[test]
fn normal_osc_still_works() {
    let mut term = new_terminal();
    term.advance_bytes(b"\x1b]2;title\x07ok");
    assert_eq!(first_line(&term), "ok");
    assert_eq!(term.get_title(), "title");
}
