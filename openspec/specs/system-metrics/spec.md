# system-metrics Specification

## Purpose
Show system metrics such as CPU and memory in the status bar, with a full monitor available in a split pane. The collector runs only while the metrics are shown.

## Requirements

### Requirement: Status bar metrics toggle
The terminal SHALL provide a key binding (default `Ctrl+Shift+I`, configurable or disableable via `setup({ monitor = {...} })`) that toggles compact CPU, RAM, network and disk figures in the right status area. Metrics MUST be collected outside the GUI thread; the `update-status` handler MUST only read the last collected sample and MUST NOT spawn processes or block.

#### Scenario: Enable metrics
- **WHEN** the user presses the metrics hotkey
- **THEN** after 1-2 intervals the status bar shows `CPU 12% RAM 7.1/16G ↓1.2M ↑40K D 3M/s`

#### Scenario: Collector missing or stale
- **WHEN** the collector failed to start or its file has not been updated for more than 3 intervals
- **THEN** the status bar shows `mon: n/a`, the UI does not hang, and the reason is written to the log

### Requirement: Collector lifetime
The background collector (`saysdf mon --status-file`) SHALL exit when metrics are toggled off, when the terminal exits, or when its parent PID disappears. At most one collector MUST run per GUI process.

#### Scenario: Terminal closed
- **WHEN** the terminal is closed while metrics are enabled
- **THEN** the collector process exits within one interval

### Requirement: Full monitor in a split
The terminal SHALL provide a key binding (default `Ctrl+Shift+J`) that opens `saysdf mon` in a split of the current tab. The monitor SHALL show history graphs (braille) for per-core CPU, memory and swap, network, disk and, where the OS provides data, GPU. When GPU data is unavailable the GPU graph MUST be hidden with a note, not shown as zero.

#### Scenario: Open monitor
- **WHEN** the user presses `Ctrl+Shift+J`
- **THEN** a split opens with the graphs and a process list; `q` closes it

#### Scenario: No GPU data
- **WHEN** the OS or driver does not expose GPU load
- **THEN** `GPU: unavailable` is shown instead of the GPU graph
