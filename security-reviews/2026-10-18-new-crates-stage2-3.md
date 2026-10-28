# Supply-chain review: new crates for stage 2-3 (deps/stage2, commit 7445695dc)

Date: 2026-10-18. Mode: read-only access to sources in `%USERPROFILE%\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\`; nothing was built or run (no build.rs was executed).

## Verdict: SAFE TO BUILD (with informational notes)

None of the 98 new registry packages was found to have: network access or process spawning at build time in the active configurations, prebuilt binaries/object files, obfuscated blobs, writes outside OUT_DIR, or environment exfiltration. All packages come from crates.io, have a checksum, and are from known authors/organizations. Only 4 build.rs files are compiled for the Windows build: 2 are trivial, 1 is empty with our features, and 1 (libdbus-sys) is not built on Windows at all.

## 1. Provenance

- Cargo.lock delta: 99 new `[[package]]` entries with `source = "registry+https://github.com/rust-lang/crates.io-index"` and 99 `checksum`s. No git/path/alternate-registry sources (the only `path` ones are the 4 saysdf-* workspace crates). The `hyper 1.11.0` entry in the diff is a reordering of an existing block; the version did not change.
- Workspace: `resolver = "2"`; the new manifests saysdf/{render,cli,secrets,db} contain only dependencies, no build.rs.
- Authors/repositories (from each crate's Cargo.toml) match expectations: RustCrypto (aes, cbc, cipher, inout, block-buffer/padding, crypto-common, digest, sha2, md-5, hmac, hkdf, hybrid-array, cmov, ctutils, const-oid, zeroize_derive), ratatui org (ratatui*, kasuari, instability, line-clipping), microsoft/windows-rs (windows*), rust-postgres (sfackler), blackbeam (mysql, mysql_common, named_pipe), BurntSushi (csv), dtolnay (indoc), taiki-e (io-enum, derive_utils), diwic (dbus, libdbus-sys), hwchen/brotsky (keyring, secret-service, dbus-secret-service), madsmtm (objc2-*), ardaku (whoami, wasite).
- Typosquatting: none found. Unfamiliar names (btoi, saturating, by_address, castaway, cmov, ctutils, kasuari, wasite, bufstream, io-enum) are real transitive dependencies of well-known crates (see §5, "who pulls it in"), not lookalike fakes.
- Versions (from my own knowledge, without network access): everything looks like ordinary 2025–2026 releases (ratatui 0.30.x + the ratatui-core/widgets/crossterm split, the RustCrypto "0.11 generation" digest/sha2/hmac, darling 0.24, strum 0.28, derive_more 2.1, windows 0.61, sysinfo 0.37, mysql 26 / mysql_common 0.35). I cannot confirm exact publication dates; to verify, check `cargo info`/crates.io manually. No sign of "abandoned-and-revived" crates; old but stable ones: bufstream 0.1.4 (the only crate without `.cargo_vcs_info.json` — published by an old cargo, 262 lines, pure std), linked-hash-map 0.5.6, yaml-rust 0.4.5 (see §4).

## 2. Build scripts and proc-macros among the new packages

All 98 manifests were checked (`build = …`, `proc-macro = true`, presence of build.rs). Result:

| Package | Type | Verdict |
|---|---|---|
| mysql-26.0.1 | build.rs (18 lines) | OK. Only forwards `CARGO_CFG_TARGET_OS/ARCH` to `cargo:rustc-env` (`build.rs:12-17`). |
| mysql_common-0.35.5 | build.rs (14 lines) + `build_test_libs.rs` | OK with our features. `main()` calls `build_test_libs()` only under `cfg(all(feature="test", unix))` (`build.rs:12-13`); the module itself is entirely under `#![cfg(all(feature = "test", unix))]` (`build_test_libs.rs:9`). **Note:** with the `test` feature this module downloads MySQL sources with `curl -sL https://downloads.mysql.com/...` + `tar` (`build_test_libs.rs:28-34`) and builds them with cmake/cc/bindgen. The `test` feature is not enabled: its dependencies bindgen/cmake/subprocess are absent from Cargo.lock (mysql_common in the lock has only base64, bitflags, btoi, … uuid). Do not enable `test`/`nightly`. |
| pulldown-cmark-0.13.4 | build.rs (249 lines) | OK. Without the `gen-tests` feature the function is empty (`build.rs:7-8`); with it, it generates tests from local spec files in `./tests/suite/` (no network). The feature is not enabled (lock: bitflags, memchr, unicase). |
| libdbus-sys-0.2.7 | build.rs (31 lines) + `build_vendored.rs`, `links = "dbus"` | OK, not built on Windows (Linux/BSD only via keyring → dbus-secret-service → dbus). Without `vendored` it only does `pkg_config::probe("dbus-1")` (`build.rs:15`). `build_vendored.rs` is included only with `feature = "vendored"` (`build.rs:4-5`): it compiles the C sources from `vendor/dbus` with cc (5.4 MB of text, no binaries) and runs `git submodule update` if the directory is missing (`build_vendored.rs:349-353`). The `vendored` feature is not enabled (in the lock libdbus-sys has only pkg-config, no cc). On Linux the build will need the system `libdbus-1-dev`. |
| darling_macro-0.24.1 | proc-macro | OK, 40 lines, a wrapper over darling_core. |
| derive_more-impl-2.1.1 | proc-macro | OK. `forbid(unsafe_code)`; declares a build-dependency `rustc_version`, but `build = false` and there is no build.rs in the archive (a dead declaration; rustc_version ends up in the lock but the script is not executed). |
| indoc-2.0.8 | proc-macro | OK (dtolnay). Same: build-dep `rustversion` without a build.rs. |
| instability-0.3.14 | proc-macro | OK, pure syn/quote/darling. |
| io-enum-1.3.0 | proc-macro | OK, `forbid(unsafe_code)`; `fs::File` appears only in a doc example. |
| palette_derive-0.7.7 | proc-macro | OK; generates `unsafe impl ArrayCast`, which is expected for this crate. |
| strum_macros-0.28.0 | proc-macro | OK. Reads `STRUM_DEBUG` from the environment only to print the generated code (`src/lib.rs:23`); sends nothing. |
| zeroize_derive-1.5.0 | proc-macro | OK, `forbid(unsafe_code)`. |

