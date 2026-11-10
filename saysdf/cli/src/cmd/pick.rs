//! `saysdf pick` — a pick list in the side panel (change tool-sidebar, design D4) on the shared
//! pager: `/` — search (n/N — next), Tab/Shift+Tab — next/previous item, Enter — select,
//! Esc/q — cancel. The result is the user var `SAYSDF_SIDEBAR` (see `crate::sidebar`).

use crate::pager::{self, Content, Nav};
use saysdf_keys::Key;
use crate::sidebar::{self, Item};
use clap::Args;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;
use saysdf_render::doc::Doc;
use saysdf_render::sanitize::visible_line;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-main");

#[derive(Args)]
pub struct PickArgs {
    /// JSON `[{id,label}]` (the file is deleted after reading)
    #[arg(long)]
    pub items: PathBuf,
    /// Title (default: "Pick", translated)
    #[arg(long)]
    pub title: Option<String>,
    /// Deprecated: search is always fuzzy (kept for compatibility of callers)
    #[arg(long)]
    pub fuzzy: bool,
    /// Send `hover:<id>` when the cursor rests on an item (debounced; change theme-live-preview)
    #[arg(long)]
    pub hover: bool,
}

/// Debounce of `hover` messages: about 120 ms of a still cursor.
const HOVER_DELAY: Duration = Duration::from_millis(120);

/// Pure hover logic with an injected clock: last position wins, the initial position and the
/// id sent last are not sent.
pub(crate) struct HoverDebounce {
    delay: Duration,
    /// Position seen at the previous call (`None` before the first call).
    seen: Option<usize>,
    /// Position waiting for the cursor to be still, and when it last changed.
    pending: Option<(usize, Instant)>,
    /// Position last sent, or the initial one (never sent).
    sent: Option<usize>,
}

impl HoverDebounce {
    pub(crate) fn new(delay: Duration) -> Self {
        Self { delay, seen: None, pending: None, sent: None }
    }

    /// Cursor at `pos` at `now`; returns the position to send, if any.
    pub(crate) fn update(&mut self, pos: usize, now: Instant) -> Option<usize> {
        if self.seen.is_none() {
            self.seen = Some(pos);
            self.sent = Some(pos);
            return None;
        }
        if self.seen != Some(pos) {
            self.seen = Some(pos);
            self.pending = (self.sent != Some(pos)).then_some((pos, now));
            if self.pending.is_none() {
                return None;
            }
        }
        match self.pending {
            Some((p, since)) if now.saturating_duration_since(since) >= self.delay => {
                self.pending = None;
                self.sent = Some(p);
                Some(p)
            }
            _ => None,
        }
    }
}

pub(crate) struct PickContent {
    items: Vec<Item>,
    doc: Doc,
    token: Option<String>,
    /// saysdf.palette sidebar-action id: the choice is sent as `action` `<id>:<item id>`
    /// (`saysdf annotations`), not as `pick`.
    action: Option<&'static str>,
    /// Viewer launch (`annotations --result-file`): the choice is written here, nothing is sent.
    result_file: Option<std::path::PathBuf>,
    /// `--hover`: debounce state; `None` sends nothing.
    hover: Option<HoverDebounce>,
}

impl PickContent {
    pub(crate) fn new(items: Vec<Item>, token: Option<String>) -> Self {
        let doc = Doc::from_lines(items.iter().map(|i| Line::from(visible_line(&i.label, 4))).collect());
        Self { items, doc, token, action: None, result_file: None, hover: None }
    }

    /// Sends the choice as a sidebar action (see the field).
    pub(crate) fn with_action(mut self, action: &'static str) -> Self {
        self.action = Some(action);
        self
    }

    /// Writes the choice to `path` (atomically) instead of sending it.
    pub(crate) fn with_result_file(mut self, path: std::path::PathBuf) -> Self {
        self.result_file = Some(path);
        self
    }

    /// Enables `hover` messages (needs a token).
    pub(crate) fn with_hover(mut self) -> Self {
        self.hover = Some(HoverDebounce::new(HOVER_DELAY));
        self
    }

    /// id of the item on line `pos`.
    pub(crate) fn choose(&self, pos: usize) -> Option<&str> {
        self.items.get(pos).map(|i| i.id.as_str())
    }

    /// Restores the terminal, sends the result and exits the process (the panel closes).
    fn finish(token: Option<&str>, action: Option<&str>, result_file: Option<&std::path::Path>, id: &str) -> ! {
        let _ = pager::suspend(|| {
            if let Some(f) = result_file {
                if let Err(e) = crate::fsutil::write_atomic(f, id.as_bytes(), true) {
                    eprintln!("saysdf pick: {e}");
                }
            } else if let Some(t) = token {
                let sent = match action {
                    Some(a) => sidebar::send(t, "action", &format!("{a}:{id}")),
                    None => sidebar::send(t, "pick", id),
                };
                if let Err(e) = sent {
                    eprintln!("saysdf pick: {e}");
                }
            } else {
                println!("{id}");
            }
        });
        std::process::exit(0)
    }
}

