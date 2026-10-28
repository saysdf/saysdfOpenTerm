//! saysdfTerminal: drop-down menu of the `top` bar (change menu-bar, D2) — a `Modal` modeled on
//! `palette.rs`: frame under the header, rows `item … hotkey`, up/down arrows — items,
//! left/right — neighbor menu, Enter — run, Esc/F9 — close, the item's physical key
//! (`accel`, shown as `Name [Q]`) — run the item, mouse click (routing — `saysdf_bars::saysdf_mouse_event`).
//! Data is a snapshot of the top bar at the time of opening; the action is the `saysdf-bar-action` event.

use crate::termwindow::box_model::*;
use crate::termwindow::modal::Modal;
use crate::termwindow::saysdf_bars::{
    box_element, box_row, box_row_colors, colors, plain_mods, text_cells, BarColors, BarItem,
    BarKind, BarSide, BarSpec, MenuSelector, SaysdfBarHit,
};
use crate::termwindow::{TermWindow, UIItemType};
use config::saysdf_keys::{display, is, parse, Key, ParseError, Pressed};
use config::Dimension;
use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use wezterm_term::{KeyCode, KeyModifiers, MouseEvent};

// ---------------------------------------------------------------------------------------------
// Accelerators
// ---------------------------------------------------------------------------------------------

/// Explicit accelerator: `&X` → letter X (symbol index in the text without `&`), `&&` → literal `&`.
pub fn parse_accel(text: &str) -> (String, Option<usize>) {
    let mut out = String::new();
    let mut accel = None;
    let mut chars = text.chars().peekable();
    let mut count = 0;
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.peek() {
                Some('&') => {
                    chars.next();
                    out.push('&');
                    count += 1;
                }
                Some(&next) if accel.is_none() && next.is_alphanumeric() => {
                    accel = Some(count);
                }
                _ => {}
            }
            continue;
        }
        out.push(c);
        count += 1;
    }
    (out, accel)
}

