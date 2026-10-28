// saysdfTerminal: parser-fuzzing — cargo-fuzz target `vt_parser` (corpus: fuzz/corpus/vt_parser/)
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    saysdf_fuzz_support::feed_vt(data);
});
