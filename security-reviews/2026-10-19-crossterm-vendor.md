# Security review: vendored crossterm 0.29.0 with the base-layout key patch (task 7.2, change `layout-independent-keys`)

Date: 2026-10-19. Mode: read-only access to `saysdf/vendor/crossterm/`, `saysdf/vendor/crossterm.patch`
and a fresh unpack of the `.crate` from the registry; nothing was built or run (including the new
patch test). Context: design D7 of change
`layout-independent-keys` and the executor's report of task 7.1.

## Verdict

Verdict: OK to build

The copy matches the published crossterm 0.29.0 (the tarball's sha256 equals the checksum in `Cargo.lock`),
exactly two files differ, and their diff is byte-for-byte equal to `crossterm.patch`. The patch only parses an
already received and terminated `CSI … u` sequence and adds a new field `KeyEvent.base_layout`;
there are no new reads/writes, `unsafe`, dependencies or build scripts, the parsing limits are not relaxed,
and the behavior of existing callers does not change. The remarks below are informational, plus one
non-security build blocker for step 8.

## 1. The copy versus the published crate

- `~/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/crossterm-0.29.0.crate`:
  sha256 `d8b9f2e4c67f833b660cdb0a3523065869fb35570177239812ed4c905aeff87b`, equal to the `checksum`
  of the `crossterm 0.29.0` entry in `Cargo.lock:1330`. The source is `registry+https://github.com/rust-lang/crates.io-index`,
  the only crossterm version in the graph (pulled in by `ratatui-crossterm 0.1.2`).
- The tarball was unpacked into a fresh `mktemp -d` (I did not trust `~/.cargo/registry/src`): 77 files.
  `diff -rq` against `saysdf/vendor/crossterm/`: only `src/event.rs` and
  `src/event/sys/unix/parse.rs` differ; the extra file in the copy is `.cargo-ok` (see remark I1).
- `diff -ruN <unpacked> saysdf/vendor/crossterm` after removing the `.cargo-ok` hunk is byte-for-byte equal to
  the body of `saysdf/vendor/crossterm.patch` (134 lines, +42/−5). The CRLF line endings of both files are preserved.
- The copy's `Cargo.toml` is identical to the published one: `build = false`, no build.rs, no proc-macro,
  the same dependencies and features as in `Cargo.lock`. `.cargo_vcs_info.json`:
  `36d95b26a26e64b0f8c12edfe11f410a6d56a812`.

## 2. The patch

**`src/event/sys/unix/parse.rs:598-620`** (`parse_csi_u_encoded_key_code`).

- The input is an already terminated buffer: `parse_csi` (`parse.rs:192-203`) calls this function only when
  the last byte is in 64..=126 and equals `u`; before that it returns `Ok(None)` and keeps accumulating. The patch does not touch this loop.
- The new code is a closure `next_alternate` with two calls: `codepoints.next()` over the already created lazy
  `split(':')` (`parse.rs:519-521`), `parse::<u32>().ok()` (overflow and an empty field are `Err` → `None`,
  without allocation), `char::from_u32` (surrogates and >0x10FFFF → `None`). No panics, no loops, no new
  allocations; the amount of work is bounded by the length of the already accepted buffer.
- Behavior for existing callers: previously the alternates were parsed only with SHIFT; now they are
  parsed always, but `code`/`modifiers` change in the same `if modifiers.contains(SHIFT)` branch
  under the same condition. No new `Err`s (alternate parse errors are still swallowed into `None`).
  The result for `code`, `modifiers`, `kind`, `state` on any input matches upstream.
- The new test `test_parse_csi_u_encoded_base_layout_key` (`parse.rs:1318-1337`): four cases,
  including an empty shifted field (`1085::121`) and the absence of a base codepoint. Not run.

**`src/event.rs:968-1079`.**

- The field `pub base_layout: Option<char>`, `None` in `new`, `new_with_kind`, `new_with_kind_and_state`,
  `From<KeyCode>`; in `PartialEq` and `Hash` it is `base_layout: _`. `normalize_case` takes `self` by
  value (`Copy`), and the field is carried over unchanged.
- All `KeyEvent {` constructions in the crate's live code (`event.rs:591,603,615`) use `..` and compile.
  The Windows parser builds events only via `KeyEvent::new*` (`event/sys/windows/parse.rs:57,228,291`),
  so the Windows build is not broken by the patch.

**Assessment of `PartialEq`/`Hash` without `base_layout`.** `Eq` and `Hash` are consistent (both via
`normalize_case`, both ignore the field). Equality is not widened: `Char('н')` with `base_layout = Some('y')`
is still not equal to `Char('y')`, so code that checks confirmations via `==` or
`key.code == KeyCode::Char('y')` sees exactly the upstream behavior. A widening of matching
("physical Y confirms on any layout") appears only where fork code itself reads
`base_layout` (`saysdf-keys::from_kitty`, D7) — that is the user's decision, recorded in design.md as a
non-goal. There is no substitution inside the crate itself. Additionally:
- `PartialOrd` of `KeyEvent` is derived (`event.rs:936`) and now takes `base_layout` into account, whereas
  `PartialEq` does not. The inconsistency existed upstream before the patch (`PartialOrd` ignores
  `normalize_case`); `Ord` is not implemented, so the type cannot be a `BTreeMap` key. Informational.
- The field is present on `Release`/`Repeat` events too: fork code that reads `base_layout` must filter
  `kind == Press` itself (for step 9).

## 3. The whole crate (brief, at the level of earlier reports)

- Build: `build = false`, no build.rs, no proc-macro, no `include_bytes!/include_str!`, no binary
  files. The tarball carries `.github/workflows/crossterm_test.yml`, `.travis.yml`, `CODEOWNERS` —
  inert in a subdirectory (GitHub reads only the root `.github/workflows/`).
- `unsafe`: 17 places, all thin FFI wrappers. Unix: `ioctl(TIOCGWINSZ)`, `cfmakeraw`, `tcgetattr`,
  `tcsetattr` (`terminal/sys/unix.rs:78,302,307,316`), `isatty` (`tty.rs:33,41`), `read`/`close`/
  `isatty` on a descriptor (`terminal/sys/file_descriptor.rs:52,101,125`). Windows:
  `GetKeyboardLayout`/`ToUnicodeEx` (`event/sys/windows/parse.rs:162,168`), `WaitForMultipleObjects`
  (`poll.rs:54`), `SetConsoleTitleW`/`GetConsoleTitleW` (`terminal/sys/windows.rs:236,465`),
  `SetConsoleCursorPosition`/`SetConsoleCursorInfo` (`cursor/sys/windows.rs:148,167`).
- Processes: the only spawn is `tput cols|lines` as a fallback if `TIOCGWINSZ` fails
  (`terminal/sys/unix.rs:274`, looked up via `PATH`); already noted in the 2026-10-18 report.
- Network: none. Environment: only `TERM`, `COLORTERM`, `NO_COLOR` (`ansi_support.rs:40`, `style.rs:174-175`,
  `style/types/colored.rs:76`). Filesystem: opening `/dev/tty` for read-write when stdin is not a tty
  (`terminal/sys/file_descriptor.rs:129-133`).

## 4. Remarks

| # | Severity | Where | What |
|---|---|---|---|
| I1 | Info | `saysdf/vendor/crossterm/.cargo-ok` | A cargo unpack marker (`{"v":1}`, 7 bytes) absent from the published crate — it came from `cp -a` of a `registry/src` directory. Harmless, but it makes `diff -r` against the tarball unclean and regenerating the patch yields an extra hunk. Remove it. |
| L1 | Low, upstream, outside the patch | `src/event/source/unix/mio.rs:199-216`, `tty.rs:248-266` | An unterminated CSI (`ESC [` + digits/`;`/`:` without a final byte) accumulates in `Parser.buffer` (a `Vec`) without a cap until a byte in 64..=126 arrives. The writer to the tty is a terminal emulator or a process with the user's privileges, not remote content; this is upstream behavior and the patch does not change it. If a cap is wanted — a separate `// saysdfTerminal:` patch (reset the buffer when `len() > N`), not an extension of this one; for now, an entry in the fork's tech-debt list at the orchestrator's discretion. |
| L2 | Low, not security, step 8 blocker | `saysdf/cli/src/db/ui.rs:2067` (test `ev`) | The exhaustive literal `KeyEvent { code, modifiers, kind, state }` will not compile with the patched crate (no `base_layout`). Replace it with `KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind)`. `ratatui-crossterm-0.1.2` has no exhaustive literals; crossterm's own `examples/` are not built as a path dependency. |
| I2 | Info | `src/event.rs:935` | If the `serde` feature is enabled (not active in the lock; crossterm has no serde in the lock), a serialized `KeyEvent` gets the new field; deserializing old data without `#[serde(default)]` will fail. Do not enable `serde` without extending the patch. |

## 5. What was NOT checked / limitations

- Nothing was compiled or run; the new patch test was not executed — that is step 8.1.
- The Windows part of the crate was reviewed with grep and by reading the `unsafe` sites, not line by line.
- The directory `C:/Users/me/dev/saysdfTerminal` on this machine is not a git checkout; whether
  `saysdf/vendor/` is tracked by git and whether a `.gitignore` covers it — check in the main checkout at commit time.
- The sources were not compared with the upstream crossterm repository by the sha from `.cargo_vcs_info.json` (no network);
  the comparison was made against the crates.io tarball by the checksum from `Cargo.lock`.

## 6. Recommendations before step 8

1. Remove `saysdf/vendor/crossterm/.cargo-ok`; after that `diff -r <unpacked .crate> saysdf/vendor/crossterm`
   should show exactly two files, and `diff -ruN` should match `crossterm.patch`.
2. In step 8.1, together with `[patch.crates-io] crossterm = { path = "saysdf/vendor/crossterm" }`
   (the section already exists, `Cargo.toml:297`), fix `saysdf/cli/src/db/ui.rs:2067` (L2) and check that
   in `Cargo.lock` only the `source` of `crossterm` changed (to path, without `checksum`) and no other
   entry moved.
3. In step 7.3 (upstream-sync notes) record: the version, sha256, the two patched files, the command to regenerate
   the patch, and that when crossterm is updated only the version delta plus applying the patch is re-reviewed.