pub fn strip_accel(text: &str) -> String {
    parse_accel(text).0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuAccel {
    pub spec: String,
    pub key: Key,
}

pub fn parse_menu_accel(spec: &str, id: Option<&str>) -> anyhow::Result<MenuAccel> {
    let key = parse(spec).map_err(|err| {
        anyhow::anyhow!(
            "saysdf_set_bar: menu entry {:?}: invalid accel: {err}",
            id.unwrap_or("")
        )
    })?;
    Ok(MenuAccel {
        spec: spec.to_string(),
        key,
    })
}

pub fn check_accels(menus: &[BarItem]) -> anyhow::Result<()> {
    for menu in menus {
        let title = strip_accel(&menu.text);
        let mut taken: HashMap<Key, &str> = HashMap::new();
        for entry in menu.children.iter().filter(|c| !texts_is_none(c)) {
            let id = entry.id.as_deref().unwrap_or("");
            let accel = entry.accel.as_ref().ok_or_else(|| {
                anyhow::anyhow!("saysdf_set_bar: menu entry {id:?} in menu {title:?} has no accel")
            })?;
            if let Some(holder) = taken.insert(accel.key, id) {
                anyhow::bail!(
                    "saysdf_set_bar: menu entry {id:?} in menu {title:?}: accel {} is already taken by entry {holder:?}",
                    display(&accel.key)
                );
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Menu model (pure)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub label: String,
    pub accel: Option<MenuAccel>,
    pub key: String,
    pub id: Option<String>,
    pub separator: bool,
}

impl MenuEntry {
    pub fn selectable(&self) -> bool {
        !self.separator && self.id.is_some()
    }
    pub fn accel_text(&self) -> Option<String> {
        self.accel
            .as_ref()
            .map(|a| format!(" [{}]", display(&a.key)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// Index of the header in `top.left`.
    pub title_idx: usize,
    pub entries: Vec<MenuEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Close,
    Accel(Pressed),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuCmd {
    Redraw,
    Close,
    Run(String),
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuModel {
    pub menus: Vec<Menu>,
    pub current: usize,
    pub selected: Option<usize>,
}

/// Index of the top bar header for `saysdf_open_menu(which)` (only headers with entries).
pub fn select_title(top: &BarSpec, which: &MenuSelector) -> Option<usize> {
    let mut menus = top
        .left
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.children.is_empty());
    match which {
        MenuSelector::First => menus.next().map(|(i, _)| i),
        MenuSelector::Index(n) => menus.nth(n.saturating_sub(1)).map(|(i, _)| i),
        MenuSelector::Name(name) => {
            let name = name.to_lowercase();
            menus
                .find(|(_, item)| {
                    strip_accel(&item.text).to_lowercase() == name
                        || item.id.as_deref().map_or(false, |id| id == name)
                })
                .map(|(i, _)| i)
        }
    }
}

impl MenuModel {
    /// `None` if the bar has no header with entries. If `title_idx` is not a menu
    /// (e.g. "…" past the edge), the nearest menu to the right of it is opened.
    pub fn new(top: &BarSpec, title_idx: usize) -> Option<Self> {
        let menus: Vec<Menu> = top
            .left
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.children.is_empty())
            .map(|(title_idx, item)| {
                let entries = item
                    .children
                    .iter()
                    .map(|c| MenuEntry {
                        label: strip_accel(&c.text),
                        accel: c.accel.clone().filter(|_| !texts_is_none(c)),
                        key: c.key.clone().unwrap_or_default(),
                        id: c.id.clone(),
                        separator: c.separator,
                    })
                    .collect();
                Menu { title_idx, entries }
            })
            .collect();
        if menus.is_empty() {
            return None;
        }
        let current = menus
            .iter()
            .position(|m| m.title_idx >= title_idx)
            .unwrap_or(0);
        let mut model = Self {
            menus,
            current,
            selected: None,
        };
        model.selected = model.first_selectable();
        Some(model)
    }

    pub fn menu(&self) -> &Menu {
        &self.menus[self.current]
    }

    fn first_selectable(&self) -> Option<usize> {
        self.menu().entries.iter().position(|e| e.selectable())
    }

    fn last_selectable(&self) -> Option<usize> {
        self.menu().entries.iter().rposition(|e| e.selectable())
    }

    fn step(&self, forward: bool) -> Option<usize> {
        let entries = &self.menu().entries;
        let n = entries.len();
        let start = match self.selected {
            Some(s) => s,
            None => return self.first_selectable(),
        };
        for k in 1..=n {
            let i = if forward {
                (start + k) % n
            } else {
                (start + n - k % n) % n
            };
            if entries[i].selectable() {
                return Some(i);
            }
        }
        self.selected
    }

    pub fn switch_to(&mut self, menu: usize) {
        self.current = menu % self.menus.len();
        self.selected = self.first_selectable();
    }

    /// Switch to the menu of header `title_idx`; `false` if it is already the current menu or
    /// the header has no entries.
    pub fn switch_to_title(&mut self, title_idx: usize) -> bool {
        match self.menus.iter().position(|m| m.title_idx == title_idx) {
            Some(i) if i != self.current => {
                self.switch_to(i);
                true
            }
            _ => false,
        }
    }

    pub fn row_action(&self, row: usize) -> Option<String> {
        self.menu()
            .entries
            .get(row)
            .filter(|e| e.selectable())
            .and_then(|e| e.id.clone())
    }

    fn accel_action(&self, pressed: &Pressed) -> Result<Option<String>, ParseError> {
        for e in self.menu().entries.iter().filter(|e| e.selectable()) {
            if let Some(accel) = &e.accel {
                if is(pressed, &accel.spec)? {
                    return Ok(e.id.clone());
                }
            }
        }
        Ok(None)
    }

    pub fn key(&mut self, key: MenuKey) -> Result<MenuCmd, ParseError> {
        match key {
            MenuKey::Close => return Ok(MenuCmd::Close),
            MenuKey::Up => self.selected = self.step(false),
            MenuKey::Down => self.selected = self.step(true),
            MenuKey::Home => self.selected = self.first_selectable(),
            MenuKey::End => self.selected = self.last_selectable(),
            MenuKey::Left => {
                let n = self.menus.len();
                self.switch_to(self.current + n - 1);
            }
            MenuKey::Right => self.switch_to(self.current + 1),
            MenuKey::Enter => {
                return Ok(match self.selected.and_then(|s| self.row_action(s)) {
                    Some(id) => MenuCmd::Run(id),
                    None => MenuCmd::Ignore,
                });
            }
            MenuKey::Accel(pressed) => {
                return Ok(match self.accel_action(&pressed)? {
                    Some(id) => MenuCmd::Run(id),
                    None => MenuCmd::Ignore,
                });
            }
        }
        Ok(MenuCmd::Redraw)
    }
}

/// Visible window of menu items when they do not fit in height: `avail` — rows that fit,
/// `selected` is always visible, the window shifts minimally from `prev_start`.
/// -> (start, end, whether there are items above, whether below). When scrolling, two rows go to
/// the ▲/▼ indicators.
pub fn scroll_view(
    total: usize,
    avail: usize,
    selected: Option<usize>,
    prev_start: usize,
) -> (usize, usize, bool, bool) {
    if total <= avail {
        return (0, total, false, false);
    }
    let cap = avail.saturating_sub(2).max(1);
    let mut start = prev_start.min(total - cap);
    if let Some(sel) = selected {
        if sel < start {
            start = sel;
        } else if sel >= start + cap {
            start = sel + 1 - cap;
        }
    }
    (start, start + cap, start > 0, start + cap < total)
}

fn texts_is_none(c: &BarItem) -> bool {
    c.separator || c.id.is_none()
}

pub fn menu_key(key: KeyCode, mods: KeyModifiers, phys: Option<Key>) -> Option<MenuKey> {
    Some(match key {
        KeyCode::UpArrow => MenuKey::Up,
        KeyCode::DownArrow => MenuKey::Down,
        KeyCode::LeftArrow => MenuKey::Left,
        KeyCode::RightArrow => MenuKey::Right,
        KeyCode::Home | KeyCode::PageUp => MenuKey::Home,
        KeyCode::End | KeyCode::PageDown => MenuKey::End,
        KeyCode::Enter => MenuKey::Enter,
        KeyCode::Escape | KeyCode::Function(9) => MenuKey::Close,
        KeyCode::Char('\r') | KeyCode::Char('\n') => MenuKey::Enter,
        KeyCode::Char(c) if plain_mods(mods) && !c.is_control() => MenuKey::Accel(match phys {
            Some(phys) => Pressed::Phys(phys),
            None => Pressed::Letter(c, mods),
        }),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Modal
// ---------------------------------------------------------------------------------------------

pub struct SaysdfMenu {
    model: RefCell<MenuModel>,
    element: RefCell<Option<Vec<ComputedElement>>>,
    /// First visible item if the menu does not fit in height.
    scroll: Cell<usize>,
}

impl SaysdfMenu {
    pub fn new(top: &BarSpec, title_idx: usize) -> Option<Self> {
        Some(Self {
            model: RefCell::new(MenuModel::new(top, title_idx)?),
            element: RefCell::new(None),
            scroll: Cell::new(0),
        })
    }

    fn apply(&self, cmd: MenuCmd, term_window: &mut TermWindow) {
        match cmd {
            MenuCmd::Ignore => {}
            MenuCmd::Redraw => {
                self.element.borrow_mut().take();
                term_window.invalidate_modal();
            }
            MenuCmd::Close => term_window.cancel_modal(),
            MenuCmd::Run(id) => {
                // the menu closes before the action: the action may open another overlay
                term_window.cancel_modal();
                term_window.saysdf_emit_bar_action(id);
            }
        }
    }

    pub fn click_row(&self, row: usize, term_window: &mut TermWindow) {
        let action = self.model.borrow().row_action(row);
        if let Some(id) = action {
            self.apply(MenuCmd::Run(id), term_window);
        }
    }

    pub fn hover_row(&self, row: usize, term_window: &mut TermWindow) {
        let changed = {
            let mut model = self.model.borrow_mut();
            if model.row_action(row).is_some() && model.selected != Some(row) {
                model.selected = Some(row);
                true
            } else {
                false
            }
        };
        if changed {
            self.apply(MenuCmd::Redraw, term_window);
        }
    }

    /// Mouse wheel over an open menu — select up/down (scrolling a long menu).
    pub fn wheel(&self, delta: i16, term_window: &mut TermWindow) {
        let key = if delta > 0 {
            MenuKey::Up
        } else {
            MenuKey::Down
        };
        let cmd = self.model.borrow_mut().key(key);
        match cmd {
            Ok(cmd) => self.apply(cmd, term_window),
            Err(err) => log::error!("saysdf menu: {err}"),
        }
    }

    pub fn switch_to_title(&self, title_idx: usize, term_window: &mut TermWindow) -> bool {
        let switched = self.model.borrow_mut().switch_to_title(title_idx);
        if switched {
            self.scroll.set(0);
            self.apply(MenuCmd::Redraw, term_window);
        }
        switched
    }

    fn compute(&self, term_window: &mut TermWindow) -> anyhow::Result<Vec<ComputedElement>> {
        let model = self.model.borrow();
        let menu = model.menu();
        let font = term_window.fonts.default_font()?;
        let metrics = term_window.render_metrics.clone();
        let cell_w = metrics.cell_size.width.max(1) as f32;
        let c = BarColors::from_config(&term_window.config);

        let label_w = menu
            .entries
            .iter()
            .map(|e| text_cells(&e.label) + e.accel_text().map_or(0, |a| text_cells(&a)))
            .max()
            .unwrap_or(0);
        let key_w = menu
            .entries
            .iter()
            .map(|e| text_cells(&e.key))
            .max()
            .unwrap_or(0);
        let inner_cells = 1 + label_w + if key_w > 0 { 3 + key_w } else { 0 } + 1;
        let inner_px = inner_cells as f32 * cell_w;

        // How many rows fit under the menu bar (1px frame at top and bottom)
        let (_, anchor_y) = term_window.saysdf_menu_anchor(menu.title_idx);
        let cell_h = metrics.cell_size.height.max(1) as f32;
        let avail = ((term_window.dimensions.pixel_height as f32 - anchor_y - 2.) / cell_h)
            .floor()
            .max(1.) as usize;
        let (start, end, more_up, more_down) =
            scroll_view(menu.entries.len(), avail, model.selected, self.scroll.get());
        self.scroll.set(start);
        let scrolling = end - start < menu.entries.len();
        let indicator = |on: bool, arrow: &str| {
            Element::new(
                &font,
                ElementContent::Text(if on {
                    format!(" {arrow}")
                } else {
                    " ".to_string()
                }),
            )
            .colors(colors(c.bar_bg, c.bar_fg.mul_alpha(0.6)))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(inner_px)))
        };

        let mut rows = vec![];
        if scrolling {
            rows.push(indicator(more_up, "▲"));
        }
        for (idx, entry) in menu.entries.iter().enumerate().take(end).skip(start) {
            let selected = model.selected == Some(idx);
            let (bg, fg) = box_row_colors(&c, selected);
            let dim = if entry.selectable() {
                fg
            } else {
                fg.mul_alpha(0.5)
            };
            let mut parts = vec![];
            if entry.separator {
                parts.push(
                    Element::new(&font, ElementContent::Text("─".repeat(inner_cells)))
                        .colors(colors(bg, dim)),
                );
            } else {
                let acc_fg = if selected { fg } else { c.hint };
                parts.push(
                    Element::new(&font, ElementContent::Text(format!(" {}", entry.label)))
                        .colors(colors(bg, dim)),
                );
                if let Some(acc) = entry.accel_text() {
                    parts.push(
                        Element::new(&font, ElementContent::Text(acc)).colors(colors(bg, acc_fg)),
                    );
                }
                if !entry.key.is_empty() {
                    parts.push(
                        Element::new(&font, ElementContent::Text(format!("{} ", entry.key)))
                            .colors(colors(bg, dim))
                            .float(Float::Right),
                    );
                }
            }
            rows.push(
                box_row(&font, parts, bg, fg, inner_px).item_type(UIItemType::SaysdfBar(
                    SaysdfBarHit {
                        bar: BarKind::Menu,
                        side: BarSide::Left,
                        idx,
                        version: 0,
                    },
                )),
            );
        }

        if scrolling {
            rows.push(indicator(more_down, "▼"));
        }

        let element = box_element(&font, rows, &c, inner_px);

        let (mut x, y) = term_window.saysdf_menu_anchor(menu.title_idx);
        let dims = term_window.dimensions;
        let total_w = inner_px + 2.;
        if x + total_w > dims.pixel_width as f32 {
            x = (dims.pixel_width as f32 - total_w).max(0.);
        }
        let height = (dims.pixel_height as f32 - y).max(0.);

        let computed =
            term_window.saysdf_compute_box(&element, euclid::rect(x, y, total_w, height), 100)?;
        Ok(vec![computed])
    }
}

impl Modal for SaysdfMenu {
    fn mouse_event(&self, _event: MouseEvent, _term_window: &mut TermWindow) -> anyhow::Result<()> {
        // mouse is routed in TermWindow::saysdf_mouse_event (by the window's ui_items)
        Ok(())
    }

    fn key_down(
        &self,
        key: KeyCode,
        mods: KeyModifiers,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<bool> {
        self.saysdf_key_down_phys(key, mods, None, term_window)
    }

    fn saysdf_key_down_phys(
        &self,
        key: KeyCode,
        mods: KeyModifiers,
        phys: Option<Key>,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<bool> {
        // While the menu is open, keys go neither to bindings nor to the program.
        if let Some(k) = menu_key(key, mods, phys) {
            let cmd = self.model.borrow_mut().key(k)?;
            self.apply(cmd, term_window);
        }
        Ok(true)
    }

    fn computed_element(
        &self,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<Ref<'_, [ComputedElement]>> {
        if self.element.borrow().is_none() {
            let element = self.compute(term_window)?;
            self.element.borrow_mut().replace(element);
        }
        Ok(Ref::map(self.element.borrow(), |v| {
            v.as_ref().unwrap().as_slice()
        }))
    }

    fn reconfigure(&self, _term_window: &mut TermWindow) {
        self.element.borrow_mut().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::termwindow::saysdf_bars::BarItem;

    fn item(text: &str, id: &str, accel: &str) -> BarItem {
        BarItem {
            text: text.into(),
            id: Some(id.into()),
            accel: Some(parse_menu_accel(accel, Some(id)).unwrap()),
            ..BarItem::default()
        }
    }

    fn sep() -> BarItem {
        BarItem {
            separator: true,
            ..BarItem::default()
        }
    }

    fn top() -> BarSpec {
        BarSpec {
            left: vec![
                BarItem {
                    text: "&File".into(),
                    children: vec![
                        item("New tab", "tabs.new", "t"),
                        sep(),
                        item("Close", "app.quit", "q"),
                    ],
                    ..BarItem::default()
                },
                BarItem {
                    text: "No menu".into(),
                    id: Some("plain".into()),
                    ..BarItem::default()
                },
                BarItem {
                    text: "Git".into(),
                    children: vec![
                        item("Commit", "git.commit", "c"),
                        item("Checkout", "git.checkout", "b"),
                        BarItem {
                            text: "Disabled".into(),
                            ..BarItem::default()
                        },
                        item("Push", "git.push", "p"),
                    ],
                    ..BarItem::default()
                },
            ],
            ..BarSpec::default()
        }
    }

    #[test]
    fn saysdf_menu_scroll_view() {
        // fits — no scrolling
        assert_eq!(scroll_view(5, 10, Some(4), 0), (0, 5, false, false));
        // 20 items, 7 rows: 5 items + 2 indicators
        assert_eq!(scroll_view(20, 7, Some(0), 0), (0, 5, false, true));
        // selection moved down — the window shifts minimally
        assert_eq!(scroll_view(20, 7, Some(6), 0), (2, 7, true, true));
        // back up inside the window — does not move
        assert_eq!(scroll_view(20, 7, Some(3), 2), (2, 7, true, true));
        // End
        assert_eq!(scroll_view(20, 7, Some(19), 2), (15, 20, true, false));
        // previous start is out of range after a menu change — clamped
        assert_eq!(scroll_view(8, 7, None, 30), (3, 8, true, false));
        // very little space — at least one item
        assert_eq!(scroll_view(20, 1, Some(10), 0), (10, 11, true, true));
    }

    #[test]
    fn saysdf_menu_accel_parse() {
        assert_eq!(parse_accel("&Commit"), ("Commit".into(), Some(0)));
        assert_eq!(parse_accel("Co&mmit"), ("Commit".into(), Some(2)));
        assert_eq!(parse_accel("A && B"), ("A & B".into(), None));
        assert_eq!(strip_accel("&File"), "File");
    }

    #[test]
    fn saysdf_menu_open_and_navigate() {
        let mut m = MenuModel::new(&top(), 0).unwrap();
        assert_eq!(m.menus.len(), 2);
        assert_eq!(m.current, 0);
        assert_eq!(m.selected, Some(0));
        // down skips the separator
        assert_eq!(m.key(MenuKey::Down).unwrap(), MenuCmd::Redraw);
        assert_eq!(m.selected, Some(2));
        // wraps around
        m.key(MenuKey::Down).unwrap();
        assert_eq!(m.selected, Some(0));
        m.key(MenuKey::Up).unwrap();
        assert_eq!(m.selected, Some(2));
        assert_eq!(
            m.key(MenuKey::Enter).unwrap(),
            MenuCmd::Run("app.quit".into())
        );
    }

    #[test]
    fn saysdf_menu_left_right_and_disabled() {
        let mut m = MenuModel::new(&top(), 0).unwrap();
        m.key(MenuKey::Right).unwrap();
        assert_eq!(m.current, 1);
        assert_eq!(m.menu().title_idx, 2);
        assert_eq!(m.selected, Some(0));
        m.key(MenuKey::Down).unwrap();
        m.key(MenuKey::Down).unwrap();
        // "Disabled" without id is skipped
        assert_eq!(m.selected, Some(3));
        m.key(MenuKey::Right).unwrap();
        assert_eq!(m.current, 0);
        m.key(MenuKey::Left).unwrap();
        assert_eq!(m.current, 1);
        assert_eq!(m.key(MenuKey::Close).unwrap(), MenuCmd::Close);
    }

    fn press(m: &mut MenuModel, c: char, mods: KeyModifiers, phys: Option<&str>) -> MenuCmd {
        let phys = phys.map(|p| parse(p).unwrap());
        m.key(menu_key(KeyCode::Char(c), mods, phys).unwrap())
            .unwrap()
    }

    #[test]
    fn saysdf_menu_physical_accel_on_both_layouts() {
        let mut m = MenuModel::new(&top(), 0).unwrap();
        let quit = MenuCmd::Run("app.quit".into());
        assert_eq!(press(&mut m, 'q', KeyModifiers::NONE, Some("q")), quit);
        assert_eq!(press(&mut m, 'й', KeyModifiers::NONE, Some("q")), quit); // i18n-allow-cyrillic
        assert_eq!(press(&mut m, 'q', KeyModifiers::NONE, None), quit);
        assert_eq!(
            press(&mut m, 'й', KeyModifiers::NONE, None), // i18n-allow-cyrillic
            MenuCmd::Ignore
        );
        assert_eq!(
            press(&mut m, 'Q', KeyModifiers::SHIFT, Some("Shift+q")),
            MenuCmd::Ignore
        );
        m.key(MenuKey::Right).unwrap();
        assert_eq!(
            press(&mut m, 'з', KeyModifiers::NONE, Some("p")), // i18n-allow-cyrillic
            MenuCmd::Run("git.push".into())
        );
        assert_eq!(
            press(&mut m, 'z', KeyModifiers::NONE, Some("z")),
            MenuCmd::Ignore
        );
        assert_eq!(m.menu().entries[0].accel_text().as_deref(), Some(" [C]"));
        assert_eq!(m.menu().entries[2].accel_text(), None);
    }

    #[test]
    fn saysdf_menu_accels_required_and_unique() {
        for spec in ["=", "-", "0", "[", "]", "y"] {
            parse_menu_accel(spec, None).unwrap();
        }
        let t = top();
        check_accels(&t.left).unwrap();
        let mut missing = t.clone();
        missing.left[0].children[0].accel = None;
        let err = check_accels(&missing.left).unwrap_err().to_string();
        assert!(
            err.contains("\"tabs.new\"") && err.contains("\"File\"") && err.contains("no accel"),
            "{}",
            err
        );
        let mut dup = t.clone();
        dup.left[2].children[3].accel = Some(parse_menu_accel("c", Some("git.push")).unwrap());
        let err = check_accels(&dup.left).unwrap_err().to_string();
        assert!(
            err.contains("\"git.push\"")
                && err.contains("\"git.commit\"")
                && err.contains("already taken"),
            "{}",
            err
        );
        assert!(parse_menu_accel("йй", Some("x.y")) // i18n-allow-cyrillic
            .unwrap_err()
            .to_string()
            .contains("\"x.y\""));
    }

    #[test]
    fn saysdf_menu_title_selection() {
        let t = top();
        assert_eq!(select_title(&t, &MenuSelector::First), Some(0));
        assert_eq!(select_title(&t, &MenuSelector::Index(2)), Some(2));
        assert_eq!(select_title(&t, &MenuSelector::Index(3)), None);
        assert_eq!(select_title(&t, &MenuSelector::Name("git".into())), Some(2));
        assert_eq!(
            select_title(&t, &MenuSelector::Name("File".into())),
            Some(0)
        );
        assert_eq!(
            select_title(&t, &MenuSelector::Name("No menu".into())),
            None
        );
        // header without entries ("…"/plain) — nearest menu to the right
        assert_eq!(MenuModel::new(&t, 1).unwrap().current, 1);
        let mut m = MenuModel::new(&t, 0).unwrap();
        assert!(m.switch_to_title(2));
        assert!(!m.switch_to_title(2));
        assert!(!m.switch_to_title(1));
        assert!(MenuModel::new(&BarSpec::default(), 0).is_none());
    }

    #[test]
    fn saysdf_menu_key_mapping() {
        assert_eq!(
            menu_key(KeyCode::Escape, KeyModifiers::NONE, None),
            Some(MenuKey::Close)
        );
        assert_eq!(
            menu_key(KeyCode::Function(9), KeyModifiers::NONE, None),
            Some(MenuKey::Close)
        );
        assert_eq!(
            menu_key(KeyCode::Char('g'), KeyModifiers::SHIFT, None),
            Some(MenuKey::Accel(Pressed::Letter('g', KeyModifiers::SHIFT)))
        );
        assert_eq!(menu_key(KeyCode::Char('g'), KeyModifiers::CTRL, None), None);
    }
}
