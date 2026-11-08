# Security review addendum: crossterm Windows base-layout key from the virtual key code (task K-1, change `text-annotations`)

Date: 2026-11-07. Addendum to `2026-10-19-crossterm-vendor.md`. Mode: read-only; nothing was built or run
(cargo was not invoked, the new tests were not executed). Reviewed commit `32324f70a`
("text-annotations K-1: Windows physical keys via VK (unbuilt, pending review)"). Context: design decision D13 of the text-annotations change (physical keys on Windows).

## Scope

- `saysdf/vendor/crossterm/src/event/sys/windows/parse.rs`: new `base_layout_for_vk(u16) -> Option<char>`,
  its single call in `parse_key_event_record`, and a `#[cfg(test)]` module.
- `saysdf/vendor/crossterm.patch`: regenerated with the Windows hunks added.
- `saysdf/cli/src/keys.rs`: `phys()` / `kitty_mod_bits()` enabled on Windows; one new Windows-only test.
- `saysdf/keymap.md` note (documentation only, not reviewed for content).

## Verdict

**APPROVED** (no conditions blocking the build; see informational remarks and the recommendations).

The delta is a pure, total function of the `wVirtualKeyCode` field of a key record that the crate already
reads, plus one field assignment on an event that is already constructed. No `unsafe`, no new imports,
no new dependencies, no allocation, no panic path for any `u16`; the paths for Alt codes, surrogates, numpad
Alt entry, modifier-only keys and mouse events return before the new code and are byte-identical to
upstream. The patch file matches the vendored sources exactly, and the previously reviewed hunks are unchanged.

## 1. The patch file versus the vendored sources and upstream

- The crates.io tarball `~/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/crossterm-0.29.0.crate`
  still has sha256 `d8b9f2e4c67f833b660cdb0a3523065869fb35570177239812ed4c905aeff87b` (equals the
  `checksum` of the `crossterm 0.29.0` entry in `Cargo.lock`, which now resolves through the `[patch.crates-io]`
  path dependency, `Cargo.toml:302-303`). It was unpacked into a fresh scratch directory, not taken from
  `registry/src`.
- `diff -rq --strip-trailing-cr <tarball> saysdf/vendor/crossterm`: exactly three files differ
  (`src/event.rs`, `src/event/sys/unix/parse.rs`, `src/event/sys/windows/parse.rs`); the only other
  difference is the tarball's `Cargo.lock`, ignored by the crate's own `.gitignore` (`**/Cargo.lock`) and
  irrelevant for a path dependency inside the workspace. The `.cargo-ok` marker flagged as I1 in the previous
  review is gone (76 tracked files).
- A `diff -u` of the three files regenerated from the CR-stripped tarball against the committed (index)
  copies is **identical** to the committed `crossterm.patch` body once the three `diff`/`---`/`+++` header lines
  per file are ignored. No hidden hunks: the patch contains exactly three file sections (lines 1, 68, 149).
- Previous patch (`32324f70a~1`) versus new patch: `diff` shows only two removed lines (the `+++` mtime stamps
  of `unix/parse.rs` and `event.rs`, 2026-10-19 → 2026-10-20) and the inserted Windows section (lines 68–148,
  81 lines). The `unix/parse.rs` and `event.rs` hunks are byte-for-byte the ones reviewed on 2026-10-19.

## 2. The code

**`src/event/sys/windows/parse.rs:291-293`** (`parse_key_event_record`). The call sits inside
`if let Some(key_code) = parse_result`, i.e. only on the branch that upstream already turns into a
`KeyEvent`. Everything that returns earlier is untouched: the Alt-code release path (`:210-232`, including the
surrogate return), the numpad-with-Alt suppression (`:235-240`), the `VK_SHIFT|VK_CONTROL|VK_MENU => None`
arm, the surrogate return inside the `_` arm (`:271-273`). `handle_key_event`, `handle_surrogate`,
`get_char_for_key` (the two existing `unsafe` FFI calls) and `parse_mouse_event_record` are unchanged.
`key_code`, `modifiers` and `kind` are computed exactly as before; the only observable difference is
`KeyEvent.base_layout`, which `PartialEq`/`Hash` ignore (see the 2026-10-19 report), so existing `==`
comparisons and `match` arms in ratatui and fork code behave identically.

