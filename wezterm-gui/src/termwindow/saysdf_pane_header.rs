//! saysdfTerminal: pane headers (change pane-header, D1–D4).
//!
//! The row above each pane is reserved by the mux (`Tab::saysdf_set_header_rows`, D2): the pty size
//! of a leaf and `PositionedPane.top/height` already account for the header. Here: the decision "whether to show",
//! row layout (title + buttons `□`/`▣` and `×`), painting on the bars machinery
//! (`saysdf_bars.rs`: `sanitize_text`, `truncate_cells`, `BarColors`, `SaysdfBarHit`)
//! and click handling → event `saysdf-pane-header-click(window, pane, button)` in Lua
//! (`saysdf.panes`). Lua is not called per frame; ready elements are cached and reset
//! together with the bars cache (`BarsState::invalidate_cache` — atlas reset, garbled glyphs).

use crate::termwindow::box_model::*;
use crate::termwindow::saysdf_bars::{
    colors, sanitize_text, text_cells, truncate_cells, BarColors, BarKind, BarSide, SaysdfBarHit,
};
use crate::termwindow::{GuiWin, UIItem, UIItemType};
use ::window::{MouseEvent, MouseEventKind as WMEK, MousePress, WindowOps};
use config::{Dimension, DimensionContext};
use mux::pane::PaneId;
use mux::tab::PositionedPane;
use mux::Mux;
use mux_lua::MuxPane;
use std::cell::RefCell;
use std::collections::HashMap;

/// Header mode (option `saysdf.panes.header`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderMode {
    /// Only when the tab has more than one pane.
    #[default]
    Splits,
    Always,
    /// No headers — upstream behavior.
    Never,
}

pub fn parse_mode(s: &str) -> anyhow::Result<HeaderMode> {
    match s {
        "splits" => Ok(HeaderMode::Splits),
        "always" => Ok(HeaderMode::Always),
        "never" => Ok(HeaderMode::Never),
        other => anyhow::bail!(
            "saysdf_set_pane_header: unknown mode `{}` (splits|always|never)",
            sanitize_text(other)
        ),
    }
}

