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
}

pub(crate) struct PickContent {
    items: Vec<Item>,
    doc: Doc,
    token: Option<String>,
}

impl PickContent {
    pub(crate) fn new(items: Vec<Item>, token: Option<String>) -> Self {
        let doc = Doc::from_lines(items.iter().map(|i| Line::from(visible_line(&i.label, 4))).collect());
        Self { items, doc, token }
    }

    /// id of the item on line `pos`.
    pub(crate) fn choose(&self, pos: usize) -> Option<&str> {
        self.items.get(pos).map(|i| i.id.as_str())
    }

    /// Restores the terminal, sends the result and exits the process (the panel closes).
    fn finish(token: Option<&str>, id: &str) -> ! {
        let _ = pager::suspend(|| {
            if let Some(t) = token {
                if let Err(e) = sidebar::send(t, "pick", id) {
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

    fn key(&mut self, k: &KeyEvent, _phys: Option<Key>, pos: usize) -> io::Result<Nav> {

        let last = self.items.len().saturating_sub(1);
        Ok(match k.code {
            KeyCode::Enter => match self.choose(pos) {
                Some(id) => Self::finish(self.token.as_deref(), id),
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
    let content = PickContent::new(items, token.clone());
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
    fn search_finds_label() {
        let c = PickContent::new(items(), None);
        let m = c.doc().unwrap().search("dev", None, saysdf_render::doc::Direction::Forward).unwrap();
        assert_eq!(c.choose(m.line), Some("dev"));
    }
}
