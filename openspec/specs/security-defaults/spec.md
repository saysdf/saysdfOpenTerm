# security-defaults Specification

## Purpose

Secure defaults of the fork: the terminal does not access the network
and does not accept files from programs unless the user explicitly enables it,
and it does not load unsigned binaries shipped with it.

## Requirements

### Requirement: No update checks by default
The terminal SHALL NOT make network requests to check for updates unless
the user sets `check_for_updates = true` in the configuration.

#### Scenario: Default configuration
- **WHEN** the terminal is started without a user config
- **THEN** there are no requests to api.github.com or other update servers

#### Scenario: User opts in
- **WHEN** the user sets `check_for_updates = true`
- **THEN** the terminal checks for updates at the interval `check_for_updates_interval_seconds`

### Requirement: No file downloads via escape sequences by default
The terminal SHALL ignore file transfer escape sequences (OSC 1337 File)
unless the user sets `allow_download_protocols = true`.

#### Scenario: Remote program sends a file
- **WHEN** a program in the terminal (including one on a remote server) sends OSC 1337 File
- **AND** `allow_download_protocols` is not enabled
- **THEN** the file is not written to disk

### Requirement: No bundled unsigned graphics libraries
The terminal SHALL NOT ship or load the prebuilt ANGLE, Mesa or MetalANGLE
libraries; on Windows and macOS it SHALL render through the system OpenGL
implementation (WGL, CGL).

#### Scenario: Windows startup
- **WHEN** the terminal starts on Windows
- **THEN** the system `opengl32.dll` from System32 is loaded
- **AND** `libEGL.dll` is not searched for in PATH or next to the exe

#### Scenario: macOS startup
- **WHEN** the terminal starts on macOS
- **THEN** CGL is used and `libEGL.dylib` is not loaded
