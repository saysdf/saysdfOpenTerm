// saysdfTerminal: parser-fuzzing — cargo-fuzz target `kitty` (corpus: fuzz/corpus/kitty/)
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // half the inputs get the protocol prefix so libFuzzer reaches the image decoder fast
    if data.first().is_some_and(|b| b & 1 == 1) {
        let mut v = b"\x1b_G".to_vec();
        v.extend_from_slice(&data[1..]);
        saysdf_fuzz_support::feed_term(&v);
    } else {
        saysdf_fuzz_support::feed_term(data);
    }
});
