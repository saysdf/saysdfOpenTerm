# Review: pane-header (pane headers, patch to mux/src/tab.rs)

Date: 2026-10-18. Uncommitted working tree (`git diff -- mux wezterm-gui` + new files). Mode: read-only access to the sources; nothing was built or run.

Scope: `mux/src/tab.rs` (header_rows, `saysdf_leaf_size`, `saysdf_shift_positioned`, `Tab::saysdf_set_header_rows`), `wezterm-gui/src/termwindow/saysdf_pane_header.rs`, patch points (`render/paint.rs`, `mod.rs`, `saysdf_bars.rs`, `scripting/guiwin.rs`), `saysdf/lua/saysdf/{panes,close}.lua`, `panes_tests.rs`, the design of change `pane-header` and the executor's report; for context — `mux/src/domain.rs::split_pane`, `wezterm-mux-server-impl/src/sessionhandler.rs` (`Pdu::Resize`), `wezterm-client/src/pane/clientpane.rs::resize`, `mouseevent.rs::resolve_ui_item`, `mod.rs::close_current_pane`.

## Verdict: NEEDS FIXES

The D2 scheme (reserving a row in one place — where a leaf is handed its size) is implemented carefully: every `pane.resize` in `tab.rs` goes through `saysdf_leaf_size`, the arithmetic is guarded (`rows > header_rows`, `checked_div`, `saturating_sub`), with `header_rows = 0` the behavior is byte-for-byte upstream, and zoom/unzoom/remove/swap/rotate/cascade are consistent with `PositionedPane`. Security is fine: the header text goes through `sanitize_text` + `truncate_cells` + a character limit, only `pane_id` and a button name from a fixed set are passed to Lua, `pane_id`s are not reused, and closing goes through `saysdf.close`. But there are two high-severity functional bugs: (H1) with the header enabled, every new split loses a row — the split tree becomes inconsistent with the tab size (visible in the executor's own figures: the right column is 12 rows in a 13-row tab); (H2) the frame version is "baked into" cached elements — after any header change (focus change, title change) the `×`/`□` buttons on panes that were not redrawn stop responding.

## Findings

### H1 (High). `compute_split_size` computes the split from shifted geometry — a row is lost on every split

- `mux/src/tab.rs:1942` (`compute_split_size`, non-top_level branch): `self.iter_panes().iter().nth(pane_index)` — after the patch `iter_panes` returns a `PositionedPane` that is already shifted (`height = leaf rows − header_rows`, lines 1060-1076). Then `split_dimension(pos.height)` and `(pos.height, pos.height)` for a horizontal split use the height **minus the header**, and the result (`SplitDirectionAndSize`) is stored as the size of a tree node (`assign_node`, line 2088), where the full leaf height is expected.
- Consequence: a vertical split of a 24-row leaf with `header_rows = 1` yields a node `11 + 1 + 11 = 23`; a horizontal one yields children of 23 rows in a 24-row slot. One row at the bottom stays empty; later splits/`adjust_y_size` (a delta relative to `self.size`) do not compensate for the loss, and it accumulates with every further split. The first split in `splits` mode does not suffer (at the time of the split `header_rows` is still 0 — it is set only on the next frame), so the test `saysdf_header_layout_splits_and_zoom` (splits before `set_header_rows(1)`) and the two-pane snapshot do not see the bug. The executor's report (`header-1.md`, "Snapshots") records it: `4 + 1 + separator + 5 + 1 = 12` for a tab height of `12 + 1 = 13`.
- Called from `mux/src/domain.rs:96` (any `SplitPane`, including keys and `cli split-pane`) and from `split_and_insert` (`tab.rs:1986`); the size of the new pty for `spawn_pane` is also taken from here (`domain.rs:105`) — not critical, `split_and_insert` recomputes it, but already from the wrong node.

Fix (a line or two, in the branch `self.iter_panes().iter().nth(pane_index).map(|pos| { ... })`): restore the unshifted height — `let height = pos.height + pos.saysdf_header_rows;` and use `height` instead of `pos.height` in both `match` arms (the width does not change). Add a "split after `set_header_rows(1)`" scenario to the test, checking `node.first.rows + 1 + node.second.rows == slot height` and the tab total `1 + h + 1 + 1 + h' == rows`.

### H2 (High). The frame version is baked into cached elements — the buttons "die" after the first change of any header

- `wezterm-gui/src/termwindow/saysdf_pane_header.rs:293-296` — whenever `headers` differs from `last` (focus change → `active` on two panes, title change, zoom), `version += 1`. `CacheKey` (lines 226-232) does not contain the version, while `header_hit(..., version)` (lines 347-351, 358, 375-379) is baked into the element's `UIItemType` at `compute_element` time. Elements of panes whose key did not change stay in the cache with the **old** version, and their `ui_items()` (line 417) are published every frame with the old version.
- `saysdf_pane_header_mouse` → `header_click` (lines 171-174): `hit.version != current_version` → `None`. Result: in a tab with three panes, after the first focus switch the buttons of the third pane do not respond; with two panes, after a title change in one of them (any command in the shell) the buttons of the other stop working. It recovers only on a cache reset (`invalidate_cache`: mode change, atlas, font).
- The version protects nothing here: `ui_items` are rebuilt only on paint, and `pane_id`s are monotonic (another pane cannot be closed by mistake); the real protection is `pane_alive` + `Mux::get_pane`.

Fix (any option): (a) when `*state.last != headers`, clear `cache` entirely (the elements are recomputed with the new version; this is O(panes) layouts per event, not per frame); or (b) drop the version from the header hits (`version: 0`, do not compare in `header_click`) and keep the `pane_alive` + mux-presence check; or (c) include `version` in `CacheKey`. A test of the `saysdf_paint_pane_headers` logic is impossible without GL — at minimum, record the invariant in a comment and a unit test of `header_click` with the version from the key.

### M1 (Medium). `local` is checked only for the active pane — a mixed tab (ClientPane + LocalPane) toggles the headers on focus change

- `saysdf_pane_header.rs:282-285`: `local = the active pane is a LocalPane`. The function's documentation (lines 48-49) promises "all panes of the tab are local". A mixed tab is possible: `SplitPane { domain = "local" }` (or `DomainName`) from a remote tab goes through the shared `mux/src/domain.rs::split_pane` → `tab.split_and_insert` for any tab; also a `TermWizTerminalPane` (connection UI, not a `LocalPane`) in a split.
- Consequence: on every focus move between a local and a client pane `header_rows` flips 1↔0 and `saysdf_set_header_rows` recomputes the sizes of all leaves: SIGWINCH to all local ptys and `Pdu::Resize` to the server from `ClientPane::resize` (`clientpane.rs:399-417`); the server does `rebuild_splits_sizes_from_contained_panes` (`sessionhandler.rs:658`) and sends the tree back (`sync_with_pane_tree` → `resize`) — a recompute loop; on the server side the pty size changes by a row back and forth.

Fix: `local = tab.iter_panes_ignoring_zoom().iter().all(|p| p.pane.downcast_ref::<LocalPane>().is_some())` (cost O(panes), comparable to the already performed `count_panes`; the two can be combined into one traversal — take `iter_panes_ignoring_zoom()` once, then `len()` and `all()`). Decide explicitly whether `TermWizTerminalPane` counts as local (its size is set by the same mux, the header is safe) — otherwise focus on it also resizes the whole tab.

### M2 (Medium). GUI as a mux server: a client `Pdu::Resize` and `rebuild_splits_sizes_from_contained_panes` break the invariant "pty = rectangle − header"

- `wezterm-mux-server-impl/src/sessionhandler.rs:644-664`: `pane.resize(size)` bypassing `Tab` + `tab.rebuild_splits_sizes_from_contained_panes()`. The GUI process itself listens on a unix/TLS domain; a connected client receives the tree via `codec_pane_tree` (nodes are full sizes, leaf `PaneEntry.size` is the pty size without the header, `tab.rs:261-273`), builds its own `Tab` from the nodes and sends `Resize(pane, full rows)` → on the server the pty becomes one row taller than the drawn area (the last row is cut off until the next `tab.resize`). `rebuild_splits_sizes_from_contained_panes` (`tab.rs:1229-1270`) builds the tree from the leaves' `get_dimensions()` (without the header) — `self.size` and the nodes shrink by `header_rows` per leaf; the executor notes this under "Remainder" but considers it unreachable, which is wrong for a GUI server.
- The scenario is uncommon (a second window/client attached to the mux of a running GUI) and the design lists the multiplexer under Non-Goals, hence Medium.

Fix (minimal, in fork code): in `rebuild_splits_sizes_from_contained_panes`, for a leaf with `self.header_rows > 0 && dims.viewport_rows >= 1` add `self.header_rows` to `rows`/`pixel_height` (the inverse of `saysdf_leaf_size`); in `sessionhandler.rs` `Pdu::Resize` either call `tab.resize(tab.get_size())` after `rebuild` (it gives the leaves their reserved size by the same path), or document the limitation in design "Risks" and the README. At the very least, update the comment in the report/design: the case is reachable.

### L1 (Low). `count_panes()` via `try_lock`: under lock contention the header is switched off for a frame

- `saysdf_pane_header.rs:281`: `tab.count_panes().unwrap_or(0)`; `Tab::count_panes` (`tab.rs:563-565`) returns `None` if `inner` is busy. Then `should_show(.., 0, ..) = 0` → `saysdf_set_header_rows(0)` → all ptys are resized by a row, and the next frame puts it back (two SIGWINCHes, flicker). Background threads (the mux server, `cli list`) also take the lock; the probability is low, but the cost is a visible glitch.

Fix: on `None`, change nothing (`return`) instead of counting zero.

### L2 (Low). Closing/zooming via a button acts on the "current" pane, not on the `pane_id` from the hit

- `close.lua:104-110`, `panes.lua:53-56`: `pane:activate()` (synchronous, `lua-api-crates/mux/src/pane.rs:415`) and then `window:perform_action(CloseCurrentPane/TogglePaneZoomState, pane)`; `close_current_pane` (`mod.rs:3232-3243`) and zoom take the tab's active pane and ignore the `pane` argument. Between `activate()` and the handling of `TermWindowNotif::PerformAssignment` the main thread may process another event (a pane-switch key) — a different pane would be closed. The race window is a single event queue, so the probability is small; the `saysdf.close` rules are respected (`confirm = mode ~= "never"`, the upstream dialog with `can_close_without_prompting`).

Fix (optional): a fork action "close pane by id" (for example, a path through `saysdf_close` with the `pane_id` from the hit that does not use `EmitEvent`) — or leave it and record the assumption in a comment.

### L3 (Low). Rule 1: edits in `mux/src/tab.rs` are not marked `// saysdfTerminal:` line by line, the patch is wider than declared

- Only the struct fields (`tab.rs:49-50, 81-82`) and the block at the end of the file are marked. The other ~15 changed upstream lines (the signature and calls of `apply_sizes_from_splits`, `saysdf_leaf_size(...)` in `toggle_zoom`, `resize`, `cascade_size_from_cursor`, `remove_pane_if`, `split_and_insert`, `saysdf_shift_positioned` in `iter_panes_impl`) have no marker; the fork's rule 1 requires marking every change. The `saysdf_` prefix makes them greppable, but during a merge conflict it does not replace a comment. The design promised "2–3 lines"; in fact there are more patch points — this is justified (D2 requires all places where a size is handed out), but it should be reflected in design "Risks" and the upstream-sync notes.
- The `wezterm-gui` patches (`paint.rs:175,177,280`, `mod.rs:87,3541`, `guiwin.rs:130`) are marked correctly; the logic lives in fork files.

Fix: add a trailing `// saysdfTerminal: pane-header` on every changed line of `tab.rs` (or on the block) and update the design.

### I1 (Info). Two resizes on the 1→2 pane transition; a background tab gets the reservation when shown

- `header_rows` is set only for the active tab in `paint` (`paint.rs:175`): on the first split the ptys get a size without the header, and on the next frame with it (two SIGWINCHes). Background tabs keep the old value until shown. The behavior is expected by design, but worth mentioning in the README (full-screen TUIs will see an extra resize).

### I2 (Info). Performance is fine

- Per frame: `saysdf_sync_pane_header_rows` (three tab locks, two tree traversals), `collect` (`get_title` — the upstream cache, `get_current_working_dir(AllowStale)` only when the title is empty, which a `LocalPane` practically never has — `localpane.rs:449-455`), the `last != headers` comparison, `retain` O(n²) in the number of panes. `compute_element` — only for new keys; Lua — only on click. After the H2 fix with option (a), recomputing all headers will happen on every change (focus/title) — still an event, not a frame; the D4 budget is achievable.

## Checked and fine

- All `pane.resize` calls in `tab.rs` go through `saysdf_leaf_size`; `apply_pane_size`/`adjust_node_at_cursor`/`compute_min_size`/`adjust_x_size`/`adjust_y_size` work with tree nodes (full sizes) and do not touch the pty — the header is irrelevant to them. There is no double subtraction (`PositionedPane` is shifted from the full node sizes, the pty from the same nodes).
- Boundary heights: a 1-row leaf — no header and no shift (`saysdf_effective_header_rows`), 2 rows — header + 1 row; `rows == 0` is ignored by `resize`; `pixel_height` via `checked_div`/`saturating_sub`; no panics.
- Zoom: the zoomed pane gets the reservation in `toggle_zoom`/`resize`/`saysdf_set_header_rows`; leaving zoom — `resize(size_before_zoom)` → `apply_sizes_from_splits` with the reservation. A single leaf in `splits` mode: `header_rows = 0`, `saysdf_leaf_size` and `saysdf_shift_positioned` are the identity, `saysdf_set_header_rows(0)` on a new tab does nothing.
- Hit-testing: `UIItem` is searched from the end of the list (`mouseevent.rs:30-33`), headers are added after panes and dividers — the buttons take priority over `Split`; the header row is not part of the pane rectangle (`mouseevent.rs:709-710`); `pane_id`s are monotonic, `pane_alive` + `Mux::get_pane` before emitting the event; in Lua an unknown button is a no-op; close confirmation goes through `saysdf.close` (`confirm_for`), and the tests cover three modes.
- Terminal input into the header: `sanitize_text`, `truncate_cells`, `MAX_TEXT_CHARS`; the `parse_mode` error is also sanitized.

## What was not checked

- The build, `cargo test`, live behavior (the executor's snapshots are taken as data); correctness of the `box_model` rendering (`Float::Right`, `min/max_width`) and the cosmetics of the right edge (noted by the executor).
- Behavior of the tmux domain (`tmux_commands.rs:255` also calls `compute_split_size`, but `TmuxPane` is not a `LocalPane` → `header_rows = 0`).
- Interaction with other working-tree changes (file-browser, tool-sidebar, CLI) — out of scope.
- Dragging a divider down to a 1–2 row leaf height live (safe by the code).

## Fixed (2026-10-18, executor 2)

- **H1** — `compute_split_size`: leaf height `pos.height + pos.saysdf_header_rows` in both arms.
  Test `saysdf_header_split_after_header_enabled`: three scenarios of 3 splits each (H+V+V, V+V+H, V+V+V)
  after `set_header_rows(1)`; after each split the bottom edge equals the tab height, under every
  pane without gaps begins the neighbor's header, pty = rectangle, `resize(get_size())` changes
  nothing; the sum of rows in a column (headers + panes + dividers) = 24.
- **H2** — option (b): there is no version in the header hits (`header_hit(pane_id, button)`, `version: 0`),
  `header_click(hit, pane_alive)`; the `version` field was removed from `PaneHeaderState`. Protection:
  monotonic `pane_id`s, `pane_alive`, `Mux::get_pane`. Test `saysdf_header_cached_hit_survives_header_changes`.
- **M1** — `Tab::saysdf_try_census(pred)`: one traversal (`try_lock`) — the pane count and "all local".
  Local means `LocalPane` and `TermWizTerminalPane` (built-in UI, its size is set by the same mux;
  the header is safe, otherwise focus on it would resize the tab). A mixed tab gets 0.
- **M2** — fixed: `rebuild_splits_sizes_from_contained_panes` adds `header_rows` to a leaf
  (`saysdf_full_leaf_size`, the inverse of `saysdf_leaf_size`); `Pdu::Resize` in `sessionhandler.rs`
  gives the pty `saysdf_leaf_size(size, tab.saysdf_header_rows())`. Test — in the same
  `saysdf_header_split_after_header_enabled` (the tree built from pty sizes equals the original). The limitation
  (a leaf of exactly 1 row) is in design "Risks".
- **L1** — `try_lock` not acquired → `return` (header_rows does not change).
- **L2** — a new Lua window method `saysdf_pane_action(pane, "zoom"|"close", confirm)`: in one call of
  the GUI thread, `tab.set_active_pane(pane)` + a check that the pane became active + `toggle_zoom` /
  upstream `close_current_pane(confirm)` (the confirmation overlay and `can_close_without_prompting` are
  upstream; `confirm` follows the `saysdf.close` mode). `close.close_pane` and zoom in `panes.lua` use
  it. Lua tests: `close_button_uses_close_mode` (the pane from the hit is closed, `activate` is not called),
  `buttons_never_use_current_pane_actions`.
- **L3** — all changed upstream lines in `mux/src/tab.rs` are marked `// saysdfTerminal: pane-header`;
  design "Risks" and the upstream-sync notes ("Patch places … pane-header") list the places.
- Verification: `-Crate mux` 7 ok, `-Crate wezterm-gui -Filter saysdf_` 38 ok, `-Crate saysdf-cli` 80 ok,
  `-Crate saysdf-lua-bundle` 152 ok. The release build (`-Build`) and snapshots were not done: the build
  was stopped because of low memory; compilation of `wezterm-mux-server-impl` (M2) is not confirmed.
