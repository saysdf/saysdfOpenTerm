# Parser fuzzing (cargo-fuzz)

Change `parser-fuzzing`. Linux/WSL only: libFuzzer with a sanitizer is not supported on Windows
(MSVC). The cheap tests in `saysdf/fuzz-support/tests/` run on any OS
(garbage input from fixed seeds + a run over `fuzz/corpus/` and `fuzz/regressions/`):
`./saysdf/tools/check.ps1 -Crate saysdf-fuzz-support`.

`fuzz/` is a separate package with its own `[workspace]`: the root workspace and its `Cargo.lock` do not see it.
The target logic lives in the `feed_*` functions of `saysdf/fuzz-support` (shared with the tests).

| Target | Function | What it parses |
|------|---------|---------------|
| `vt_parser` | `feed_vt` | vtparse + wezterm-escape-parser (CSI/OSC/APC/DCS) |
| `osc` | `feed_osc` | `OperatingSystemCommand::parse` (OSC 8/52/1337 ...) |
| `kitty` | `feed_term` | `Terminal`, APC `_G` (kitty graphics, `o=z`, chunks) |
| `iterm` | `feed_term` | `Terminal`, OSC 1337 `File=` |
| `sixel` | `feed_term` | `Terminal`, DCS `q` |

## Running in WSL / Linux

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked --version 0.12.0
cd fuzz
mkdir -p work-corpus/kitty && cp corpus/kitty/* work-corpus/kitty/   # keep the repo corpus clean
cargo +nightly fuzz run kitty work-corpus/kitty -- -max_total_time=60 -rss_limit_mb=4096
```

Run it manually on Linux/WSL (there is no CI on GitHub); the reference workflow is
`saysdf/ci/github-workflows/fuzz.yml`.

## A crash was found

```sh
cargo +nightly fuzz tmin kitty artifacts/kitty/crash-<hash>
cp <minimized file> regressions/kitty/<description>.bin
```

The test `saysdf/fuzz-support/tests/corpus.rs` runs `regressions/` on any OS. Put the fix in the same
commit (or add an entry to the fork's tech-debt list and an `#[ignore]` test). Synthetic data only.
