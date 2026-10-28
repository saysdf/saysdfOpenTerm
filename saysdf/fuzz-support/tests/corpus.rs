//! Runs every file in `fuzz/corpus/<target>/` and `fuzz/regressions/<target>/` through the
//! matching `feed_*` function, so crashes found by cargo-fuzz stay fixed on any OS.

use saysdf_fuzz_support::feed_for_target;
use std::path::Path;

fn run_dir(kind: &str) -> usize {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz").join(kind);
    let Ok(targets) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut count = 0;
    for target in targets.flatten() {
        let name = target.file_name().to_string_lossy().into_owned();
        let Some(feed) = feed_for_target(&name) else {
            continue;
        };
        for file in std::fs::read_dir(target.path()).unwrap().flatten() {
            if !file.path().is_file() {
                continue;
            }
            let data = std::fs::read(file.path()).unwrap();
            eprintln!("{kind}/{name}/{}", file.file_name().to_string_lossy());
            feed(&data);
            count += 1;
        }
    }
    count
}

#[test]
fn corpus_files_do_not_panic() {
    assert!(run_dir("corpus") >= 25, "fuzz/corpus is missing or too small");
}

#[test]
fn regression_files_do_not_panic() {
    run_dir("regressions");
}