**`base_layout_for_vk` (`:300-319`).** A `match` on `u16` with a `_ => None` arm; the two
`char::from_u32` calls take values in `0x30..=0x39` and `0x61..=0x7A`, which are always valid scalar values
(the function cannot return `None` for them, and cannot panic). The mapping covers `VK_SPACE`, `VK_0..9`,
`VK_A..Z` (lower-cased) and the eleven `VK_OEM_*` keys that exist on the US layout, each to its unshifted US
character; `VK_OEM_102` (ISO `<>` key) and everything else (numpad, F-keys, arrows, modifiers, IME keys,
browser/media keys, `0x00`, `0xFF`) → `None`, which makes `phys()` fall back to the typed character exactly as
today. Numeric literals instead of `winapi` constants keep the `use` block untouched (smaller delta, rule 1);
I checked the values against `winuser.h`: all fourteen are correct.

**Test module (`:403-437`).** `#[cfg(test)]`, Windows-only like the parent module; covers every mapped code
and a set of unmapped ones. Not executed in this review.

**`saysdf/cli/src/keys.rs`.** `phys()` is unchanged in body; it is now compiled on `any(unix, windows)` and
feeds the base character and the crossterm modifier bits to the existing `saysdf_keys::from_kitty`
(`saysdf/keys/src/lib.rs:142`), which only accepts ASCII, lower-cases it and maps through
`KeyCode::to_phys()` (`' '` → `PhysKeyCode::Space`, `wezterm-input-types/src/lib.rs:222`). One
implementation for both OSes, no copy (rule 2). The `cfg(not(any(unix, windows)))` stub stays for other
targets. The two Windows-only tests are consistent with the mapping.

Rule 1 marking: every changed or added line in the vendored file carries `// saysdfTerminal: base-layout key`.

## 3. Security impact of reporting a base-layout key

**Source of the data.** `KEY_EVENT_RECORD.wVirtualKeyCode` arrives in the same record, from the same
`ReadConsoleInputW` call, as the `uChar` and `dwControlKeyState` fields the crate already trusts for
`KeyCode::Char` and the modifiers. Whoever can put a record into the process's console input buffer (the
terminal/ConPTY on behalf of the user, or another process of the same user via `WriteConsoleInput`)
already controls the typed character and the Ctrl/Alt/Shift bits; the new field adds no information that
record could not already express. It is a pure function of the record, never read from the output side,
never logged, never persisted.

**Terminal output → input.** Terminal output cannot write to the console input buffer except through
query responses (DSR, DA, XTVERSION, OSC 52 reads) that the terminal sends back. Those responses are printable
bytes plus `ESC` and never carry `CONTROL`, and fork TUI output of untrusted content goes through
`saysdf_render::sanitize` (`pager.rs:22`), so untrusted file/DB/process text cannot emit a query in the first
place. After D13 every viewer command is a Ctrl combination, which a response cannot produce; the plain-letter
pager navigation keys (`/`, `n`, `q`, `]`…) were already matched by typed character before this change.
Net: no new path by which untrusted output triggers a TUI action (rule 3 holds).

**Widening of matching.** The only semantic change is intended by D13: on Windows a key record whose VK is
`VK_R` with Ctrl now matches `"Ctrl+R"` regardless of the character `ToUnicodeEx` returns for the active
layout (Cyrillic ka, U+043A, on Russian). On a US layout the same record already matched, so the set of records that trigger a
command is not enlarged for an attacker, only made layout-independent for the user. A record with `base_layout`
`Some(..)` takes `Pressed::Phys` precedence over `Pressed::Letter` (`keys.rs:58-64`); exact equality of
modifiers is required, so AltGr combinations (`CTRL|ALT` on Windows) cannot satisfy a `Ctrl+X` spec.

**Release events.** `base_layout` is also set on `KeyEventKind::Release` records (same as the Unix patch).
All fork consumers filter on `Press`/not-`Release` before dispatch (`pager.rs:439`, `form.rs:143`,
`files/mod.rs:651`, `mon/ui.rs:544`, `db/ui.rs:2148`, `cmd/diff.rs:505`); no change here.

## 4. Correctness risks (not security; for the orchestrator)