/// How many header rows to set on a tab: `local` — all panes of the tab are local
/// (`LocalPane` or `TermWizTerminalPane`, see `is_local_pane`; for multiplexer domains
/// the server computes the size — no header, design "Risks").
/// The height of each leaf (`rows >= 2`) is additionally checked by the mux.
pub fn should_show(mode: HeaderMode, pane_count: usize, local: bool) -> usize {
    if !local || pane_count == 0 {
        return 0;
    }
    match mode {
        HeaderMode::Never => 0,
        HeaderMode::Always => 1,
        HeaderMode::Splits => usize::from(pane_count > 1),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HeaderButton {
    Title,
    Zoom,
    Close,
}

impl HeaderButton {
    pub fn as_str(self) -> &'static str {
        match self {
            HeaderButton::Title => "title",
            HeaderButton::Zoom => "zoom",
            HeaderButton::Close => "close",
        }
    }
}

pub const ZOOM_TEXT: &str = "□";
pub const UNZOOM_TEXT: &str = "▣";
pub const CLOSE_TEXT: &str = "×";
/// Button width in cells: the symbol with a space on each side.
pub const BUTTON_CELLS: usize = 3;
/// Minimum width at which both the title and both buttons are shown.
pub const MIN_FULL_CELLS: usize = 2 * BUTTON_CELLS + 2;

/// Header row layout in pane cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderLayout {
    /// Title text (with a space on the left), already truncated; occupies cells `0..title_cells`.
    pub title: String,
    pub title_cells: usize,
    /// Buttons: (button, text, first cell, cell after the last).
    pub buttons: Vec<(HeaderButton, &'static str, usize, usize)>,
}

/// Layout: title on the left (truncated with `…`), buttons pushed right. A narrow pane — only
/// `×`; a very narrow one (< 3 cells) — nothing.
pub fn header_layout(title: &str, width: usize, zoomed: bool) -> HeaderLayout {
    let title = sanitize_text(title);
    let title = title.trim();
    let mut buttons = vec![];
    let mut right = width;
    if width >= BUTTON_CELLS {
        buttons.push((HeaderButton::Close, CLOSE_TEXT, right - BUTTON_CELLS, right));
        right -= BUTTON_CELLS;
    }
    if width >= MIN_FULL_CELLS {
        let text = if zoomed { UNZOOM_TEXT } else { ZOOM_TEXT };
        buttons.push((HeaderButton::Zoom, text, right - BUTTON_CELLS, right));
        right -= BUTTON_CELLS;
    }
    buttons.reverse();
    // a space left of the title and at least one cell before the buttons
    let avail = right.saturating_sub(2);
    let title = if avail == 0 {
        String::new()
    } else {
        format!(" {}", truncate_cells(title, avail))
    };
    let title_cells = text_cells(&title);
    HeaderLayout {
        title,
        title_cells,
        buttons,
    }
}

/// Which button is under cell `col` (in pane cells); outside buttons — `Title`, outside the width — None.
pub fn button_at(layout: &HeaderLayout, width: usize, col: usize) -> Option<HeaderButton> {
    if col >= width {
        return None;
    }
    layout
        .buttons
        .iter()
        .find(|(_, _, start, end)| col >= *start && col < *end)
        .map(|(b, _, _, _)| *b)
        .or(Some(HeaderButton::Title))
}

fn button_idx(b: HeaderButton) -> usize {
    match b {
        HeaderButton::Title => 0,
        HeaderButton::Zoom => 1,
        HeaderButton::Close => 2,
    }
}

pub fn button_from_idx(idx: usize) -> Option<HeaderButton> {
    match idx {
        0 => Some(HeaderButton::Title),
        1 => Some(HeaderButton::Zoom),
        2 => Some(HeaderButton::Close),
        _ => None,
    }
}

/// Click payload: `bar = PaneHeader`, `idx` = pane_id, `side` is unused,
/// the button is in `BarKind::PaneHeader(button_idx)`.
///
/// The hit has no frame version (always 0, review H2): elements are cached by key, and a
/// version baked into a cached element would diverge from the current one after any other
/// header changes. Protection from a stale hit: `pane_id` (monotonic, never reused),
/// `pane_alive` (the pane is among the drawn ones) and `Mux::get_pane`; `ui_items` are rebuilt
/// every frame only from current keys, so the hit geometry is always current.
pub fn header_hit(pane_id: PaneId, button: HeaderButton) -> SaysdfBarHit {
    SaysdfBarHit {
        bar: BarKind::PaneHeader(button_idx(button)),
        side: BarSide::Left,
        idx: pane_id,
        version: 0,
    }
}

/// Click decision: the button, if the pane is still among the drawn ones.
pub fn header_click(hit: &SaysdfBarHit, pane_alive: bool) -> Option<HeaderButton> {
    if !pane_alive {
        return None;
    }
    match hit.bar {
        BarKind::PaneHeader(b) => button_from_idx(b),
        _ => None,
    }
}

/// Button action on a specific pane (`window:saysdf_pane_action(pane, action, confirm)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneAction {
    Zoom,
    Close { confirm: bool },
}

pub fn parse_pane_action(s: &str, confirm: bool) -> anyhow::Result<PaneAction> {
    match s {
        "zoom" => Ok(PaneAction::Zoom),
        "close" => Ok(PaneAction::Close { confirm }),
        other => anyhow::bail!(
            "saysdf_pane_action: unknown action `{}` (zoom|close)",
            sanitize_text(other)
        ),
    }
}

/// What is needed to know about a pane to draw its header (snapshot before the `paint_pane` loop).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HeaderPos {
    pub pane_id: PaneId,
    pub left: usize,
    /// Header row (in cells of the tab area).
    pub row: usize,
    pub width: usize,
    pub active: bool,
    pub zoomed: bool,
    pub title: String,
}

