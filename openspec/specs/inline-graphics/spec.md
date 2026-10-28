# inline-graphics Specification

## Purpose
Display images inline in the terminal through supported image protocols and open images by click. Size, memory, placement, transfer media and escape-sequence length are all bounded to resist hostile output.

## Requirements

### Requirement: Image protocols support
The terminal SHALL display images using the kitty graphics (APC `_G`), iTerm2 (OSC 1337 `File=`) and Sixel (DCS `q`) protocols on Windows, Linux and macOS. Support MUST be covered by automated tests of image parsing and ingestion for each protocol.

#### Scenario: Small image over each protocol
- **WHEN** a valid 2x2 pixel image is written to the terminal using the kitty, iTerm2 or Sixel protocol
- **THEN** the image is attached to screen cells and the terminal does not panic

### Requirement: Image size limit
The terminal SHALL reject an image whose width×height exceeds `max_image_pixels` (configurable, 33 177 600 by default — 7680×4320) before decoding the pixels, for all three protocols. A rejected image MUST NOT be displayed and MUST NOT occupy memory for decoded pixels; the terminal keeps working.

#### Scenario: Image too large
- **WHEN** a program outputs a PNG with a 20000×20000 header using the iTerm2 protocol
- **THEN** the image is not displayed, the log contains a message about the limit being exceeded, and the terminal stays responsive

#### Scenario: Limit from config
- **WHEN** `config.max_image_pixels = 1000000` and a program outputs a 1200×1000 image
- **THEN** the image is rejected

### Requirement: Kitty image memory budget
The memory occupied by stored kitty images SHALL be limited by `kitty_image_memory_budget` (configurable, 320 MB by default). When it is exceeded, images with no placements are removed first; if the budget is still exceeded after that, the new image MUST be rejected.

#### Scenario: Budget overflow by placed images
- **WHEN** the budget is 1 MB and a program uploads and places images totalling more than 1 MB
- **THEN** the image that does not fit in the budget is rejected, and previously placed ones remain

### Requirement: Image viewing command
The command `saysdfTerm-cli imgcat <file>` SHALL output an image fitted to the pane size, preserving the aspect ratio. With the `--hold` flag the command SHALL, after output, wait for a key press and only then exit, so that the split with the image does not close immediately.

#### Scenario: Viewing in a split
- **WHEN** `saysdfTerm-cli imgcat --hold shot.png` is run in a new pane
- **THEN** the image is fitted to the pane, and the pane stays open until a key is pressed

### Requirement: Open image by click
A click on an image path (`png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`) SHALL open it in a split via `saysdfTerm-cli imgcat --hold` (implemented by the `saysdf.paths` module, change `clickable-file-paths`).

#### Scenario: Click on a screenshot
- **WHEN** the user Ctrl+clicks `out/shot.png` in the output
- **THEN** a split with the image opens

### Requirement: Image placement size limit
The size of an image placement in cells SHALL be limited: no more than `max(4 × screen width, 1000)` columns and `max(4 × screen height, 1000)` rows, for all protocols (including explicit kitty `c=`/`r=`). For iTerm2, the requested target size (`width=`/`height=`) in pixels MUST pass the same `max_image_pixels` check as the source image. A placement larger than the limit MUST be rejected quickly, without hanging and without allocating memory for the target size.

#### Scenario: iTerm2 width=99999
- **WHEN** a program outputs `ESC ] 1337 ; File=inline=1;width=99999;height=auto:<PNG 11x11> BEL`
- **THEN** the image is not placed, and the terminal responds and processes the following output in under 5 seconds

### Requirement: Kitty transfer media restricted
By default the terminal SHALL accept kitty images only with direct data transmission (`t=d`). The transmissions `t=f` (file), `t=t` (temporary file) and `t=s` (shared memory) from program output MUST be rejected until the user explicitly enables `allow_kitty_file_transfer = true`. The terminal's reply to a request with a forbidden transmission medium MUST be the same for any path and MUST NOT reveal whether the file exists.

#### Scenario: File existence probe via a=q
- **WHEN** with default settings a program sends `a=q,t=f` with the path of an existing file and with the path of a nonexistent one
- **THEN** both replies are identical (`EPERM:file transfer disabled`), and the file is not opened

#### Scenario: Explicitly enabled
- **WHEN** `config.allow_kitty_file_transfer = true` and a program sends `a=q,t=f` with the path of a readable file
- **THEN** the terminal replies `OK`

### Requirement: DCS length limit
DCS data that the terminal accumulates in full (DECRQSS, Sixel, XTGETTCAP) SHALL be limited by the same ceiling as OSC/APC (`vtparse::MAX_SEQUENCE_BYTES`, 64 MiB). A longer sequence MUST be discarded without processing; the next sequence is parsed normally. Streaming DCS (passthrough, tmux control mode) are not accumulated and are not subject to the ceiling.

#### Scenario: Long unterminated Sixel
- **WHEN** a program outputs a Sixel DCS longer than the ceiling
- **THEN** no image is created, memory does not grow beyond the ceiling, and the next DCS is processed