impl Content for PickContent {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
        (from..to.min(self.doc.len())).map(|i| Line::from(self.doc.line_text(i))).collect()
    }

    fn doc(&self) -> Option<&Doc> {
        Some(&self.doc)
    }

    fn picker(&self) -> bool {
        true
    }

    fn cursor_moved(&mut self, pos: usize) {
        let (Some(h), Some(t)) = (self.hover.as_mut(), self.token.as_deref()) else { return };
        if let Some(id) = h.update(pos, Instant::now()).and_then(|p| self.items.get(p)) {
            // Best effort: a lost hover is superseded by the next one or by the final pick.
            let _ = sidebar::send_once(t, "hover", &id.id);
        }
    }

    fn key(&mut self, k: &KeyEvent, _phys: Option<Key>, pos: usize) -> io::Result<Nav> {

        let last = self.items.len().saturating_sub(1);
        Ok(match k.code {
            KeyCode::Enter => match self.choose(pos) {
                Some(id) => Self::finish(self.token.as_deref(), self.action, self.result_file.as_deref(), id),
                None => Nav::Status(I18N.t("empty").into_owned()),
            },
            KeyCode::Tab => Nav::Goto((pos + 1).min(last)),
            KeyCode::BackTab => Nav::Goto(pos.saturating_sub(1)),
            _ => Nav::None,
        })
    }
}

pub fn run(args: PickArgs) -> i32 {
    // The token is taken before any child process (D3); without a token the choice goes to stdout.
    let token = sidebar::take_token();
    let items = match sidebar::read_items(&args.items) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("saysdf pick: {e}");
            return 1;
        }
    };
    let n = items.len();
    let mut content = PickContent::new(items, token.clone());
    if args.hover && token.is_some() {
        content = content.with_hover();
    }
    let opts = pager::Options {
        path: None,
        title: match &args.title {
            Some(t) => visible_line(t, 4),
            None => I18N.t("Pick").into_owned(),
        },
        line: (n > 0).then_some(0),
        col: 1,
        editor: None,
        watch: false,
        status: I18N.t("↑↓ item, Enter select, / search, Esc cancel").into_owned(),
    };
    let res = pager::run(content, opts);
    // Reached only on cancel (Esc/q): Enter exits the process in finish().
    if let Some(t) = &token {
        if let Err(e) = sidebar::send(t, "pick", "") {
            eprintln!("saysdf pick: {e}");
        }
    }
    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("saysdf pick: {}", I18N.f("terminal: {error}", &[("error", &e)]));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn items() -> Vec<Item> {
        vec![
            Item { id: "main".into(), label: "Branch main".into() },
            Item { id: "dev".into(), label: "Branch dev".into() },
        ]
    }

    #[test]
    fn choose_by_position() {
        let c = PickContent::new(items(), None);
        assert_eq!(c.choose(0), Some("main"));
        assert_eq!(c.choose(1), Some("dev"));
        assert_eq!(c.choose(2), None);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn tab_moves_cursor() {
        let mut c = PickContent::new(items(), None);
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(matches!(c.key(&tab, None, 0).unwrap(), Nav::Goto(1)));
        assert!(matches!(c.key(&tab, None, 1).unwrap(), Nav::Goto(1)));
        let back = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert!(matches!(c.key(&back, None, 0).unwrap(), Nav::Goto(0)));
    }

    #[test]
    fn hover_debounce_last_wins_and_skips_initial_and_repeats() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut h = HoverDebounce::new(Duration::from_millis(120));
        // initial position is never sent
        assert_eq!(h.update(0, ms(0)), None);
        assert_eq!(h.update(0, ms(500)), None);
        // quick moves: only the last one, after the delay
        assert_eq!(h.update(1, ms(600)), None);
        assert_eq!(h.update(2, ms(650)), None);
        assert_eq!(h.update(3, ms(700)), None);
        assert_eq!(h.update(3, ms(800)), None);
        assert_eq!(h.update(3, ms(820)), Some(3));
        // sent once
        assert_eq!(h.update(3, ms(1000)), None);
        // moving away and back to the last sent id sends nothing
        assert_eq!(h.update(4, ms(1100)), None);
        assert_eq!(h.update(3, ms(1150)), None);
        assert_eq!(h.update(3, ms(1400)), None);
        // back to the initial position is sent (it is not the last sent id)
        assert_eq!(h.update(0, ms(1500)), None);
        assert_eq!(h.update(0, ms(1630)), Some(0));
    }

    #[test]
    fn hover_off_without_flag_and_pick_unaffected() {
        let mut c = PickContent::new(items(), Some("0123abcd-89ef".into()));
        assert!(c.hover.is_none());
        c.cursor_moved(1); // no flag: nothing happens, no panic
        assert_eq!(c.choose(1), Some("dev"));
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(matches!(c.key(&tab, None, 0).unwrap(), Nav::Goto(1)));
        let c = PickContent::new(items(), None).with_hover();
        assert!(c.hover.is_some());
    }

    #[test]
    fn search_finds_label() {
        let c = PickContent::new(items(), None);
        let m = c.doc().unwrap().search("dev", None, saysdf_render::doc::Direction::Forward).unwrap();
        assert_eq!(c.choose(m.line), Some("dev"));
    }
}
