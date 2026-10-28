//! Deterministic garbage-input tests (stable, any OS). Fixed seeds, fixed number of inputs.
//! Every `feed_*` must return without panicking.

use saysdf_fuzz_support::{feed_osc, feed_term, feed_vt, XorShift64};
use std::time::{Duration, Instant};

const PREFIXES: &[&[u8]] = &[
    b"",
    b"\x1b]",            // OSC
    b"\x1b_",            // APC
    b"\x1b_G",           // kitty graphics
    b"\x1bP",            // DCS
    b"\x1bPq",           // Sixel
    b"\x1b[",            // CSI
    b"\x1b]1337;File=",  // iTerm2 image
    b"\x1b]52;c;",       // clipboard
    b"\x1b]8;;",         // hyperlink
];

const TERMINATORS: &[&[u8]] = &[b"", b"\x07", b"\x1b\\", b"\x9c", b"\x18", b"\x1b"];

const ALPHABET: &[u8] = b" ;:=,0123456789abcdefqGa?!#~-$\"/+";

fn input(rng: &mut XorShift64) -> Vec<u8> {
    let mut v = PREFIXES[rng.below(PREFIXES.len())].to_vec();
    let len = rng.below(256);
    let body = rng.bytes(len);
    // half the inputs are restricted to printable/sequence-ish bytes to get deeper into parsers
    if rng.below(2) == 0 {
        v.extend(body.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()]));
    } else {
        v.extend(body);
    }
    v.extend_from_slice(TERMINATORS[rng.below(TERMINATORS.len())]);
    v
}

#[test]
fn garbage_vt_and_osc() {
    let start = Instant::now();
    for seed in 0..1500u64 {
        let mut rng = XorShift64::new(seed);
        let data = input(&mut rng);
        feed_vt(&data);
        feed_osc(&data);
    }
    // budget from design.md (generous for debug builds on slow machines)
    assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
}

#[test]
fn garbage_terminal_with_images() {
    for seed in 10_000..10_500u64 {
        let mut rng = XorShift64::new(seed);
        feed_term(&input(&mut rng));
    }
}

#[test]
fn truncated_and_huge_parameters() {
    feed_vt(b"\x1b[999999999999999999999999;-1;4294967296m");
    let many: Vec<u8> = std::iter::repeat(b"1;".as_slice()).take(1000).flatten().copied().collect();
    feed_vt(&[b"\x1b[".as_slice(), &many, b"m"].concat());
    feed_osc(&many); // > 64 OSC params
    feed_osc(b"1337;File=size=99999999999999;inline=1:");
    feed_osc(b"52;c;?");
    feed_osc(b"8;id=x;");
    // UTF-8 split everywhere, unterminated sequences
    feed_term("\x1b]0;\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442} \u{43c}\u{438}\u{440}\x07\u{442}\u{435}\u{43a}\u{441}\u{442}".as_bytes());
    feed_term(b"\x1b_Ga=T,f=32,s=4294967295,v=4294967295,m=1;AAAA\x1b\\");
    feed_term(b"\x1b_Ga=T,f=32,o=z,s=2,v=2;bm90IHpsaWI=\x1b\\");
    feed_term(b"\x1bPq\"1;1;99999;99999#0!99999~\x1b\\");
}
