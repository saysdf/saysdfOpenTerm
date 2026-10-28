# public-release Specification

## Purpose
Publish the fork as an English public repository by releases only: the public tree is exported from the private repository by a manifest with an overlay, a leak scan blocks a release with internal files, Cyrillic text or references to private paths, each release is one commit with a version tag on top of the upstream WezTerm history, and bootstrap scripts build and install the terminal from source.

## Requirements

### Requirement: Public tree is defined by a manifest
The set of files published in the public repository SHALL be the tree of the private `HEAD`
minus the paths excluded in `saysdf/public/manifest.toml`, plus the files of the overlay directory
named in the manifest, which are placed at the repository root and replace files with the same path.
The overlay directory itself MUST NOT be published at its private path. File contents, file modes
(including the executable bit) and submodule entries MUST be published unchanged.

#### Scenario: Internal files are not published
- **WHEN** a release is exported
- **THEN** the public tree contains no `AGENTS.md`, `CLAUDE.md`, `.claude/`, `UPSTREAM.md`, `saysdf/work/`, `saysdf/tech-debt.md`, `openspec/changes/`, `.github/workflows-upstream/` or `saysdf/ci/github-workflows/`

#### Scenario: Overlay replaces an upstream file
- **WHEN** the overlay contains `CONTRIBUTING.md`
- **THEN** the public `CONTRIBUTING.md` has the overlay's content and `saysdf/public/overlay/` is absent from the public tree

#### Scenario: Executable bit and submodules survive the export
- **WHEN** a release is exported
- **THEN** `get-deps` has mode 100755 and `deps/harfbuzz/harfbuzz` is a submodule entry with the same commit as in the private tree

### Requirement: Leak scan blocks a release
Before any commit is created, the release script MUST scan the exported tree and fail without
creating a commit or tag when any of these is found: a path under `.github/workflows/`; a file named
`.env` or `.env.*`; a file containing `BEGIN ... PRIVATE KEY`; and, in files that differ from the
upstream WezTerm commit the release is based on, a file larger than the manifest's size limit
(5 MB), a run of two or more Cyrillic letters outside the manifest's Cyrillic allow-list globs,
or a match of any regular expression from a private deny-list. The deny-list file MUST live outside
the repository (default `~/.config/saysdfTerm/public-denylist.txt`, one regex per line, `#` comments);
a missing deny-list MUST fail the release. Every hit MUST be reported with its path and line.

#### Scenario: Russian comment in fork code
- **WHEN** a fork file outside the allow-list contains a Lua comment written in Russian (a word of two or more Cyrillic letters)
- **THEN** the release fails and names the file and line

#### Scenario: Translation catalog is allowed
- **WHEN** the manifest allow-list contains `**/locales/ru*` and `saysdf/locales/ru/tabs.toml` contains Russian words
- **THEN** the scan does not report that file

#### Scenario: Layout key example is allowed
- **WHEN** a fork file mentions the key `E/У` (a single Cyrillic letter)
- **THEN** the scan does not report it

#### Scenario: Private marker
- **WHEN** the deny-list contains a regex matching a private host name and a fork file contains that host
- **THEN** the release fails and the message shows the path and line but the deny-list stays outside the repository

### Requirement: Release history in the public repository
The release script SHALL create exactly one commit per release in a local clone of the public
repository, with the exported tree, the message `saysdfTerm vX.Y.Z` followed by the changelog
section for that version from `saysdf/public/CHANGELOG.md`, and an annotated tag `vX.Y.Z`. The
first parent MUST be the previous public release commit or, in an empty clone, the upstream WezTerm
commit the private repository was last merged with (`git merge-base HEAD upstream/main`), so the
public history is the WezTerm history plus release commits. When that upstream commit is not yet in
the public history, the release commit MUST be a merge with it as the second parent. The script
MUST refuse to run with uncommitted changes in the private tree, an unclean public clone, an
existing tag, or a missing changelog section, and it MUST NOT push; it prints the push commands.
`-DryRun` MUST run the export and scan and print the plan without writing to the public clone.

#### Scenario: First release
- **WHEN** `release-public.ps1 -Version 0.1.0 -PublicRepo <empty clone>` runs
- **THEN** the clone's branch has one new commit whose parent is the merged upstream commit, tag `v0.1.0` exists, and the script prints `git push` commands without running them

#### Scenario: Upstream merged since the last release
- **WHEN** the private repository merged a newer upstream commit after release 0.1.0 and `-Version 0.2.0` runs
- **THEN** the release commit has the 0.1.0 commit as first parent and the new upstream commit as second parent

#### Scenario: Missing changelog
- **WHEN** `CHANGELOG.md` has no section for the requested version
- **THEN** the script fails before creating a commit

### Requirement: Bootstrap builds and installs from source
The repository SHALL provide `bootstrap/bootstrap.ps1` (Windows) and `bootstrap/bootstrap.sh`
(Linux, macOS) that print every step, check prerequisites (git, Rust via rustup, on Windows Perl
and the MSVC C++ build tools, on Linux the system packages via `get-deps`, on macOS the Xcode
command line tools), ask before installing anything (`-Yes` / `--yes` to skip questions), never
run `sudo` without saying so, and never pipe a remote script into a shell without naming it. They
SHALL use the checkout they are run from, or clone `https://github.com/saysdf/saysdfOpenTerm.git` with
submodules (default `~/src/saysdfTerm`, ref from `--ref`, default the latest `v*` tag), build in
release mode with a job count that leaves at least 10% of RAM and CPU free at low priority, and
install for the current user without admin rights: Windows `%LOCALAPPDATA%\Programs\saysdfTerm` with
a Start menu shortcut; Linux `~/.local/lib/saysdfTerm` with links in `~/.local/bin`, a `.desktop`
file and an icon; macOS `~/Applications/saysdfTerm.app`. They MUST support uninstall, which never
removes the user's configuration, and a dry run that only prints the planned actions. At the end they
MUST print where things were installed, how to start the terminal and the config path
`~/.config/saysdfTerm/saysdfTerm.lua`.

#### Scenario: Dry run from a checkout
- **WHEN** `bootstrap.ps1 -DryRun` runs inside a checkout
- **THEN** it prints the prerequisite checks, the build command with the job limit and the install destination, and changes nothing

#### Scenario: Missing Rust, user declines
- **WHEN** `cargo` is not found and the user answers "no" to installing rustup
- **THEN** the script prints how to install Rust manually and exits without building

#### Scenario: Uninstall keeps the config
- **WHEN** `bootstrap.sh --uninstall` runs
- **THEN** the install directory, links, `.desktop` file and icon are removed and `~/.config/saysdfTerm/` is untouched
