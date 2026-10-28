// saysdfTerminal: parser-fuzzing — cargo-fuzz target `osc` (corpus: fuzz/corpus/osc/)
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    saysdf_fuzz_support::feed_osc(data);
});
