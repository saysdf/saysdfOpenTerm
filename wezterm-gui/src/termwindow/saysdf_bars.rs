//! saysdfTerminal: fork bars — menu on top (`top`), status bar at the bottom (`bottom`) and a temporary
//! banner bar (`banner`, above the status bar). Change menu-bar (D1, D3, D6), startup-help (D1, D4),
//! hotkey-statusbar (D1). Upstream files contain only one-line calls marked with
//! `// saysdfTerminal:`; layout, parsing of Lua data, painting and hit-testing live here.
//!
//! The bar height is added to the window border in `get_os_border()`: every upstream place that
//! computes the terminal area, the tab bar, the mouse offset and overlay positions already accounts
//! for the border, so the bars get their space without edits in each of them.
//!
//! Data format of `window:saysdf_set_bar(kind, spec)`:
//! - `kind`: `"top"` | `"bottom"` | `"banner"`;
//! - `spec`: `nil` (remove the bar) or
//!   `{ left = { item, ... }, right = { item, ... }, timeout = <sec, banner only> }`;
//! - `item`: a string (text only) or
//!   `{ text = "Git", id = "menu.git", key = "F9", children = { item, ... }, separator = true }`;
//!   `children` — drop-down menu entries (one level), `key` — hotkey label on the right,
//!   `accel` — required physical key of a menu entry (`"q"`, parsed by `saysdf_keys::parse`).
//! - for `banner` each `left` element is one line (at most 5), `right` is unused.
//!
//! Click/selection action is the event `saysdf-bar-action(window, pane, id)`; Rust holds no
//! Lua callbacks. Bar data comes only from the Lua config (terminal output cannot change it).

use crate::termwindow::box_model::*;
use crate::termwindow::saysdf_menu::SaysdfMenu;
use crate::termwindow::{GuiWin, TermWindowNotif, UIItem, UIItemType};
use ::window::{MouseEvent, MouseEventKind as WMEK, MousePress, WindowOps};
use config::{Dimension, DimensionContext, TabBarColors};
use mux_lua::MuxPane;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use termwiz::cell::unicode_column_width;
use wezterm_font::LoadedFont;
use wezterm_term::{KeyCode, KeyModifiers};
use window::color::LinearRgba;

/// Limits on data from Lua (rule 3: limits on size and count).
pub const MAX_ITEMS: usize = 64;
pub const MAX_CHILDREN: usize = 64;
pub const MAX_TEXT_CHARS: usize = 256;
pub const MAX_BANNER_LINES: usize = 5;
pub const BANNER_TIMEOUT_DEFAULT_SECS: u64 = 20;
pub const BANNER_TIMEOUT_MAX_SECS: u64 = 600;
/// The terminal always keeps at least this many rows.
pub const MIN_TERMINAL_ROWS: usize = 1;
/// The banner is not shown in a window shorter than this many rows (startup-help, edge cases).
pub const BANNER_MIN_WINDOW_ROWS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarKind {
    Top,
    Bottom,
    Banner,
    /// Drop-down menu row (`saysdf_menu.rs`).
    Menu,
    /// Pane header (`saysdf_pane_header.rs`): button index; the hit `idx` is the pane_id.
    PaneHeader(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarSide {
    Left,
    Right,
    /// Overflow item "…"; `idx` is the first hidden item of the left side.
    Overflow,
}

/// Payload of `UIItemType::SaysdfBar`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaysdfBarHit {
    pub bar: BarKind,
    pub side: BarSide,
    pub idx: usize,
    /// `BarsState.version` when the element was built; a click on a stale frame is ignored.
    pub version: u64,
}

/// What to do on a left click on a bar element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarClick {
    None,
    /// Open the menu of the top bar header with this index.
    OpenMenu(usize),
    /// Run a registry action (`saysdf-bar-action`).
    Action(String),
    /// Show the full list (command palette) — for the bottom bar's "…".
    ShowAll,
}

