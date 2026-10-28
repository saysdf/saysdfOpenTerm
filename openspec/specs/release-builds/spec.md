# release-builds Specification

## Purpose
Local build, testing, and packaging of saysdfTerm artifacts for Windows, Linux, and macOS, with verifiable checksums. By the owner's decision, GitHub CI is not used.

## Requirements

### Requirement: Local build and test
The repository SHALL provide a local entry point (`saysdf/tools/check.ps1 -Build` on Windows) that builds the workspace and runs its tests; Linux and macOS use the same cargo commands on their own hosts.

#### Scenario: Windows check
- **WHEN** `saysdf/tools/check.ps1 -Build` is run
- **THEN** the build and tests run, and on failure the exit code is nonzero

### Requirement: No GitHub CI
The fork SHALL NOT run any CI on GitHub: `.github/workflows/` MUST be empty or absent, upstream workflows MUST stay in `.github/workflows-upstream/`, and reference copies of the fork's workflows MUST live only in `saysdf/ci/github-workflows/`. None of these workflows SHALL use a `schedule` trigger.

#### Scenario: Workflow audit
- **WHEN** `.github/workflows/` is checked
- **THEN** it contains no files, and `bash saysdf/ci/check-pins.sh` passes for the reference workflows

### Requirement: Local packaging for three platforms
`saysdf/ci/package.sh` SHALL produce a zip for Windows, a tar.gz for Linux and a zip containing `saysdfTerm.app` for macOS from `target/release`, run on the corresponding OS. Binaries inside the archives MUST be named `saysdfTerm` (with `.exe` on Windows) and the companion binaries per the app identity.

#### Scenario: Windows packaging
- **WHEN** `package.sh` is run in Git Bash after a release build
- **THEN** a zip containing `saysdfTerm.exe` is created

#### Scenario: Linux packaging
- **WHEN** `package.sh` is run on Linux (WSL or a VM)
- **THEN** a tar.gz containing the `saysdfTerm` binary is created

### Requirement: SHA256 checksums
Every set of release archives SHALL be accompanied by a `SHA256SUMS` file with the SHA-256 checksum of every archive, verifiable with `sha256sum -c`.

#### Scenario: Verify archive
- **WHEN** the user has received an archive and `SHA256SUMS`
- **THEN** `sha256sum -c SHA256SUMS --ignore-missing` verifies the archive

### Requirement: Pinned third-party actions in reference workflows
Every `uses:` reference to an action outside the repository in `saysdf/ci/github-workflows/` SHALL be pinned to a full 40-character commit SHA, with the human-readable version in a trailing comment.

#### Scenario: Tag reference
- **WHEN** `uses: actions/checkout@v5` appears in a reference workflow
- **THEN** `bash saysdf/ci/check-pins.sh` fails

### Requirement: Secure defaults in shipped builds
Release builds SHALL be produced with the default feature set that satisfies the `security-defaults` capability; packaging MUST NOT enable features or configuration that weaken it.

#### Scenario: Released binary
- **WHEN** the user runs a release build without a config
- **THEN** update checks and file downloads via escape sequences are disabled