/// Snapshot of headers from the frame's pane list; panes without a header are skipped.
pub fn collect(panes: &[PositionedPane]) -> Vec<HeaderPos> {
    panes
        .iter()
        .filter(|p| p.saysdf_header_rows > 0 && p.top >= p.saysdf_header_rows)
        .map(|p| {
            let mut title = p.pane.get_title();
            if title.trim().is_empty() {
                if let Some(url) = p.pane.get_current_working_dir(mux::pane::CachePolicy::AllowStale) {
                    title = url
                        .path()
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string();
                }
            }
            HeaderPos {
                pane_id: p.pane.pane_id(),
                left: p.left,
                row: p.top - p.saysdf_header_rows,
                width: p.width,
                active: p.is_active,
                zoomed: p.is_zoomed,
                title: title.chars().take(crate::termwindow::saysdf_bars::MAX_TEXT_CHARS).collect(),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    pos: HeaderPos,
    /// Window geometry in pixels: x, y of the tab area origin, cell, dpi.
    origin: (isize, isize),
    cell: (isize, isize),
    dpi: usize,
}

/// Window header state (a `BarsState` field, so the cache is reset together with the bars cache).
#[derive(Default)]
pub struct PaneHeaderState {
    pub mode: HeaderMode,
    last: RefCell<Vec<HeaderPos>>,
    cache: RefCell<HashMap<CacheKey, ComputedElement>>,
}

impl PaneHeaderState {
    pub fn invalidate_cache(&self) {
        self.cache.borrow_mut().clear();
    }

    fn pane_alive(&self, pane_id: PaneId) -> bool {
        self.last.borrow().iter().any(|h| h.pane_id == pane_id)
    }
}

/// A pane whose size is fully computed by this process: `LocalPane` (pty) and
/// `TermWizTerminalPane` (built-in UI: connection, choice; its size is set by the same mux,
/// the header is safe). Client (`ClientPane`), tmux and others — not.
fn is_local_pane(p: &std::sync::Arc<dyn mux::pane::Pane>) -> bool {
    p.downcast_ref::<mux::localpane::LocalPane>().is_some()
        || p.downcast_ref::<mux::termwiztermtab::TermWizTerminalPane>().is_some()
}

impl crate::TermWindow {
    /// Lua: `window:saysdf_set_pane_header(mode)`.
    pub fn saysdf_set_pane_header(&mut self, mode: HeaderMode) {
        if self.saysdf_bars.pane_header.mode != mode {
            self.saysdf_bars.pane_header.mode = mode;
            self.saysdf_bars.invalidate_cache();
            self.saysdf_sync_pane_header_rows();
            if let Some(window) = self.window.as_ref() {
                window.invalidate();
            }
        }
    }

    /// Set `header_rows` on the active tab according to mode and pane count (called before
    /// frame layout: O(pane count), no Lua; the mux changes sizes only when the value changes).
    pub fn saysdf_sync_pane_header_rows(&mut self) {
        let mux = match Mux::try_get() {
            Some(mux) => mux,
            None => return,
        };
        let tab = match mux.get_active_tab_for_window(self.mux_window_id) {
            Some(tab) => tab,
            None => return,
        };
        // one pass: pane count + whether all are local (review M1); the tab is busy in another
        // thread — change nothing rather than disable the header for a frame (review L1)
        let (count, local) = match tab.saysdf_try_census(is_local_pane) {
            Some(c) => c,
            None => return,
        };
        let rows = should_show(self.saysdf_bars.pane_header.mode, count, local);
        tab.saysdf_set_header_rows(rows);
    }

    /// Paint headers (after panes and dividers — buttons take priority over `Split`).
    pub fn saysdf_paint_pane_headers(&mut self, headers: Vec<HeaderPos>) -> anyhow::Result<()> {
        let state = &self.saysdf_bars.pane_header;
        if *state.last.borrow() != headers {
            *state.last.borrow_mut() = headers.clone();
        }
        if headers.is_empty() {
            state.cache.borrow_mut().clear();
            return Ok(());
        }

        let (padding_left, padding_top) = self.padding_left_top();
        let tab_bar_height = if self.show_tab_bar && !self.config.tab_bar_at_bottom {
            self.tab_bar_pixel_height()?
        } else {
            0.
        };
        let border = self.get_os_border();
        let origin_x = padding_left + border.left.get() as f32;
        let origin_y = tab_bar_height + padding_top + border.top.get() as f32;
        let cell_w = self.render_metrics.cell_size.width.max(1) as f32;
        let cell_h = self.render_metrics.cell_size.height.max(1) as f32;

        let keys: Vec<CacheKey> = headers
            .into_iter()
            .map(|pos| CacheKey {
                pos,
                origin: (origin_x as isize, origin_y as isize),
                cell: (
                    self.render_metrics.cell_size.width,
                    self.render_metrics.cell_size.height,
                ),
                dpi: self.dimensions.dpi,
            })
            .collect();

        let missing: Vec<&CacheKey> = {
            let cache = state.cache.borrow();
            keys.iter().filter(|k| !cache.contains_key(*k)).collect()
        };
        if !missing.is_empty() {
            let font = self.fonts.default_font()?;
            let bar_colors = BarColors::from_config(&self.config);
            let mut fresh = vec![];
            for key in missing {
                let pos = &key.pos;
                let layout = header_layout(&pos.title, pos.width, pos.zoomed);
                let (bg, fg) = if pos.active {
                    (bar_colors.accent_bg, bar_colors.accent_fg)
                } else {
                    (bar_colors.bar_bg, bar_colors.bar_fg)
                };
                let c = colors(bg, fg);
                let mut children = vec![Element::new(&font, ElementContent::Text(layout.title.clone()))
                    .colors(c.clone())
                    .item_type(UIItemType::SaysdfBar(header_hit(pos.pane_id, HeaderButton::Title)))];
                let buttons: Vec<Element> = layout
                    .buttons
                    .iter()
                    .map(|(b, text, _, _)| {
                        Element::new(&font, ElementContent::Text(format!(" {text} ")))
                            .colors(c.clone())
                            .item_type(UIItemType::SaysdfBar(header_hit(pos.pane_id, *b)))
                    })
                    .collect();
                if !buttons.is_empty() {
                    children.push(
                        Element::new(&font, ElementContent::Children(buttons))
                            .colors(c.clone())
                            .float(Float::Right),
                    );
                }
                let width_px = pos.width as f32 * cell_w;
                let element = Element::new(&font, ElementContent::Children(children))
                    .display(DisplayType::Block)
                    .colors(c.clone())
                    .min_width(Some(Dimension::Pixels(width_px)))
                    .max_width(Some(Dimension::Pixels(width_px)))
                    .min_height(Some(Dimension::Pixels(cell_h)))
                    .item_type(UIItemType::SaysdfBar(header_hit(pos.pane_id, HeaderButton::Title)));
                let x = origin_x + pos.left as f32 * cell_w;
                let y = origin_y + pos.row as f32 * cell_h;
                let computed = self.compute_element(
                    &LayoutContext {
                        height: DimensionContext {
                            dpi: self.dimensions.dpi as f32,
                            pixel_max: self.dimensions.pixel_height as f32,
                            pixel_cell: cell_h,
                        },
                        width: DimensionContext {
                            dpi: self.dimensions.dpi as f32,
                            pixel_max: self.dimensions.pixel_width as f32,
                            pixel_cell: cell_w,
                        },
                        bounds: euclid::rect(x, y, width_px, cell_h),
                        metrics: &self.render_metrics,
                        gl_state: self.render_state.as_ref().unwrap(),
                        zindex: 5,
                    },
                    &element,
                )?;
                fresh.push((key.clone(), computed));
            }
            let mut cache = state.cache.borrow_mut();
            for (k, v) in fresh {
                cache.insert(k, v);
            }
        }

        let mut ui_items: Vec<UIItem> = vec![];
        {
            let mut cache = self.saysdf_bars.pane_header.cache.borrow_mut();
            // do not keep elements of closed/moved panes
            cache.retain(|k, _| keys.contains(k));
            let gl_state = self.render_state.as_ref().unwrap();
            for key in &keys {
                if let Some(computed) = cache.get(key) {
                    ui_items.append(&mut computed.ui_items());
                    self.render_element(computed, gl_state, None)?;
                }
            }
        }
        self.ui_items.append(&mut ui_items);
        Ok(())
    }

    /// Close/zoom pane `pane_id` (review L2). Activation and the action are performed
    /// synchronously in one GUI-thread call: no other event can intervene, and upstream's
    /// `close_current_pane`/`toggle_zoom` is guaranteed to act on this pane.
    /// Only a pane of this window's active tab; otherwise nothing.
    pub fn saysdf_pane_action(&mut self, pane_id: PaneId, action: PaneAction) {
        let mux = Mux::get();
        let tab = match mux.get_active_tab_for_window(self.mux_window_id) {
            Some(tab) => tab,
            None => return,
        };
        let pane = match tab
            .iter_panes_ignoring_zoom()
            .into_iter()
            .find(|p| p.pane.pane_id() == pane_id)
        {
            Some(p) => p.pane,
            None => return,
        };
        tab.set_active_pane(&pane);
        // set_active_pane may refuse (zoom + unzoom_on_switch_pane = false)
        if tab.get_active_pane().map(|p| p.pane_id()) != Some(pane_id) {
            return;
        }
        match action {
            PaneAction::Zoom => tab.toggle_zoom(),
            PaneAction::Close { confirm } => self.close_current_pane(confirm),
        }
    }

    /// Click on a header (from `saysdf_bar_mouse`).
    pub fn saysdf_pane_header_mouse(
        &mut self,
        hit: SaysdfBarHit,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        context.set_cursor(Some(::window::CursorIcon::Default));
        if event.kind != WMEK::Press(MousePress::Left) {
            return;
        }
        let state = &self.saysdf_bars.pane_header;
        let button = match header_click(&hit, state.pane_alive(hit.idx)) {
            Some(b) => b,
            None => return,
        };
        let pane_id = hit.idx;
        if Mux::get().get_pane(pane_id).is_none() {
            return;
        }
        let window = GuiWin::new(self);
        let pane = MuxPane(pane_id);
        let button = button.as_str().to_string();
        promise::spawn::spawn(config::with_lua_config_on_main_thread(move |lua| async move {
            if let Some(lua) = lua {
                let args = lua.pack_multi((window, pane, button))?;
                config::lua::emit_event(&lua, ("saysdf-pane-header-click".to_string(), args))
                    .await
                    .map_err(|e| {
                        log::error!("while processing saysdf-pane-header-click event: {:#}", e);
                        e
                    })?;
            }
            Ok::<(), anyhow::Error>(())
        }))
        .detach();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn saysdf_header_should_show() {
        assert_eq!(should_show(HeaderMode::Splits, 1, true), 0);
        assert_eq!(should_show(HeaderMode::Splits, 2, true), 1);
        assert_eq!(should_show(HeaderMode::Splits, 3, true), 1);
        assert_eq!(should_show(HeaderMode::Always, 1, true), 1);
        assert_eq!(should_show(HeaderMode::Never, 3, true), 0);
        // multiplexer/remote domain — no header
        assert_eq!(should_show(HeaderMode::Always, 3, false), 0);
        assert_eq!(should_show(HeaderMode::Always, 0, true), 0);
        assert_eq!(HeaderMode::default(), HeaderMode::Splits);
        assert_eq!(parse_mode("always").unwrap(), HeaderMode::Always);
        assert!(parse_mode("bogus").is_err());
        assert_eq!(parse_pane_action("zoom", true).unwrap(), PaneAction::Zoom);
        assert_eq!(
            parse_pane_action("close", false).unwrap(),
            PaneAction::Close { confirm: false }
        );
        assert!(parse_pane_action("rm -rf", true).is_err());
    }

    #[test]
    fn saysdf_header_layout_full() {
        let l = header_layout("pwsh", 40, false);
        assert_eq!(l.title, " pwsh");
        assert_eq!(
            l.buttons,
            vec![
                (HeaderButton::Zoom, ZOOM_TEXT, 34, 37),
                (HeaderButton::Close, CLOSE_TEXT, 37, 40),
            ]
        );
        let z = header_layout("pwsh", 40, true);
        assert_eq!(z.buttons[0].1, UNZOOM_TEXT);
    }

    #[test]
    fn saysdf_header_layout_narrow() {
        // only ×
        let l = header_layout("very long title", 5, false);
        assert_eq!(l.buttons, vec![(HeaderButton::Close, CLOSE_TEXT, 2, 5)]);
        assert!(l.title_cells <= 2);
        // nothing
        let l = header_layout("x", 2, false);
        assert!(l.buttons.is_empty());
        // boundary of the full layout
        assert_eq!(header_layout("t", MIN_FULL_CELLS, false).buttons.len(), 2);
        assert_eq!(header_layout("t", MIN_FULL_CELLS - 1, false).buttons.len(), 1);
    }

    #[test]
    fn saysdf_header_layout_truncates_wide_and_hostile() {
        let l = header_layout("日本語のとても長いタイトル", 14, false);
        // title does not run over the buttons
        assert!(l.title_cells <= l.buttons[0].2 - 1, "{:?}", l);
        assert!(l.title.ends_with('…'));
        let h = header_layout("evil\x1b[31m\u{202e}\r\ntitle", 60, false);
        assert!(!h.title.chars().any(|c| c.is_control()));
        assert!(!h.title.contains('\u{202e}'));
    }

    #[test]
    fn saysdf_header_hit_testing() {
        let l = header_layout("pwsh", 40, false);
        assert_eq!(button_at(&l, 40, 0), Some(HeaderButton::Title));
        assert_eq!(button_at(&l, 40, 33), Some(HeaderButton::Title));
        assert_eq!(button_at(&l, 40, 34), Some(HeaderButton::Zoom));
        assert_eq!(button_at(&l, 40, 36), Some(HeaderButton::Zoom));
        assert_eq!(button_at(&l, 40, 37), Some(HeaderButton::Close));
        assert_eq!(button_at(&l, 40, 39), Some(HeaderButton::Close));
        assert_eq!(button_at(&l, 40, 40), None);

        let hit = header_hit(7, HeaderButton::Close);
        assert_eq!(header_click(&hit, true), Some(HeaderButton::Close));
        // closed pane — ignored
        assert_eq!(header_click(&hit, false), None);
        for b in [HeaderButton::Title, HeaderButton::Zoom, HeaderButton::Close] {
            assert_eq!(header_click(&header_hit(1, b), true), Some(b));
        }
    }

    /// review H2: a hit baked into a cached element before other headers changed
    /// (focus change, title) keeps working — the hit has nothing that changes between frames.
    #[test]
    fn saysdf_header_cached_hit_survives_header_changes() {
        let state = PaneHeaderState::default();
        let pos = |id, active| HeaderPos {
            pane_id: id,
            left: 0,
            row: 0,
            width: 40,
            active,
            zoomed: false,
            title: "pwsh".into(),
        };
        *state.last.borrow_mut() = vec![pos(1, true), pos(2, false), pos(3, false)];
        let cached = header_hit(3, HeaderButton::Zoom);
        // focus moved to pane 2: headers 1 and 2 changed, the pane 3 element is from cache
        *state.last.borrow_mut() = vec![pos(1, false), pos(2, true), pos(3, false)];
        assert_eq!(header_click(&cached, state.pane_alive(3)), Some(HeaderButton::Zoom));
        // pane closed — ignored
        *state.last.borrow_mut() = vec![pos(1, false), pos(2, true)];
        assert_eq!(header_click(&cached, state.pane_alive(3)), None);
    }
}
