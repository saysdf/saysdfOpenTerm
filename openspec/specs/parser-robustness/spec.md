# parser-robustness Specification

## Purpose
Ensure that terminal parsers and decoders survive arbitrary or hostile input without crashing or exhausting resources. This is achieved with fuzz targets, a seed corpus, regression tests and hard limits on sequence length and decompressed size.

## Requirements

### Requirement: Garbage input robustness
The escape sequence parsers (`vtparse`, `wezterm-escape-parser`) and the graphics intake in `term` (kitty, iTerm2, Sixel) SHALL handle an arbitrary byte sequence without panicking, hanging, or unbounded memory growth. This MUST be verified by automated tests on a deterministic set of garbage inputs, run with `cargo test` on Windows, Linux, and macOS without nightly Rust.

#### Scenario: Random bytes
- **WHEN** pseudorandom bytes (fixed seed) are fed to the parser in blocks of varying length
- **THEN** the parser returns control, there is no panic, and the test is reproducible

#### Scenario: Truncated sequence
- **WHEN** the input breaks off in the middle of an OSC, APC, DCS, or CSI and is followed by plain text
- **THEN** the parser returns to its initial state and parses the following text correctly

### Requirement: Fuzz targets
The repository SHALL contain cargo-fuzz targets in `fuzz/` for: the VT/escape parser, OSC (8, 52, 1337), kitty graphics (APC `_G`), iTerm2 (OSC 1337 `File=`), and Sixel (DCS `q`). The targets MUST run on Linux (including WSL) with nightly Rust; the regular workspace build and `cargo test` MUST NOT depend on `fuzz/` or nightly.

#### Scenario: Run a target
- **WHEN** `cargo +nightly fuzz run <target> -- -max_total_time=60` is run on Linux
- **THEN** the target builds, takes its seed corpus from the repository, and runs until the timeout or until it finds a crash

#### Scenario: Regular build unaffected
- **WHEN** `cargo test --workspace` is run on Windows without nightly
- **THEN** `fuzz/` is not part of the build and the tests pass

### Requirement: Seed corpus and regressions
The repository SHALL store a small seed corpus (at most 1 MB in total, synthetic data only) and regression tests. Each crash found by fuzzing MUST get a regression test with a minimized input, which runs in `cargo test`, and a fix (or an entry in the fork's tech-debt list if the fix falls outside rule 1).

#### Scenario: Corpus as a test
- **WHEN** `cargo test` is run for a crate with a corpus
- **THEN** every corpus file is run through the parser without a panic

#### Scenario: Crash found
- **WHEN** fuzzing finds an input that causes a panic
- **THEN** the minimized input is added to the regression tests, and the test fails until the fix lands

### Requirement: Sequence length limits
The parser SHALL limit the total size of the data of a single OSC and APC sequence (and of accumulated DCS data). When the limit is exceeded, the sequence MUST be discarded as a whole (not dispatched partially), the memory freed, and parsing of subsequent input continued. The limit MUST be covered by a test that feeds input larger than the limit.

#### Scenario: Giant APC
- **WHEN** a program outputs an APC `_G` with data larger than the limit, followed by `ESC \` and text
- **THEN** the sequence is discarded, memory does not grow beyond the limit, and the text is output

#### Scenario: Giant OSC
- **WHEN** a program outputs OSC 52 or 1337 with data larger than the limit
- **THEN** no action is performed and the terminal keeps working

### Requirement: Decompression limit
Decompression of kitty graphics data (`o=z`) SHALL be bounded in output size by a limit derived from the maximum image size (at most width×height×4 bytes) and a fixed ceiling. An image whose decompression exceeds the limit MUST be rejected without allocating memory beyond the limit.

#### Scenario: Zlib bomb
- **WHEN** a kitty image `o=z` is sent in which a few kilobytes of compressed data expand to gigabytes
- **THEN** the image is rejected, peak memory is bounded by the limit, and the terminal stays responsive

### Requirement: Manual fuzzing workflow
The repository SHALL contain a GitHub Actions workflow that runs fuzzing only manually (`workflow_dispatch`) on Linux with the inputs "target" and "time". The workflow MUST NOT run on a schedule or on push, MUST have `permissions: contents: read`, and MUST publish any crashes found as a run artifact.

#### Scenario: Manual run
- **WHEN** a developer runs the workflow with a target and a time of 300 seconds
- **THEN** fuzzing runs on Linux, and if it crashes, the input is available as an artifact

#### Scenario: No automatic run
- **WHEN** a push happens or the scheduled time arrives
- **THEN** fuzzing does not start