| # | Severity | What |
|---|---|---|
| R1 | Info | **"Physical" on Windows is the VK, not the scan code.** Windows layouts assign VKs per key as printed: on AZERTY the key labelled A has `VK_A`, on Windows Dvorak the key at the QWERTY-O position has `VK_R`. So a Dvorak user presses the key *labelled* R for `Ctrl+R`, while the kitty base-layout key on Linux/macOS is the QWERTY *position*. The executor flagged this. For Latin layouts the Windows behaviour matches the printed label, which is arguably what users expect; it only diverges from Unix on remapped-Latin layouts. If uniformity is wanted later: `MapVirtualKeyExW(virtual_scan_code, MAPVK_VSC_TO_VK_EX, <US layout>)`, a separate task and a further patch. |
| R2 | Info | **ConPTY without win32-input-mode synthesizes the VK from the character** (`VkKeyScanW` in the console's own layout). For `0x01..0x1a` control bytes conhost produces `VK = byte + 0x40` + Ctrl, which is layout-free, so Ctrl commands work (this is the case saysdfTerm's GUI exercises: Ctrl + Cyrillic Ka is sent as `0x12`). For printable characters the VK is a reconstruction: a character that the console layout cannot produce gives `VK 0` → `None` → the old typed-character fallback (no regression); a character that exists at a different position in the console layout than in the GUI layout yields a different "physical" key than the one pressed. Bounded to keys that can produce that character; the by-eye check in K-1 covers the main configurations. |
| R3 | Info | **Behaviour change for non-US layouts on Windows, intended by D13:** punctuation and letters are now matched by key position / VK, not by typed character, as on Unix with kitty. Example: on the Russian layout `/` is typed as Shift+`\` (`VK_OEM_5`+Shift → `Backslash+SHIFT`), which no longer matches the pager's `"/"`; the physical `/` key (which types `.`) does. Same as Linux/macOS today. Worth one line in `keymap.md`/help if not already there. |
| R4 | Info | **Pasted control bytes under ConPTY** become synthesized Ctrl+letter records (pre-existing conhost behaviour; crossterm already decoded them as `Char+CONTROL`). With D13 they now match Ctrl commands on any layout. Paste is a user action and the viewer does not take destructive actions without further confirmation; noted for completeness, not introduced by this patch. |

## 5. Line endings of the patch file

- Published crate: all three sources have CRLF. Git index (and therefore any Linux/macOS checkout): both the
  vendored sources and `crossterm.patch` are stored **LF-only** (`core.autocrlf=true` on this machine, no
  `.gitattributes`; `git ls-files --eol` reports `i/lf` for all four files). Working tree on Windows: sources
  CRLF; the patch is mixed (195 CRLF lines from the sources, 20 LF header/hunk lines, 0 bare CR) and
  normalizes to the committed LF-only content (`diff` after `tr -d '\r'`: identical). This is the same state
  as the 2026-10-19 commit, not something K-1 introduced; the previous report's "CRLF preserved" statement
  described the non-git working copy of that day.
- Consequence (not security): the committed patch applies cleanly to the committed (LF) vendored tree and to a
  CR-stripped tarball, but not directly to a freshly unpacked CRLF tarball; `patch --ignore-whitespace` or
  stripping CRs first is needed. Recommend adding that one line to the crossterm upgrade steps in the private upstream-sync notes.

## 6. What was NOT checked / limitations

- Nothing compiled; the test module and `keys.rs` tests were not run. The Rust in the test uses inline format
  args and by-value array iteration (edition 2021, `rust-version = "1.63.0"` in the crate manifest), which the
  workspace toolchain supports; type inference of the `(u16, char)` tuples relies on the call site, which looks
  fine but is a compiler question.
- The by-eye checks listed in K-1 (Windows Terminal / conhost / saysdfTerm with the Russian layout) are
  acceptance work for the orchestrator, outside this review.
- `saysdf/keymap.md` content was not reviewed.

## 7. Conditions / recommendations

None blocking. Recommended, non-blocking:
1. After the build, run the vendored test `virtual_key_code_gives_us_layout_char` and the two `keys.rs`
   Windows tests, and perform the K-1 by-eye checks (Ctrl + Cyrillic Ka, Cyrillic hard sign → `]`, Shift+arrows) before accepting K-1.
2. Upstream-sync notes, crossterm section: add the line-ending note from section 5 and the fact that the patch now
   has three file sections (the next crossterm upgrade must re-apply the Windows hunk too).
3. If AZERTY/Dvorak uniformity with Unix (R1) ever matters, open a separate task; do not extend this patch
   ad hoc.