/// Click decision: `hit` is the element from the frame, `current_version` the current spec version,
/// `spec` the current spec of bar `hit.bar`.
pub fn bar_click(hit: &SaysdfBarHit, current_version: u64, spec: Option<&BarSpec>) -> BarClick {
    if hit.version != current_version {
        return BarClick::None;
    }
    match (hit.bar, hit.side) {
        (BarKind::Banner | BarKind::Menu | BarKind::PaneHeader(_), _) => return BarClick::None,
        // the status bar's "…" does not run the hidden hint, it opens the full list
        (BarKind::Bottom, BarSide::Overflow) => return BarClick::ShowAll,
        _ => {}
    }
    let spec = match spec {
        Some(spec) => spec,
        None => return BarClick::None,
    };
    let item = match hit.side {
        BarSide::Left | BarSide::Overflow => spec.left.get(hit.idx),
        BarSide::Right => spec.right.get(hit.idx),
    };
    let item = match item {
        Some(item) => item,
        None => return BarClick::None,
    };
    if hit.bar == BarKind::Top && hit.side == BarSide::Overflow {
        // the top bar's "…" opens the nearest hidden menu
        return BarClick::OpenMenu(hit.idx);
    }
    if hit.bar == BarKind::Top && hit.side == BarSide::Left && !item.children.is_empty() {
        return BarClick::OpenMenu(hit.idx);
    }
    match &item.id {
        Some(id) => BarClick::Action(id.clone()),
        None => BarClick::None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarItem {
    pub text: String,
    pub id: Option<String>,
    pub key: Option<String>,
    pub accel: Option<crate::termwindow::saysdf_menu::MenuAccel>,
    pub separator: bool,
    pub children: Vec<BarItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarSpec {
    pub left: Vec<BarItem>,
    pub right: Vec<BarItem>,
    pub timeout_secs: Option<u64>,
}

impl BarSpec {
    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.right.is_empty()
    }
}

/// Menu selector for `window:saysdf_open_menu(which)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuSelector {
    First,
    /// 1-based index among the headers that have entries.
    Index(usize),
    /// Header text (without `&`, case-insensitive) or the header `id`.
    Name(String),
}

// ---------------------------------------------------------------------------------------------
// Parsing data from Lua
// ---------------------------------------------------------------------------------------------

pub fn parse_kind(kind: &str) -> anyhow::Result<BarKind> {
    match kind {
        "top" => Ok(BarKind::Top),
        "bottom" => Ok(BarKind::Bottom),
        "banner" => Ok(BarKind::Banner),
        other => anyhow::bail!(
            "saysdf_set_bar: unknown bar kind {other:?} (expected \"top\", \"bottom\" or \"banner\")"
        ),
    }
}

// Invisible format and bidi characters: one set for the bars, catalog validation and the TUI
// escaping (rule 2; security review 2026-10-21-i18n, M1).
use saysdf_i18n::text::is_invisible_format;

/// Text from Lua: control, invisible and bidi characters are replaced with a space, length is limited.
pub fn sanitize_text(s: &str) -> String {
    s.chars()
        .take(MAX_TEXT_CHARS)
        .map(|c| {
            if c.is_control() || is_invisible_format(c) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn opt_string(table: &mlua::Table, name: &str) -> mlua::Result<Option<String>> {
    Ok(opt_string_raw(table, name)?.map(|s| sanitize_text(&s)))
}

fn opt_string_raw(table: &mlua::Table, name: &str) -> mlua::Result<Option<String>> {
    match table.get::<_, mlua::Value>(name)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::String(s) => Ok(Some(s.to_str()?.chars().take(MAX_TEXT_CHARS * 2).collect())),
        other => Err(mlua::Error::external(format!(
            "saysdf_set_bar: field `{name}` must be a string, got {}",
            other.type_name()
        ))),
    }
}

fn parse_item(value: mlua::Value, depth: usize) -> mlua::Result<BarItem> {
    match value {
        mlua::Value::String(s) => Ok(BarItem {
            text: sanitize_text(s.to_str()?),
            ..BarItem::default()
        }),
        mlua::Value::Table(t) => {
            let separator = matches!(
                t.get::<_, mlua::Value>("separator")?,
                mlua::Value::Boolean(true)
            );
            let mut text = opt_string_raw(&t, "text")?;
            // `"Item\tCtrl+X"` — hotkey label after the tab (format of saysdf.menu.to_bar)
            let mut tab_key = None;
            if let Some((label, key)) = text.as_deref().and_then(|t| t.split_once('\t')) {
                tab_key = Some(sanitize_text(key.trim())).filter(|k| !k.is_empty());
                text = Some(label.to_string());
            }
            let text = text.map(|t| sanitize_text(&t));
            if text.is_none() && !separator {
                return Err(mlua::Error::external(
                    "saysdf_set_bar: item needs `text` (or `separator = true`)",
                ));
            }
            let mut children = vec![];
            if depth == 0 {
                match t.get::<_, mlua::Value>("children")? {
                    mlua::Value::Nil => {}
                    mlua::Value::Table(list) => {
                        for v in list.sequence_values::<mlua::Value>().take(MAX_CHILDREN) {
                            children.push(parse_item(v?, depth + 1)?);
                        }
                    }
                    other => {
                        return Err(mlua::Error::external(format!(
                            "saysdf_set_bar: `children` must be a list, got {}",
                            other.type_name()
                        )))
                    }
                }
            }
            let id = opt_string(&t, "id")?.filter(|s| !s.is_empty());
            let accel = match opt_string_raw(&t, "accel")? {
                Some(spec) => Some(
                    crate::termwindow::saysdf_menu::parse_menu_accel(&spec, id.as_deref())
                        .map_err(mlua::Error::external)?,
                ),
                None => None,
            };
            Ok(BarItem {
                text: text.unwrap_or_default(),
                id,
                accel,
                key: opt_string(&t, "key")?.filter(|s| !s.is_empty()).or(tab_key),
                separator,
                children,
            })
        }
        other => Err(mlua::Error::external(format!(
            "saysdf_set_bar: item must be a string or a table, got {}",
            other.type_name()
        ))),
    }
}

fn parse_list(table: &mlua::Table, name: &str, limit: usize) -> mlua::Result<Vec<BarItem>> {
    match table.get::<_, mlua::Value>(name)? {
        mlua::Value::Nil => Ok(vec![]),
        mlua::Value::Table(list) => {
            let mut items = vec![];
            for v in list.sequence_values::<mlua::Value>().take(limit) {
                items.push(parse_item(v?, 0)?);
            }
            Ok(items)
        }
        other => Err(mlua::Error::external(format!(
            "saysdf_set_bar: `{name}` must be a list, got {}",
            other.type_name()
        ))),
    }
}

/// `nil` → `None` (remove the bar); an empty spec also removes the bar.
pub fn parse_spec(kind: BarKind, value: mlua::Value) -> mlua::Result<Option<BarSpec>> {
    let table = match value {
        mlua::Value::Nil => return Ok(None),
        mlua::Value::Table(t) => t,
        other => {
            return Err(mlua::Error::external(format!(
                "saysdf_set_bar: spec must be a table or nil, got {}",
                other.type_name()
            )))
        }
    };
    let banner = kind == BarKind::Banner;
    let mut spec = BarSpec {
        left: parse_list(
            &table,
            "left",
            if banner { MAX_BANNER_LINES } else { MAX_ITEMS },
        )?,
        right: if banner {
            vec![]
        } else {
            parse_list(&table, "right", MAX_ITEMS)?
        },
        timeout_secs: None,
    };
    if banner {
        spec.timeout_secs = match table.get::<_, mlua::Value>("timeout")? {
            mlua::Value::Nil => None,
            mlua::Value::Integer(i) => Some(i.max(0) as u64),
            mlua::Value::Number(n) => Some(n.max(0.) as u64),
            other => {
                return Err(mlua::Error::external(format!(
                    "saysdf_set_bar: `timeout` must be a number, got {}",
                    other.type_name()
                )))
            }
        };
    }
    if kind == BarKind::Top {
        crate::termwindow::saysdf_menu::check_accels(&spec.left).map_err(mlua::Error::external)?;
    }
    Ok(if spec.is_empty() { None } else { Some(spec) })
}

pub fn parse_menu_selector(value: Option<mlua::Value>) -> mlua::Result<MenuSelector> {
    match value {
        None | Some(mlua::Value::Nil) => Ok(MenuSelector::First),
        Some(mlua::Value::Integer(i)) if i >= 1 => Ok(MenuSelector::Index(i as usize)),
        Some(mlua::Value::Number(n)) if n >= 1. => Ok(MenuSelector::Index(n as usize)),
        Some(mlua::Value::String(s)) => Ok(MenuSelector::Name(s.to_str()?.to_string())),
        Some(other) => Err(mlua::Error::external(format!(
            "saysdf_open_menu: expected nil, a 1-based index or a menu name, got {other:?}"
        ))),
    }
}

// ---------------------------------------------------------------------------------------------
// Layout (pure functions)
// ---------------------------------------------------------------------------------------------

/// How many cell rows each bar takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BarRows {
    pub top: usize,
    pub bottom: usize,
    pub banner: usize,
}

impl BarRows {
    pub fn top_total(&self) -> usize {
        self.top
    }
    pub fn bottom_total(&self) -> usize {
        self.bottom + self.banner
    }
}

/// `avail_rows` — window rows available to the terminal and bars (after border, padding and the
/// tab bar). An empty/disabled bar takes no space. When space is short the bars are hidden
/// in the order banner → menu → status bar; the terminal keeps `MIN_TERMINAL_ROWS`.
pub fn layout_rows(avail_rows: usize, top: bool, bottom: bool, banner_lines: usize) -> BarRows {
    let mut rows = BarRows::default();
    let mut left = avail_rows;
    if bottom && left >= 1 + MIN_TERMINAL_ROWS {
        rows.bottom = 1;
        left -= 1;
    }
    if top && left >= 1 + MIN_TERMINAL_ROWS {
        rows.top = 1;
        left -= 1;
    }
    let banner_lines = banner_lines.min(MAX_BANNER_LINES);
    if banner_lines > 0
        && avail_rows >= BANNER_MIN_WINDOW_ROWS
        && left >= banner_lines + MIN_TERMINAL_ROWS
    {
        rows.banner = banner_lines;
    }
    rows
}

/// Vertical positions of the bars (pixels from the top of the window's client area).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarYs {
    /// Top of the menu bar (right under the window border).
    pub top: usize,
    /// Top of the banner (above the status bar).
    pub banner: usize,
    /// Top of the status bar (its bottom is `bottom + cell_h` = bottom of the client area minus border).
    pub bottom: usize,
}

/// Where to draw the bars: `pixel_height` is the client area height, `border_top/bottom` the
/// window border without fork bars. Bottom bars stick to the bottom border, all inside the window.
pub fn bar_ys(
    pixel_height: usize,
    border_top: usize,
    border_bottom: usize,
    cell_h: usize,
    rows: BarRows,
) -> BarYs {
    let bottom = pixel_height.saturating_sub(border_bottom + rows.bottom * cell_h);
    BarYs {
        top: border_top,
        banner: bottom.saturating_sub(rows.banner * cell_h),
        bottom,
    }
}

pub fn text_cells(s: &str) -> usize {
    unicode_column_width(s, None)
}

/// Truncate a string to `max` cells; if truncated, the last cell becomes `…`.
pub fn truncate_cells(s: &str, max: usize) -> String {
    if text_cells(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = unicode_column_width(c.encode_utf8(&mut [0u8; 4]), None);
        if used + w > max - 1 {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// Width of a bar element in cells: the text with a space on each side.
pub fn item_cells(text: &str) -> usize {
    text_cells(text) + 2
}

pub const OVERFLOW_TEXT: &str = "…";

/// Which bar elements fit in `width` cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fit {
    /// Indices of visible left elements (in order).
    pub left: Vec<usize>,
    /// If the left ones did not fit: index of the first hidden one — for the "…" element.
    pub overflow: Option<usize>,
    /// Indices of visible right elements (in order).
    pub right: Vec<usize>,
}

/// The right part sticks to the right edge and has priority (when space is short the outermost left
/// elements are dropped); the left is truncated first, those that do not fit go into "…".
pub fn fit_items(left: &[usize], right: &[usize], width: usize) -> Fit {
    let mut fit = Fit::default();
    let mut right_used = 0;
    let mut first_right = right.len();
    for (i, w) in right.iter().enumerate().rev() {
        if right_used + w > width {
            break;
        }
        right_used += w;
        first_right = i;
    }
    fit.right = (first_right..right.len()).collect();

    let avail = width - right_used;
    let mut used = 0;
    for (i, w) in left.iter().enumerate() {
        if used + w > avail {
            break;
        }
        used += w;
        fit.left.push(i);
    }
    if fit.left.len() < left.len() {
        let overflow_w = item_cells(OVERFLOW_TEXT);
        while used + overflow_w > avail {
            match fit.left.pop() {
                Some(i) => used -= left[i],
                None => break,
            }
        }
        if used + overflow_w <= avail {
            fit.overflow = Some(fit.left.len());
        }
    }
    fit
}

// ---------------------------------------------------------------------------------------------
// State in TermWindow
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct CacheKey {
    version: u64,
    pixel_width: usize,
    pixel_height: usize,
    dpi: usize,
    cell: (isize, isize),
    rows: BarRows,
    top_y: usize,
    bottom_y: usize,
}

#[derive(Default)]
pub struct BarsState {
    top: Option<BarSpec>,
    bottom: Option<BarSpec>,
    banner: Option<BarSpec>,
    banner_generation: u64,
    version: u64,
    cache: RefCell<Option<(CacheKey, Vec<ComputedElement>)>>,
    /// x (pixels) of the drawn top bar headers: (index in `top.left`, x).
    title_x: RefCell<Vec<(usize, f32)>>,
    /// Pane headers (pane-header): mode and element cache — reset together with the bars.
    pub(crate) pane_header: crate::termwindow::saysdf_pane_header::PaneHeaderState,
    swallow_release: Option<MousePress>,
    atlas_epoch: std::cell::Cell<u64>,
}

fn take_swallowed_release(slot: &mut Option<MousePress>, kind: &WMEK) -> bool {
    match kind {
        WMEK::Release(press) if *slot == Some(*press) => {
            *slot = None;
            true
        }
        WMEK::Press(_) => {
            *slot = None;
            false
        }
        _ => false,
    }
}

impl BarsState {
    /// Replace the bar spec. `false` — same data: no version bump, no cache reset, no
    /// repaint (Lua may send the same thing on every `update-status`, perf-1).
    pub fn replace(&mut self, kind: BarKind, spec: Option<BarSpec>) -> bool {
        let slot = match kind {
            BarKind::Top => &mut self.top,
            BarKind::Bottom => &mut self.bottom,
            BarKind::Banner => &mut self.banner,
            BarKind::Menu | BarKind::PaneHeader(_) => return false,
        };
        if *slot == spec {
            return false;
        }
        *slot = spec;
        self.version += 1;
        self.invalidate_cache();
        true
    }

    /// Reset the ready bar elements. They hold sprite coordinates in the glyph atlas, so they are
    /// reset wherever upstream resets the fancy tab bar cache (atlas recreation/growth,
    /// new quads, shaping cache reset, config reload, resize); otherwise after an atlas reset
    /// the bars paint foreign texture pieces — garbled glyphs (perf-1).
    pub fn invalidate_cache(&self) {
        self.cache.borrow_mut().take();
        self.pane_header.invalidate_cache();
        self.atlas_epoch.set(self.atlas_epoch.get() + 1);
    }

    pub fn atlas_epoch(&self) -> u64 {
        self.atlas_epoch.get()
    }

    pub fn title_x(&self, idx: usize) -> Option<f32> {
        let titles = self.title_x.borrow();
        titles
            .iter()
            .find(|(i, _)| *i == idx)
            .or_else(|| titles.last())
            .map(|(_, x)| *x)
    }
}

/// Bar colors: `tab_bar` from the config or scheme (`resolved_palette.tab_bar`), otherwise from the
/// scheme's background and text (change theme-switcher): bars are light on a light scheme and dark on a dark one.
pub struct BarColors {
    pub bar_bg: LinearRgba,
    pub bar_fg: LinearRgba,
    pub accent_bg: LinearRgba,
    pub accent_fg: LinearRgba,
    /// Color of the accelerator letter and banner border: the scheme palette color with the highest contrast to the bar background.
    pub hint: LinearRgba,
}

/// Blend in sRGB: `t` = 0 gives `a`, 1 gives `b`.
pub fn mix_srgb(
    a: termwiz::color::SrgbaTuple,
    b: termwiz::color::SrgbaTuple,
    t: f32,
) -> termwiz::color::SrgbaTuple {
    termwiz::color::SrgbaTuple(
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
        1.0,
    )
}

/// Share of text in the bar background and in the active item.
pub const BAR_MIX: f32 = 0.10;
pub const ACCENT_MIX: f32 = 0.25;
/// Minimum contrast of bar text to its background (WCAG AAA).
pub const MIN_CONTRAST: f32 = 7.0;

/// Text `fg`, shifted toward black/white if needed until contrast to `bg` is ≥ `min`.
pub fn ensure_contrast(
    fg: termwiz::color::SrgbaTuple,
    bg: termwiz::color::SrgbaTuple,
    min: f32,
) -> termwiz::color::SrgbaTuple {
    use termwiz::color::SrgbaTuple;
    let bgl = bg.to_linear();
    let black = SrgbaTuple(0., 0., 0., 1.);
    let white = SrgbaTuple(1., 1., 1., 1.);
    let target = if black.to_linear().contrast_ratio(&bgl) >= white.to_linear().contrast_ratio(&bgl)
    {
        black
    } else {
        white
    };
    let mut c = fg;
    for step in 1..=10 {
        if c.to_linear().contrast_ratio(&bgl) >= min {
            break;
        }
        c = mix_srgb(fg, target, step as f32 / 10.0);
    }
    c
}

/// Background and text of the bar under the tabs (the fancy tab bar takes them from `window_frame`): if
/// `window_frame.*_titlebar_bg` was not changed (upstream #333333) and `tab_bar` is unset — the same
/// scheme-derived colors as the fork bars (change theme-switcher).
pub fn titlebar_colors(config: &config::ConfigHandle, focused: bool) -> (LinearRgba, LinearRgba) {
    use termwiz::color::SrgbaTuple;
    let wf = &config.window_frame;
    let (bg, fg) = if focused {
        (wf.active_titlebar_bg, wf.active_titlebar_fg)
    } else {
        (wf.inactive_titlebar_bg, wf.inactive_titlebar_fg)
    };
    let upstream = (bg.to_linear(), fg.to_linear());
    let default_bg = SrgbaTuple(
        0x33 as f32 / 255.,
        0x33 as f32 / 255.,
        0x33 as f32 / 255.,
        1.,
    );
    let pal = &config.resolved_palette;
    let user_tab_bar = config
        .colors
        .as_ref()
        .and_then(|c| c.tab_bar.as_ref())
        .is_some();
    let b = SrgbaTuple::from(bg);
    let is_default = [
        (b.0, default_bg.0),
        (b.1, default_bg.1),
        (b.2, default_bg.2),
    ]
    .iter()
    .all(|(x, y)| (x - y).abs() < 0.003);
    if !is_default || user_tab_bar || pal.tab_bar.is_some() {
        return upstream;
    }
    match (pal.background, pal.foreground) {
        (Some(sbg), Some(sfg)) => {
            let sbg = SrgbaTuple::from(sbg);
            let sfg = SrgbaTuple::from(sfg);
            let bar = mix_srgb(sbg, sfg, BAR_MIX);
            (
                bar.to_linear(),
                ensure_contrast(sfg, bar, MIN_CONTRAST).to_linear(),
            )
        }
        _ => upstream,
    }
}

/// Tab bar colors: `resolved_palette.tab_bar` (config/scheme), otherwise derived from the background
/// and text of the scheme, so that on a light scheme the tab bar does not stay dark gray
/// (change theme-switcher). Without a scheme — `None` (upstream behavior).
pub fn effective_tab_bar(config: &config::ConfigHandle) -> Option<TabBarColors> {
    use config::{RgbaColor, TabBarColor};
    use termwiz::color::SrgbaTuple;
    let pal = &config.resolved_palette;
    if let Some(t) = &pal.tab_bar {
        return Some(t.clone());
    }
    let bg = SrgbaTuple::from(pal.background?);
    let fg = SrgbaTuple::from(pal.foreground?);
    let bar = mix_srgb(bg, fg, BAR_MIX);
    let hover = mix_srgb(bg, fg, ACCENT_MIX);
    let rgba = |c: SrgbaTuple| RgbaColor::from(c);
    let tab = |b: SrgbaTuple, f: SrgbaTuple| TabBarColor {
        bg_color: rgba(b),
        fg_color: rgba(ensure_contrast(f, b, MIN_CONTRAST)),
        ..TabBarColor::default()
    };
    let dim = mix_srgb(bg, fg, 0.8);
    Some(TabBarColors {
        background: Some(rgba(bar)),
        active_tab: Some(tab(bg, fg)),
        inactive_tab: Some(tab(bar, dim)),
        inactive_tab_hover: Some(tab(hover, fg)),
        new_tab: Some(tab(bar, dim)),
        new_tab_hover: Some(tab(hover, fg)),
        inactive_tab_edge: Some(rgba(hover)),
        inactive_tab_edge_hover: Some(rgba(hover)),
    })
}

impl BarColors {
    pub fn from_config(config: &config::ConfigHandle) -> Self {
        let pal = &config.resolved_palette;
        let explicit = config
            .colors
            .as_ref()
            .and_then(|c| c.tab_bar.as_ref())
            .or(pal.tab_bar.as_ref())
            .cloned();
        let (bar_bg, bar_fg, accent_bg, accent_fg, fallback_hint) =
            match (explicit, pal.background, pal.foreground) {
                (None, Some(bg), Some(fg)) => {
                    let bg = termwiz::color::SrgbaTuple::from(bg);
                    let fg = termwiz::color::SrgbaTuple::from(fg);
                    let bar = mix_srgb(bg, fg, BAR_MIX);
                    let accent = mix_srgb(bg, fg, ACCENT_MIX);
                    let bar_fg = ensure_contrast(fg, bar, MIN_CONTRAST).to_linear();
                    let accent_fg = ensure_contrast(fg, accent, MIN_CONTRAST).to_linear();
                    (
                        bar.to_linear(),
                        bar_fg,
                        accent.to_linear(),
                        accent_fg,
                        bar_fg,
                    )
                }
                (colors, _, _) => {
                    let colors = colors.unwrap_or_else(TabBarColors::default);
                    let inactive = colors.inactive_tab();
                    let active = colors.active_tab();
                    (
                        colors.background().to_linear(),
                        inactive.fg_color.to_linear(),
                        active.bg_color.to_linear(),
                        active.fg_color.to_linear(),
                        active.fg_color.to_linear(),
                    )
                }
            };
        // saysdfTerminal: take the accent from the scheme palette (yellow/blue/cyan/green and bright ones) so it
        // reads on both dark and light schemes.
        let default_pal = wezterm_term::color::ColorPalette::default();
        use termwiz::color::SrgbaTuple;
        let hint = [3usize, 4, 6, 2]
            .iter()
            .map(|&i| {
                pal.ansi
                    .as_ref()
                    .map(|a| SrgbaTuple::from(a[i]))
                    .unwrap_or_else(|| default_pal.colors.0[i])
            })
            .chain([3usize, 4, 6, 2].iter().map(|&i| {
                pal.brights
                    .as_ref()
                    .map(|a| SrgbaTuple::from(a[i]))
                    .unwrap_or_else(|| default_pal.colors.0[i + 8])
            }))
            .map(|c| c.to_linear())
            .max_by(|a, b| {
                a.contrast_ratio(&bar_bg)
                    .partial_cmp(&b.contrast_ratio(&bar_bg))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(fallback_hint);
        // accelerator letter/banner border — at least 4.5:1 against the bar background (ANSI colors are pale on light schemes)
        let hint = ensure_contrast(hint.to_srgb(), bar_bg.to_srgb(), 4.5).to_linear();
        Self {
            hint,
            bar_bg,
            bar_fg,
            accent_bg,
            accent_fg,
        }
    }
}

pub fn colors(bg: LinearRgba, fg: LinearRgba) -> ElementColors {
    ElementColors {
        border: BorderColor::default(),
        bg: bg.into(),
        text: fg.into(),
    }
}

pub fn box_row_colors(c: &BarColors, selected: bool) -> (LinearRgba, LinearRgba) {
    if selected {
        (c.accent_bg, c.accent_fg)
    } else {
        (c.bar_bg, c.bar_fg)
    }
}

pub fn box_row(
    font: &Rc<LoadedFont>,
    parts: Vec<Element>,
    bg: LinearRgba,
    fg: LinearRgba,
    inner_px: f32,
) -> Element {
    Element::new(font, ElementContent::Children(parts))
        .colors(colors(bg, fg))
        .display(DisplayType::Block)
        .min_width(Some(Dimension::Pixels(inner_px)))
}

pub fn box_element(
    font: &Rc<LoadedFont>,
    rows: Vec<Element>,
    c: &BarColors,
    inner_px: f32,
) -> Element {
    Element::new(font, ElementContent::Children(rows))
        .colors(ElementColors {
            border: BorderColor::new(c.bar_fg),
            bg: c.bar_bg.into(),
            text: c.bar_fg.into(),
        })
        .border(BoxDimension::new(Dimension::Pixels(1.)))
        .min_width(Some(Dimension::Pixels(inner_px)))
}

fn cells_padding(left: f32, right: f32) -> BoxDimension {
    BoxDimension {
        left: Dimension::Cells(left),
        right: Dimension::Cells(right),
        top: Dimension::Cells(0.),
        bottom: Dimension::Cells(0.),
    }
}

impl crate::TermWindow {
    fn saysdf_base_border(&self) -> window::parameters::Border {
        Self::get_os_border_impl(
            &self.os_parameters,
            &self.config,
            &self.dimensions,
            &self.render_metrics,
        )
    }

    /// Rows currently taken by the fork bars (for the current window size).
    pub fn saysdf_bar_rows(&self) -> BarRows {
        let state = &self.saysdf_bars;
        if state.top.is_none() && state.bottom.is_none() && state.banner.is_none() {
            return BarRows::default();
        }
        let base = self.saysdf_base_border();
        let cell_h = self.render_metrics.cell_size.height.max(1) as usize;
        let v_context = DimensionContext {
            dpi: self.dimensions.dpi as f32,
            pixel_max: self.terminal_size.pixel_height as f32,
            pixel_cell: cell_h as f32,
        };
        let padding = self.config.window_padding.top.evaluate_as_pixels(v_context)
            + self
                .config
                .window_padding
                .bottom
                .evaluate_as_pixels(v_context);
        let tab_bar = if self.show_tab_bar {
            self.tab_bar_pixel_height().unwrap_or(0.)
        } else {
            0.
        };
        let avail_px = self
            .dimensions
            .pixel_height
            .saturating_sub((base.top + base.bottom).get() as usize)
            .saturating_sub((padding + tab_bar).max(0.) as usize);
        layout_rows(
            avail_px / cell_h,
            state.top.is_some(),
            state.bottom.is_some(),
            state.banner.as_ref().map(|b| b.left.len()).unwrap_or(0),
        )
    }

    /// Called from `get_os_border()`: the bar height is added to the window border at top and bottom.
    pub fn saysdf_bars_border(
        &self,
        mut border: window::parameters::Border,
    ) -> window::parameters::Border {
        let rows = self.saysdf_bar_rows();
        let cell_h = self.render_metrics.cell_size.height.max(0) as usize;
        border.top += window::ULength::new(rows.top_total() * cell_h);
        border.bottom += window::ULength::new(rows.bottom_total() * cell_h);
        border
    }

    /// Apply a new bar spec (from `window:saysdf_set_bar`).
    pub fn saysdf_set_bar(&mut self, kind: BarKind, spec: Option<BarSpec>) {
        let before = self.saysdf_bar_rows();
        if !self.saysdf_bars.replace(kind, spec) {
            return;
        }
        let state = &mut self.saysdf_bars;

        if kind == BarKind::Banner {
            state.banner_generation += 1;
            let generation = state.banner_generation;
            let secs = state.banner.as_ref().map(|banner| {
                banner
                    .timeout_secs
                    .unwrap_or(BANNER_TIMEOUT_DEFAULT_SECS)
                    .min(BANNER_TIMEOUT_MAX_SECS)
            });
            if let Some(secs) = secs.filter(|s| *s > 0) {
                self.saysdf_schedule_banner_timeout(generation, secs);
            }
        }
        if kind == BarKind::Top && self.saysdf_bars.top.is_none() {
            self.saysdf_close_menu();
        }
        self.saysdf_bars_changed(before);
    }

    fn saysdf_schedule_banner_timeout(&self, generation: u64, secs: u64) {
        if let Some(window) = self.window.clone() {
            promise::spawn::spawn(async move {
                smol::Timer::after(Duration::from_secs(secs)).await;
                window.notify(TermWindowNotif::Apply(Box::new(move |tw| {
                    if tw.saysdf_bars.banner_generation == generation {
                        tw.saysdf_hide_banner();
                    }
                })));
            })
            .detach();
        }
    }

    pub fn saysdf_hide_banner(&mut self) {
        if self.saysdf_bars.banner.is_some() {
            self.saysdf_set_bar(BarKind::Banner, None);
        }
    }

    /// Recompute sizes if the bars changed the height they take (the usual resize path —
    /// the pty gets the new size the same way as when the tab bar is shown/hidden).
    fn saysdf_bars_changed(&mut self, before: BarRows) {
        let after = self.saysdf_bar_rows();
        if let Some(window) = self.window.clone() {
            if before != after {
                let dimensions = self.dimensions;
                self.apply_dimensions(&dimensions, None, &window);
            }
            window.invalidate();
        }
    }

    pub fn saysdf_compute_box(
        &self,
        element: &Element,
        bounds: ::window::RectF,
        zindex: i8,
    ) -> anyhow::Result<ComputedElement> {
        let dims = self.dimensions;
        self.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: dims.dpi as f32,
                    pixel_max: dims.pixel_height as f32,
                    pixel_cell: self.render_metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: dims.dpi as f32,
                    pixel_max: dims.pixel_width as f32,
                    pixel_cell: self.render_metrics.cell_size.width.max(1) as f32,
                },
                bounds,
                metrics: &self.render_metrics,
                gl_state: self.render_state.as_ref().unwrap(),
                zindex,
            },
            element,
        )
    }

    /// Paint the bars (called from `paint_pass` after the main pass, before the modal).
    pub fn saysdf_paint_bars(&mut self) -> anyhow::Result<()> {
        let rows = self.saysdf_bar_rows();
        if rows == BarRows::default() {
            return Ok(());
        }
        let base = self.saysdf_base_border();
        let cell_h = self.render_metrics.cell_size.height.max(0) as usize;
        let ys = bar_ys(
            self.dimensions.pixel_height,
            base.top.get() as usize,
            base.bottom.get() as usize,
            cell_h,
            rows,
        );
        let (top_y, bottom_y) = (ys.top, ys.bottom);
        let key = CacheKey {
            version: self.saysdf_bars.version,
            pixel_width: self.dimensions.pixel_width,
            pixel_height: self.dimensions.pixel_height,
            dpi: self.dimensions.dpi,
            cell: (
                self.render_metrics.cell_size.width,
                self.render_metrics.cell_size.height,
            ),
            rows,
            top_y,
            bottom_y,
        };
        let cached = matches!(&*self.saysdf_bars.cache.borrow(), Some((k, _)) if *k == key);
        if !cached {
            let elements = self.saysdf_compute_bars(rows, &base, ys)?;
            let mut titles = vec![];
            for computed in &elements {
                for item in computed.ui_items() {
                    if let UIItemType::SaysdfBar(SaysdfBarHit {
                        bar: BarKind::Top,
                        side: BarSide::Left | BarSide::Overflow,
                        idx,
                        ..
                    }) = item.item_type
                    {
                        titles.push((idx, item.x as f32));
                    }
                }
            }
            *self.saysdf_bars.title_x.borrow_mut() = titles;
            self.saysdf_bars.cache.borrow_mut().replace((key, elements));
        }

        let mut ui_items: Vec<UIItem> = vec![];
        {
            let cache = self.saysdf_bars.cache.borrow();
            if let Some((_, elements)) = cache.as_ref() {
                let gl_state = self.render_state.as_ref().unwrap();
                for computed in elements {
                    ui_items.append(&mut computed.ui_items());
                    self.render_element(computed, gl_state, None)?;
                }
            }
        }
        self.ui_items.append(&mut ui_items);
        Ok(())
    }

    fn saysdf_compute_bars(
        &self,
        rows: BarRows,
        base: &window::parameters::Border,
        ys: BarYs,
    ) -> anyhow::Result<Vec<ComputedElement>> {
        let font = self.fonts.default_font()?;
        let colors = BarColors::from_config(&self.config);
        let cell_w = self.render_metrics.cell_size.width.max(1) as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let x = base.left.get() as f32;
        let width =
            (self.dimensions.pixel_width as f32 - (base.left + base.right).get() as f32).max(0.);
        let width_cells = (width / cell_w) as usize;

        let mut out = vec![];
        let mut place = |element: Element, y: f32, height: f32| -> anyhow::Result<()> {
            out.push(self.compute_element(
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
                    bounds: euclid::rect(x, y, width, height),
                    metrics: &self.render_metrics,
                    gl_state: self.render_state.as_ref().unwrap(),
                    zindex: 10,
                },
                &element,
            )?);
            Ok(())
        };

        if rows.top > 0 {
            if let Some(spec) = &self.saysdf_bars.top {
                let version = self.saysdf_bars.version;
                let e = bar_element(
                    &font,
                    BarKind::Top,
                    spec,
                    version,
                    width_cells,
                    width,
                    cell_h,
                    &colors,
                );
                place(e, ys.top as f32, cell_h)?;
            }
        }
        if rows.banner > 0 {
            if let Some(spec) = &self.saysdf_bars.banner {
                let e = banner_element(
                    &font,
                    spec,
                    rows.banner,
                    width_cells,
                    width,
                    cell_h,
                    &colors,
                );
                place(e, ys.banner as f32, rows.banner as f32 * cell_h)?;
            }
        }
        if rows.bottom > 0 {
            if let Some(spec) = &self.saysdf_bars.bottom {
                let version = self.saysdf_bars.version;
                let e = bar_element(
                    &font,
                    BarKind::Bottom,
                    spec,
                    version,
                    width_cells,
                    width,
                    cell_h,
                    &colors,
                );
                place(e, ys.bottom as f32, cell_h)?;
            }
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------------------------
    // Mouse and keyboard
    // -----------------------------------------------------------------------------------------

    fn saysdf_hit_at(&self, x: isize, y: isize) -> Option<SaysdfBarHit> {
        self.ui_items
            .iter()
            .rev()
            .find(|item| item.hit_test(x, y))
            .and_then(|item| match item.item_type {
                UIItemType::SaysdfBar(hit) => Some(hit),
                _ => None,
            })
    }

    /// Called at the start of `mouse_event_impl`. While a fork menu is open, all mouse events go to it
    /// (a click outside the menu closes it and does not reach the program). A button press hides the banner.
    /// Returns `true` if the event was consumed.
    pub fn saysdf_mouse_event(&mut self, event: &MouseEvent) -> bool {
        if take_swallowed_release(&mut self.saysdf_bars.swallow_release, &event.kind) {
            self.saysdf_release_mouse_state(&event.kind);
            return true;
        }
        if let WMEK::Press(press) = event.kind {
            if self.saysdf_bars.banner.is_some() {
                let on_banner = matches!(
                    self.saysdf_hit_at(event.coords.x, event.coords.y),
                    Some(SaysdfBarHit {
                        bar: BarKind::Banner,
                        ..
                    })
                );
                self.saysdf_hide_banner();
                if on_banner {
                    self.saysdf_bars.swallow_release = Some(press);
                    return true;
                }
            }
        }

        let menu = match self.get_modal() {
            Some(modal) => match modal.downcast_rc::<SaysdfMenu>() {
                Ok(menu) => menu,
                Err(_) => return false,
            },
            None => return false,
        };
        let hit = self.saysdf_hit_at(event.coords.x, event.coords.y);
        if let WMEK::Press(press) = event.kind {
            self.saysdf_bars.swallow_release = Some(press);
        }
        match event.kind {
            WMEK::Press(MousePress::Left) => match hit {
                Some(SaysdfBarHit {
                    bar: BarKind::Menu,
                    idx,
                    ..
                }) => menu.click_row(idx, self),
                Some(SaysdfBarHit {
                    bar: BarKind::Top,
                    side: BarSide::Left | BarSide::Overflow,
                    idx,
                    ..
                }) => {
                    if !menu.switch_to_title(idx, self) {
                        self.saysdf_close_menu();
                    }
                }
                _ => self.saysdf_close_menu(),
            },
            WMEK::Press(_) => self.saysdf_close_menu(),
            WMEK::VertWheel(delta) => menu.wheel(delta, self),
            WMEK::Move => {
                if let Some(SaysdfBarHit {
                    bar: BarKind::Menu,
                    idx,
                    ..
                }) = hit
                {
                    menu.hover_row(idx, self);
                }
            }
            WMEK::Release(_) => self.saysdf_release_mouse_state(&event.kind),
            _ => {}
        }
        true
    }

    fn saysdf_release_mouse_state(&mut self, kind: &WMEK) {
        if let WMEK::Release(press) = kind {
            self.current_mouse_capture = None;
            self.current_mouse_buttons.retain(|p| p != press);
        }
    }

    /// Click on a bar element (the `UIItemType::SaysdfBar` branch in `mouse_event_ui_item`).
    pub fn saysdf_bar_mouse(
        &mut self,
        hit: SaysdfBarHit,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        if event.kind == WMEK::Press(MousePress::Left) {
            self.saysdf_bars.swallow_release = Some(MousePress::Left);
        }
        if let BarKind::PaneHeader(_) = hit.bar {
            return self.saysdf_pane_header_mouse(hit, event, context);
        }
        context.set_cursor(Some(::window::CursorIcon::Default));
        if event.kind != WMEK::Press(MousePress::Left) {
            return;
        }
        let spec = match hit.bar {
            BarKind::Top => self.saysdf_bars.top.as_ref(),
            BarKind::Bottom => self.saysdf_bars.bottom.as_ref(),
            BarKind::Banner | BarKind::Menu | BarKind::PaneHeader(_) => None,
        };
        match bar_click(&hit, self.saysdf_bars.version, spec) {
            BarClick::None => {}
            BarClick::OpenMenu(idx) => self.saysdf_open_menu_at_title(idx),
            BarClick::Action(id) => self.saysdf_emit_bar_action(id),
            BarClick::ShowAll => {
                if let Some(pane) = self.get_active_pane_or_overlay() {
                    if let Err(e) = self.perform_key_assignment(
                        &pane,
                        &config::keyassignment::KeyAssignment::ActivateCommandPalette,
                    ) {
                        log::error!("saysdf bar overflow: {:#}", e);
                    }
                }
            }
        }
    }

    /// Called in `process_key` on key press: any input hides the banner and passes on
    /// (the character reaches the shell), Esc hides the banner and is consumed.
    pub fn saysdf_key_down(&mut self, keycode: &::window::KeyCode) -> bool {
        if self.saysdf_bars.banner.is_none() {
            return false;
        }
        let key = match self.win_key_code_to_termwiz_key_code(keycode) {
            crate::termwindow::keyevent::Key::Code(key) => key,
            _ => return false,
        };
        if key.is_modifier() {
            return false;
        }
        self.saysdf_hide_banner();
        key == KeyCode::Escape
    }

    /// Event `saysdf-bar-action(window, pane, id)` (modeled on `new-tab-button-click`).
    pub fn saysdf_emit_bar_action(&mut self, id: String) {
        let pane = match self.get_active_pane_or_overlay() {
            Some(pane) => MuxPane(pane.pane_id()),
            None => return,
        };
        let window = GuiWin::new(self);
        promise::spawn::spawn(config::with_lua_config_on_main_thread(
            move |lua| async move {
                if let Some(lua) = lua {
                    let args = lua.pack_multi((window, pane, id))?;
                    config::lua::emit_event(&lua, ("saysdf-bar-action".to_string(), args))
                        .await
                        .map_err(|e| {
                            log::error!("while processing saysdf-bar-action event: {:#}", e);
                            e
                        })?;
                }
                Ok::<(), anyhow::Error>(())
            },
        ))
        .detach();
    }

    // -----------------------------------------------------------------------------------------
    // Menu
    // -----------------------------------------------------------------------------------------

    pub fn saysdf_close_menu(&mut self) {
        let is_menu = self
            .get_modal()
            .map(|m| m.downcast_rc::<SaysdfMenu>().is_ok())
            .unwrap_or(false);
        if is_menu {
            self.cancel_modal();
        }
    }

    fn saysdf_open_menu_at_title(&mut self, title_idx: usize) {
        let top = match &self.saysdf_bars.top {
            Some(top) => top.clone(),
            None => return,
        };
        match SaysdfMenu::new(&top, title_idx) {
            Some(menu) => self.set_modal(Rc::new(menu)),
            None => log::debug!("saysdf menu: no menus in the top bar"),
        }
    }

    /// `window:saysdf_open_menu(which)`.
    pub fn saysdf_open_menu(&mut self, which: MenuSelector) {
        let top = match &self.saysdf_bars.top {
            Some(top) => top,
            None => {
                log::debug!("saysdf_open_menu: top bar is not set");
                return;
            }
        };
        match crate::termwindow::saysdf_menu::select_title(top, &which) {
            Some(idx) => self.saysdf_open_menu_at_title(idx),
            None => log::warn!("saysdf_open_menu: menu {which:?} not found"),
        }
    }

    /// Drop-down menu geometry: header x and y under the top bar.
    pub fn saysdf_menu_anchor(&self, title_idx: usize) -> (f32, f32) {
        let base = self.saysdf_base_border();
        let rows = self.saysdf_bar_rows();
        let cell_h = self.render_metrics.cell_size.height as f32;
        let x = self
            .saysdf_bars
            .title_x(title_idx)
            .unwrap_or(base.left.get() as f32);
        (x, base.top.get() as f32 + rows.top as f32 * cell_h)
    }
}

/// Element of the `top`/`bottom` bar (one row of cells).
fn bar_element(
    font: &Rc<LoadedFont>,
    kind: BarKind,
    spec: &BarSpec,
    version: u64,
    width_cells: usize,
    width_px: f32,
    cell_h: f32,
    colors: &BarColors,
) -> Element {
    let display = |item: &BarItem| -> String {
        if kind == BarKind::Top {
            crate::termwindow::saysdf_menu::strip_accel(&item.text)
        } else {
            item.text.clone()
        }
    };
    let left_w: Vec<usize> = spec.left.iter().map(|i| item_cells(&display(i))).collect();
    let right_w: Vec<usize> = spec.right.iter().map(|i| item_cells(&display(i))).collect();
    let fit = fit_items(&left_w, &right_w, width_cells);

    let make = |text: String, side: BarSide, idx: usize| {
        Element::new(font, ElementContent::Text(format!(" {text} ")))
            .colors(colors_for(colors))
            .item_type(UIItemType::SaysdfBar(SaysdfBarHit {
                bar: kind,
                side,
                idx,
                version,
            }))
    };

    let mut children = vec![];
    for &i in &fit.left {
        children.push(make(display(&spec.left[i]), BarSide::Left, i));
    }
    if let Some(i) = fit.overflow {
        children.push(make(OVERFLOW_TEXT.to_string(), BarSide::Overflow, i));
    }
    let right: Vec<Element> = fit
        .right
        .iter()
        .map(|&i| make(display(&spec.right[i]), BarSide::Right, i))
        .collect();
    if !right.is_empty() {
        children.push(
            Element::new(font, ElementContent::Children(right))
                .colors(colors_for(colors))
                .float(Float::Right),
        );
    }

    let bar = Element::new(font, ElementContent::Children(children))
        .display(DisplayType::Block)
        .colors(colors_for(colors))
        .min_width(Some(Dimension::Pixels(width_px)))
        .min_height(Some(Dimension::Pixels(cell_h)));
    if kind == BarKind::Top {
        // empty space of the menu bar behaves like empty space of the tab bar: window dragging
        // (and double click for INTEGRATED_BUTTONS) — the upstream handler
        bar.item_type(UIItemType::TabBar(crate::tabbar::TabBarItem::None))
    } else {
        bar
    }
}

fn colors_for(c: &BarColors) -> ElementColors {
    colors(c.bar_bg, c.bar_fg)
}

/// Banner element: text lines truncated to the width; a click on it hides the banner.
fn banner_element(
    font: &Rc<LoadedFont>,
    spec: &BarSpec,
    lines: usize,
    width_cells: usize,
    width_px: f32,
    cell_h: f32,
    colors: &BarColors,
) -> Element {
    let children = spec
        .left
        .iter()
        .take(lines)
        .map(|item| {
            Element::new(
                font,
                ElementContent::Text(truncate_cells(&item.text, width_cells.saturating_sub(2))),
            )
            .colors(colors_for_banner(colors))
            .padding(cells_padding(1., 1.))
            .display(DisplayType::Block)
        })
        .collect();
    Element::new(font, ElementContent::Children(children))
        .display(DisplayType::Block)
        .colors(ElementColors {
            border: BorderColor {
                top: colors.hint.into(),
                ..BorderColor::default()
            },
            ..colors_for_banner(colors)
        })
        .border(BoxDimension {
            top: Dimension::Pixels(1.),
            ..BoxDimension::default()
        })
        .min_width(Some(Dimension::Pixels(width_px)))
        .min_height(Some(Dimension::Pixels(lines as f32 * cell_h)))
        .item_type(UIItemType::SaysdfBar(SaysdfBarHit {
            bar: BarKind::Banner,
            side: BarSide::Left,
            idx: 0,
            version: 0,
        }))
}

fn colors_for_banner(c: &BarColors) -> ElementColors {
    colors(c.bar_bg, c.bar_fg)
}

/// Modifiers under which a letter in the menu counts as an accelerator.
pub fn plain_mods(mods: KeyModifiers) -> bool {
    mods == KeyModifiers::NONE || mods == KeyModifiers::SHIFT
}

#[cfg(test)]
mod tests {
    #[test]
    fn saysdf_swallowed_release_matches_consumed_press() {
        let mut slot = Some(MousePress::Left);
        assert!(!take_swallowed_release(&mut slot, &WMEK::Move));
        assert!(!take_swallowed_release(
            &mut slot,
            &WMEK::Release(MousePress::Right)
        ));
        assert!(take_swallowed_release(
            &mut slot,
            &WMEK::Release(MousePress::Left)
        ));
        assert_eq!(slot, None);
        assert!(!take_swallowed_release(
            &mut slot,
            &WMEK::Release(MousePress::Left)
        ));

        let mut slot = Some(MousePress::Left);
        assert!(!take_swallowed_release(
            &mut slot,
            &WMEK::Press(MousePress::Left)
        ));
        assert_eq!(slot, None);
    }

    use super::*;

    #[test]
    fn saysdf_ensure_contrast_reaches_min() {
        use termwiz::color::SrgbaTuple;
        // Solarized Light: text #657b83 on background #fdf6e3 with 10% blend — < 7:1 without correction
        let bg = SrgbaTuple(
            0xfd as f32 / 255.,
            0xf6 as f32 / 255.,
            0xe3 as f32 / 255.,
            1.,
        );
        let fg = SrgbaTuple(
            0x65 as f32 / 255.,
            0x7b as f32 / 255.,
            0x83 as f32 / 255.,
            1.,
        );
        let bar = mix_srgb(bg, fg, BAR_MIX);
        let c = ensure_contrast(fg, bar, MIN_CONTRAST);
        assert!(c.to_linear().contrast_ratio(&bar.to_linear()) >= MIN_CONTRAST);
        // already contrasting text is unchanged
        let black = SrgbaTuple(0., 0., 0., 1.);
        assert_eq!(ensure_contrast(black, bg, MIN_CONTRAST), black);
    }

    #[test]
    fn saysdf_layout_empty_bars_take_no_space() {
        assert_eq!(layout_rows(30, false, false, 0), BarRows::default());
    }

    #[test]
    fn saysdf_layout_all_bars() {
        let r = layout_rows(30, true, true, 4);
        assert_eq!(
            r,
            BarRows {
                top: 1,
                bottom: 1,
                banner: 4
            }
        );
        assert_eq!(r.top_total(), 1);
        assert_eq!(r.bottom_total(), 5);
    }

    #[test]
    fn saysdf_layout_banner_capped_and_hidden_in_small_window() {
        assert_eq!(layout_rows(30, false, false, 9).banner, MAX_BANNER_LINES);
        // window shorter than 8 rows — no banner
        assert_eq!(
            layout_rows(7, true, true, 2),
            BarRows {
                top: 1,
                bottom: 1,
                banner: 0
            }
        );
        // exactly 8 rows: menu, status bar, 5 banner rows and 1 terminal row
        assert_eq!(
            layout_rows(8, true, true, 5),
            BarRows {
                top: 1,
                bottom: 1,
                banner: 5
            }
        );
    }

    #[test]
    fn saysdf_layout_priority_in_tiny_window() {
        // 3 rows: menu and status bar, 1 for the terminal
        assert_eq!(
            layout_rows(3, true, true, 0),
            BarRows {
                top: 1,
                bottom: 1,
                banner: 0
            }
        );
        // 2 rows: only the status bar remains
        assert_eq!(
            layout_rows(2, true, true, 0),
            BarRows {
                top: 0,
                bottom: 1,
                banner: 0
            }
        );
        // 2 rows, menu only
        assert_eq!(
            layout_rows(2, true, false, 0),
            BarRows {
                top: 1,
                bottom: 0,
                banner: 0
            }
        );
        // 1 row — all to the terminal
        assert_eq!(layout_rows(1, true, true, 3), BarRows::default());
        assert_eq!(layout_rows(0, true, true, 3), BarRows::default());
    }

    #[test]
    fn saysdf_bar_ys_inside_client_area() {
        // different window sizes, cell heights and borders: all bars fully inside the window,
        // status bar sticks to the bottom border, banner right above it, menu under the top border
        for &(h, cell, bt, bb) in &[
            (650usize, 24usize, 0usize, 0usize),
            (500, 24, 0, 0),
            (642, 27, 1, 1),
            (1600, 40, 30, 8),
            (200, 30, 0, 0),
        ] {
            let avail = (h - bt - bb) / cell;
            for banner in 0..=5 {
                let rows = layout_rows(avail, true, true, banner);
                let ys = bar_ys(h, bt, bb, cell, rows);
                assert_eq!(ys.top, bt);
                if rows.bottom > 0 {
                    assert_eq!(ys.bottom + cell, h - bb, "h={h} cell={cell}");
                }
                assert_eq!(ys.banner + rows.banner * cell, ys.bottom);
                assert!(ys.top + rows.top * cell <= ys.banner, "bars overlap: h={h}");
                assert!(ys.bottom + rows.bottom * cell <= h - bb);
            }
        }
        // tiny window: nothing goes negative
        let ys = bar_ys(
            10,
            0,
            0,
            24,
            BarRows {
                top: 0,
                bottom: 1,
                banner: 0,
            },
        );
        assert_eq!(ys.bottom, 0);
    }

    #[test]
    fn saysdf_fit_all_items() {
        let f = fit_items(&[4, 5], &[3], 20);
        assert_eq!(
            f,
            Fit {
                left: vec![0, 1],
                overflow: None,
                right: vec![0]
            }
        );
    }

    #[test]
    fn saysdf_fit_overflow_left_first() {
        // width 12: right 3, left 9: 4 + "…"(3) = 7, the next 4 does not fit
        let f = fit_items(&[4, 4, 4], &[3], 12);
        assert_eq!(
            f,
            Fit {
                left: vec![0],
                overflow: Some(1),
                right: vec![0]
            }
        );
    }

    fn hint(text: &str, id: &str) -> BarItem {
        BarItem {
            text: text.into(),
            id: Some(id.into()),
            ..BarItem::default()
        }
    }

    fn hit(bar: BarKind, side: BarSide, idx: usize, version: u64) -> SaysdfBarHit {
        SaysdfBarHit {
            bar,
            side,
            idx,
            version,
        }
    }

    #[test]
    fn saysdf_click_bottom_overflow_runs_nothing_hidden() {
        let spec = BarSpec {
            left: vec![hint("a", "core.a"), hint("b", "git.commit")],
            ..BarSpec::default()
        };
        // "…" in place of hidden hint 1 — does not run git.commit, it opens the full list
        let f = fit_items(&[4, 4, 4], &[3], 12);
        let ov = f.overflow.unwrap();
        assert_eq!(
            bar_click(
                &hit(BarKind::Bottom, BarSide::Overflow, ov, 3),
                3,
                Some(&spec)
            ),
            BarClick::ShowAll
        );
        assert_eq!(
            bar_click(&hit(BarKind::Bottom, BarSide::Left, 0, 3), 3, Some(&spec)),
            BarClick::Action("core.a".into())
        );
        // the top bar's "…" opens the nearest hidden menu
        assert_eq!(
            bar_click(&hit(BarKind::Top, BarSide::Overflow, 1, 3), 3, Some(&spec)),
            BarClick::OpenMenu(1)
        );
        assert_eq!(
            bar_click(&hit(BarKind::Bottom, BarSide::Right, 0, 3), 3, Some(&spec)),
            BarClick::None
        );
    }

    #[test]
    fn saysdf_same_bar_data_does_not_invalidate() {
        let spec = BarSpec {
            left: vec![hint("a", "core.a")],
            ..BarSpec::default()
        };
        let mut st = BarsState::default();
        assert!(st.replace(BarKind::Bottom, Some(spec.clone())));
        let v = st.version;
        // identical data (every update-status) — no new version, no repaint
        assert!(!st.replace(BarKind::Bottom, Some(spec.clone())));
        assert_eq!(st.version, v);
        assert!(!st.replace(BarKind::Menu, None));
        let other = BarSpec {
            left: vec![hint("b", "core.b")],
            ..BarSpec::default()
        };
        assert!(st.replace(BarKind::Bottom, Some(other)));
        assert_eq!(st.version, v + 1);
        assert!(st.replace(BarKind::Bottom, None));
        assert!(!st.replace(BarKind::Bottom, None));
    }

    #[test]
    fn saysdf_click_stale_version_ignored() {
        let spec = BarSpec {
            left: vec![hint("a", "core.a")],
            ..BarSpec::default()
        };
        assert_eq!(
            bar_click(&hit(BarKind::Bottom, BarSide::Left, 0, 4), 5, Some(&spec)),
            BarClick::None
        );
        assert_eq!(
            bar_click(
                &hit(BarKind::Bottom, BarSide::Overflow, 0, 4),
                5,
                Some(&spec)
            ),
            BarClick::None
        );
        assert_eq!(
            bar_click(&hit(BarKind::Bottom, BarSide::Left, 0, 5), 5, Some(&spec)),
            BarClick::Action("core.a".into())
        );
    }

    #[test]
    fn saysdf_sanitize_invisible_and_bidi() {
        for c in [
            '\u{00AD}', '\u{061C}', '\u{180E}', '\u{200B}', '\u{200F}', '\u{2028}', '\u{202A}',
            '\u{202E}', '\u{2060}', '\u{2064}', '\u{2066}', '\u{2069}', '\u{FEFF}',
        ] {
            assert_eq!(
                sanitize_text(&format!("a{c}b")),
                "a b",
                "U+{:04X}",
                c as u32
            );
        }
        assert_eq!(sanitize_text("branch main ✓"), "branch main ✓");
    }

    #[test]
    fn saysdf_fit_right_drops_leftmost() {
        let f = fit_items(&[4], &[5, 5, 5], 11);
        assert_eq!(f.right, vec![1, 2]);
        assert!(f.left.is_empty());
        assert_eq!(f.overflow, None);
    }

    #[test]
    fn saysdf_truncate_cells() {
        assert_eq!(truncate_cells("hello", 10), "hello");
        assert_eq!(truncate_cells("hello", 4), "hel…");
        assert_eq!(truncate_cells("hello", 3), "he…");
        assert_eq!(truncate_cells("xy", 0), "");
    }

    fn lua_spec(kind: BarKind, code: &str) -> mlua::Result<Option<BarSpec>> {
        let lua = mlua::Lua::new();
        let value: mlua::Value = lua.load(code).eval()?;
        parse_spec(kind, value)
    }

    #[test]
    fn saysdf_parse_menu_spec() {
        let spec = lua_spec(
            BarKind::Top,
            r#"{ left = { { text = "&Git", id = "menu.git", children = {
                    { text = "Commit", id = "git.commit", key = "Ctrl+G", accel = "c" },
                    { text = "&Push\tCtrl+Shift+U", id = "git.push", accel = "p" },
                    { separator = true },
                    "Disabled",
                } }, "View" },
                right = { { text = "main", id = "git.branch" } } }"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.left.len(), 2);
        let git = &spec.left[0];
        assert_eq!(git.text, "&Git");
        assert_eq!(git.id.as_deref(), Some("menu.git"));
        assert_eq!(git.children.len(), 4);
        assert_eq!(git.children[0].key.as_deref(), Some("Ctrl+G"));
        assert_eq!(git.children[1].text, "&Push");
        assert_eq!(git.children[1].key.as_deref(), Some("Ctrl+Shift+U"));
        assert!(git.children[2].separator);
        assert_eq!(git.children[3].text, "Disabled");
        assert_eq!(git.children[3].id, None);
        assert_eq!(spec.left[1].text, "View");
        assert_eq!(spec.right[0].id.as_deref(), Some("git.branch"));
    }

    #[test]
    fn saysdf_parse_nil_and_empty_clear() {
        assert_eq!(lua_spec(BarKind::Top, "nil").unwrap(), None);
        assert_eq!(lua_spec(BarKind::Bottom, "{}").unwrap(), None);
        assert_eq!(lua_spec(BarKind::Bottom, "{ left = {} }").unwrap(), None);
    }

    #[test]
    fn saysdf_parse_errors() {
        assert!(lua_spec(BarKind::Top, "42").is_err());
        assert!(lua_spec(BarKind::Top, "{ left = { { id = 'x' } } }").is_err());
        assert!(lua_spec(BarKind::Top, "{ left = 'x' }").is_err());
        assert!(lua_spec(BarKind::Top, "{ left = { { text = 1 } } }").is_err());
        assert!(parse_kind("side").is_err());
        assert_eq!(parse_kind("banner").unwrap(), BarKind::Banner);
    }

    #[test]
    fn saysdf_parse_limits_and_sanitize() {
        let spec = lua_spec(
            BarKind::Banner,
            r#"{ left = { "a\27[31mb", "2", "3", "4", "5", "6", "7" }, right = { "x" }, timeout = 5 }"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.left.len(), MAX_BANNER_LINES);
        assert_eq!(spec.left[0].text, "a [31mb");
        assert!(spec.right.is_empty());
        assert_eq!(spec.timeout_secs, Some(5));

        let long = lua_spec(BarKind::Top, "{ left = { string.rep('x', 1000) } }")
            .unwrap()
            .unwrap();
        assert_eq!(long.left[0].text.chars().count(), MAX_TEXT_CHARS);

        let many = lua_spec(
            BarKind::Top,
            "local t = {} for i = 1, 500 do t[i] = tostring(i) end return { left = t }",
        )
        .unwrap()
        .unwrap();
        assert_eq!(many.left.len(), MAX_ITEMS);
    }

    #[test]
    fn saysdf_parse_menu_selector() {
        let lua = mlua::Lua::new();
        assert_eq!(parse_menu_selector(None).unwrap(), MenuSelector::First);
        assert_eq!(
            parse_menu_selector(Some(lua.load("2").eval().unwrap())).unwrap(),
            MenuSelector::Index(2)
        );
        assert_eq!(
            parse_menu_selector(Some(lua.load("'Git'").eval().unwrap())).unwrap(),
            MenuSelector::Name("Git".into())
        );
        assert!(parse_menu_selector(Some(lua.load("0").eval().unwrap())).is_err());
    }
}