No build.rs/proc-macro touches the network, writes outside OUT_DIR, or links `*.a/*.lib/*.so/*.dll/*.o`.

## 3. Binaries and opaque blobs

- A scan of all files of the 98 crates by extension (`.a .lib .so .dll .dylib .o .obj .exe .bin .wasm .dat .gz .zip …`) and by a "not text" heuristic found only RustCrypto test vectors (`tests/data/*.blb`, `*.bin`, up to 58 KB; they are not compiled into our binary, only into the crates' own tests) and syntect assets:
  - `syntect-5.3.0/assets/default_newlines.packdump` (368 KB), `default_nonewlines.packdump` (368 KB), `default_metadata.packdump` (5 KB), `default.themedump` (5 KB, zlib stream). These are bincode dumps of syntaxes/themes embedded via `include_bytes!` (`src/dumps.rs:172-216`) — the standard syntect mechanism (`default-syntaxes`/`default-themes`). I did not audit the dump contents byte by byte; they are deserialized by bincode 1.3.3 into data structures, not code.
- Lines > 1000 characters: only generated windows-rs bindings (`link!` macros, constants), a test in syntect html.rs, and a license in security-framework. No base64 blobs, no minified code.
- Large files: windows-0.61.3 (113 MB of generated bindings text; only the modules enabled by sysinfo's features are compiled), csv examples (`examples/data/*.csv`), objc2-io-kit generated (macOS).

## 4. Runtime behavior (worth knowing)

Severity is low/informational; nothing that violates the project rules by itself.

1. **mysql-26.0.1 — client attributes on connect** (`src/conn/mod.rs:805-825`): sends the MySQL server `_client_name=rust-mysql-simple`, the crate version, `_os`, `_platform`, `_pid` and `program_name = argv[0]` (the executable path). This is standard MySQL client behavior and goes only to the server the user connects to. If disclosing the binary path is undesirable, set `program_name` via `OptsBuilder::connect_attrs`. `DATABASE_URL`/`SSL`/`COMPRESS` are read only in test macros (`src/lib.rs:1001-1027`), not in production code. `RUST_MYSQL_BUFFER_*` is the buffer pool size.
2. **tokio-postgres-0.7.18 → whoami-2.1.3**: the OS user name is used as the default `user` when it is absent from the config (`src/connect_raw.rs:109`). whoami on Windows calls `GetUserNameW/GetUserNameExW/GetComputerNameExW`, on unix reads `LANG/LC_ALL/XDG_*`, `SSH_*` — no network. The default `sslmode = prefer` (`config.rs:255`) — for the DB viewer it is worth explicitly requiring `require` in settings.
3. **keyring-3.6.3**: on Windows only `CredReadW/CredWriteW/CredDeleteW` (`src/windows.rs:50-51, 163, 271, 299`); on macOS security-framework; on Linux dbus-secret-service via libdbus (D-Bus session bus). No network, no processes, no reading of other applications' secrets. Consistent with the rule "secrets only in the OS keychain".
4. **sysinfo-0.37.2** (features system, disk, network): on Windows ToolHelp/NtQueryInformationProcess, the registry (`RegOpenKeyExW`), `GetAdaptersAddresses`, COM `CoCreateInstance` (only for WMI in components — not our feature). The only process spawn is `taskkill.exe /PID … /F` inside `Process::kill_with` (`src/windows/process.rs:367`), i.e. only if project code explicitly calls kill. On macOS `sysctl` for the CPU. Expected for this crate.
5. **crossterm-0.29.0**: on unix a `tput cols/lines` fallback (`src/terminal/sys/unix.rs:274`) if ioctl fails; reads `TERM/COLORTERM/NO_COLOR`. Normal.
6. **secret-service-4.0.0 (+ zbus) is present in Cargo.lock, but the `async-secret-service` feature is not enabled**: it got into the lock because of the weak feature `secret-service?/crypto-rust` in keyring (known Cargo behavior: weak dependencies are recorded in the lock but not activated). I expect it not to be compiled; after the first build, confirm with `cargo tree -i secret-service` (pure resolver, no build). The crate itself was checked: no network or processes.
7. **yaml-rust-0.4.5** (via syntect `yaml-load`, part of `default-fancy`): the project is unmaintained (RUSTSEC-2024-0320, "unmaintained", not a vulnerability). Used only to parse `.sublime-syntax` when loading user syntaxes; if only the built-in dumps are loaded, `yaml-load` can later be disabled by assembling syntect's features manually.
8. **libdbus-sys / dbus / dbus-secret-service / secret-service / security-framework 2.11.1 / objc2-***: only for Linux/BSD/macOS/iOS targets, not compiled on Windows.

No self-initiated network connections (telemetry, update check) were found in any new crate. A grep for `TcpStream::connect/UdpSocket/reqwest/ureq/hyper/WinHttp/InternetOpen/URLDownload` outside tests yields only tokio-postgres (`connect_socket.rs:25`) and mysql (`io/tcp.rs:129`) — i.e. a connection to a DB at a user-specified address.

## 5. Version duplicates (informational, impact on size/build time)

New second/third versions that appeared because of this commit (who pulls them in):

- `digest 0.11.3`, `sha2 0.11.1`, `hmac 0.13.0`, `md-5 0.11.0`, `crypto-common 0.2.2`, `block-buffer 0.12.1` (+ new hybrid-array, ctutils, cmov, const-oid) — postgres-protocol 0.6.12 moved to RustCrypto 0.11; the rest of the workspace (including mysql_common, dbus-secret-service) is on `sha2 0.10.9`/`digest 0.10.7`. A second set of hashes in the binary, small in size.
- `hmac 0.12.1` ← hkdf 0.12.4 ← dbus-secret-service (Linux only).
- `lru 0.12.5` ← mysql; `lru 0.18.5` ← ratatui-core; `lru 0.16.4` was already there. Three versions of lru.
- `phf 0.13.1`/`phf_shared 0.13.1` ← tokio-postgres (was 0.11.3).
- `fancy-regex 0.16.2` ← syntect (was 0.14.0) — a second copy of the backtracking regex engine, noticeable in size.
- `darling 0.24.1` ← instability (was 0.23.0); `strum 0.28.0` ← ratatui (was 0.26.3) — proc-macros, affect build time, not size.
- `fallible-iterator 0.2.0` ← postgres (was 0.3.0).
- `windows 0.61.3` + `windows-core 0.61.2`, `windows-link 0.1.3`, `windows-result 0.3.4`, `windows-strings 0.4.2` ← sysinfo (were the 0.58/0.62 lines). The heaviest contribution to compile time on Windows.
- `security-framework 2.11.1` ← keyring (iOS target; not built on Windows), `whoami 2.1.3`/`wasite 1.0.2`/`wasi 0.14.7` ← tokio-postgres, `crossterm 0.29.0` is new (in the lock `crossterm 0.29.0` is the only version).
- `palette 0.7.7` + `approx` + `by_address` — in the lock because of the ratatui-core weak feature `palette`; the feature is not enabled, so they should not be compiled (same as §4.6).

## 6. What was NOT checked / limitations

- I did not read the ~98 crates line by line (hundreds of thousands of lines in total; windows-0.61.3 alone is 113 MB of generated code). Read in full: all 4 build.rs files and their helper modules (`build_vendored.rs`, `build_test_libs.rs`), the manifests of all 98 crates (build/proc-macro/links/build-dependencies/features), lock entries of key crates, and the code fragments around every grep hit.
- Grep coverage: process spawning (`process::Command`, `Exec::cmd`, `CreateProcess`, `ShellExecute`, `libc::exec*/fork/system/popen`), network APIs, URLs in code, `env::var`/`env!`, `include_bytes!/include_str!`, lines >1000 characters, binary files by extension and by the "not text" heuristic, `unsafe`/`asm!` in small little-known crates (castaway 58 unsafe — type casting as intended; cmov — inline asm for constant-time select; named_pipe 51 unsafe — a WinAPI wrapper). Grep misses code that builds strings/API names dynamically; I did not specifically look for such obfuscation beyond the check for long lines and base64.
- I did not compare the checksums in Cargo.lock with the directory contents (cargo does this itself when unpacking a `.crate`) and did not compare the sources with the upstream git repositories.
- I did not check publication dates or ownership history on crates.io (no network). The assessment "versions look ordinary" is based on my knowledge up to June 2026.
- I did not audit the contents of the syntect bincode dumps or the RustCrypto `.blb` test files.
- I did not systematically check cryptographic correctness/vulnerabilities (RUSTSEC) — only the one advisory known to me (yaml-rust unmaintained). Running `cargo audit`/`cargo deny` after the build is recommended.
- Linux/macOS-specific code (dbus, secret-service, security-framework, objc2-*) was only skimmed (grep), since the current target is Windows.

## Recommendations before/after the first build

1. It is fine to build. Do not enable the `test`/`nightly` features of mysql_common, `vendored` of keyring/libdbus-sys, or `gen-tests` of pulldown-cmark.
2. After the build: `cargo tree -i secret-service`, `cargo tree -i palette` — make sure the weak dependencies were not activated; `cargo audit`.
3. In saysdf-db set `program_name` in the mysql connect-attrs and `sslmode=require` by default for postgres (policy, not supply chain).
