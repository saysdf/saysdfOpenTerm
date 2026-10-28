//! saysdfTerminal: text of close-confirmation dialogs and the Quit check
//! (change close-confirmation, design D2, D3). Fork file: upstream only has calls.
use mux::pane::{CachePolicy, Pane};
use mux::tab::Tab;
use mux::window::WindowId;
use mux::Mux;
use std::collections::HashSet;
use std::sync::Arc;

/// How many process names to show; the rest — "and N more".
const MAX_NAMES: usize = 5;

/// What is being closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseKind {
    Pane,
    Tab,
    Window,
    Application { windows: usize },
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Dialog text: what is being closed, number of tabs/panes and running processes.
/// `names` — already without shells from the skip list; duplicates removed, order preserved.
pub fn format_close_message(kind: CloseKind, tabs: usize, panes: usize, names: &[String]) -> String {
    let mut out = match kind {
        CloseKind::Pane => "🛑 Close this pane?".to_string(),
        CloseKind::Tab => "🛑 Close this tab?".to_string(),
        CloseKind::Window => "🛑 Close this window?".to_string(),
        CloseKind::Application { .. } => format!("🛑 Quit {}?", saysdf_identity::APP_NAME),
    };

    let mut counts = vec![];
    match kind {
        CloseKind::Pane => {}
        CloseKind::Tab => {
            if panes > 1 {
                counts.push(plural(panes, "pane"));
            }
        }
        CloseKind::Window => {
            counts.push(plural(tabs, "tab"));
            counts.push(plural(panes, "pane"));
        }
        CloseKind::Application { windows } => {
            counts.push(plural(windows, "window"));
            counts.push(plural(tabs, "tab"));
            counts.push(plural(panes, "pane"));
        }
    }
    if !counts.is_empty() {
        out.push(' ');
        out.push_str(&counts.join(", "));
        out.push('.');
    }

    let mut seen = HashSet::new();
    let unique: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| seen.insert(*n))
        .collect();
    if unique.is_empty() {
        out.push_str(" No running processes.");
    } else {
        out.push_str(" Running: ");
        out.push_str(&unique[..unique.len().min(MAX_NAMES)].join(", "));
        if unique.len() > MAX_NAMES {
            out.push_str(&format!(" and {} more", unique.len() - MAX_NAMES));
        }
        out.push('.');
    }
    out
}

/// Name of the pane's foreground process for the dialog, or None if it is a shell from the skip list.
fn process_name(pane: &Arc<dyn Pane>, skip: &[String]) -> Option<String> {
    let path = pane.get_foreground_process_name(CachePolicy::AllowStale)?;
    let base = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(path);
    if skip.iter().any(|s| *s == base) {
        return None;
    }
    let display = match base.len().checked_sub(4) {
        Some(i) if base.is_char_boundary(i) && base[i..].eq_ignore_ascii_case(".exe") => {
            base[..i].to_string()
        }
        _ => base,
    };
    Some(display)
}

#[derive(Default)]
struct Summary {
    tabs: usize,
    panes: usize,
    names: Vec<String>,
}

impl Summary {
    fn add_pane(&mut self, pane: &Arc<dyn Pane>, skip: &[String]) {
        self.panes += 1;
        if let Some(name) = process_name(pane, skip) {
            self.names.push(name);
        }
    }

    fn add_tab(&mut self, tab: &Arc<Tab>, skip: &[String]) {
        self.tabs += 1;
        for pos in tab.iter_panes_ignoring_zoom() {
            self.add_pane(&pos.pane, skip);
        }
    }

    fn add_window(&mut self, mux: &Mux, window_id: WindowId, skip: &[String]) {
        if let Some(window) = mux.get_window(window_id) {
            for tab in window.iter_tabs() {
                self.add_tab(tab, skip);
            }
        }
    }

    fn message(&self, kind: CloseKind) -> String {
        format_close_message(kind, self.tabs, self.panes, &self.names)
    }
}

fn skip_list() -> Vec<String> {
    config::configuration()
        .skip_close_confirmation_for_processes_named
        .clone()
}

pub fn pane_message(pane: &Arc<dyn Pane>) -> String {
    let mut s = Summary::default();
    s.add_pane(pane, &skip_list());
    s.message(CloseKind::Pane)
}

pub fn tab_message(tab: &Arc<Tab>) -> String {
    let mut s = Summary::default();
    s.add_tab(tab, &skip_list());
    s.message(CloseKind::Tab)
}

pub fn window_message(window_id: WindowId) -> String {
    let mut s = Summary::default();
    s.add_window(&Mux::get(), window_id, &skip_list());
    s.message(CloseKind::Window)
}

/// Message for Quit: tabs and panes across all windows.
pub fn quit_message() -> String {
    let mux = Mux::get();
    let skip = skip_list();
    let windows = mux.iter_windows();
    let mut s = Summary::default();
    for id in &windows {
        s.add_window(&mux, *id, &skip);
    }
    s.message(CloseKind::Application {
        windows: windows.len(),
    })
}

/// D2: Quit without a dialog if every window can be closed without asking
/// (only-if-running-process mode; in always the `mux-is-process-stateful` hook returns true).
pub fn quit_without_prompt() -> bool {
    let mux = Mux::get();
    mux.iter_windows().into_iter().all(|id| {
        mux.get_window(id)
            .map_or(true, |w| w.can_close_without_prompting())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn window_with_counts_and_names() {
        assert_eq!(
            format_close_message(CloseKind::Window, 3, 4, &names(&["claude", "ssh"])),
            "🛑 Close this window? 3 tabs, 4 panes. Running: claude, ssh."
        );
    }

    #[test]
    fn pane_without_processes() {
        assert_eq!(
            format_close_message(CloseKind::Pane, 0, 1, &[]),
            "🛑 Close this pane? No running processes."
        );
    }

    #[test]
    fn tab_shows_panes_only_when_split() {
        assert_eq!(
            format_close_message(CloseKind::Tab, 1, 1, &names(&["vim"])),
            "🛑 Close this tab? Running: vim."
        );
        assert_eq!(
            format_close_message(CloseKind::Tab, 1, 2, &[]),
            "🛑 Close this tab? 2 panes. No running processes."
        );
    }

    #[test]
    fn application_singular_counts() {
        let msg = format_close_message(CloseKind::Application { windows: 1 }, 1, 1, &[]);
        assert_eq!(
            msg,
            format!(
                "🛑 Quit {}? 1 window, 1 tab, 1 pane. No running processes.",
                saysdf_identity::APP_NAME
            )
        );
    }

    #[test]
    fn dedup_and_truncate() {
        let msg = format_close_message(
            CloseKind::Window,
            7,
            8,
            &names(&["a", "b", "a", "c", "d", "e", "f", "g", "b"]),
        );
        assert_eq!(
            msg,
            "🛑 Close this window? 7 tabs, 8 panes. Running: a, b, c, d, e and 2 more."
        );
    }
}
