//! Dangerous-mode screen logic without drawing (design D5): UI actions -> `WriteMode`,
//! frame layout (the banner is the first line, outside scrolling), banner text.

use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;
use saysdf_db::writemode::{ButtonActivated, ResetCause, WriteGrant, WriteMode};
use saysdf_render::sanitize::str_width;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-db");

/// Where keyboard input goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The "Allow writes" / "Leave dangerous mode" toolbar button.
    WriteButton,
    Tree,
    /// Right pane: structure, data or the query editor.
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    /// Mouse click on the toolbar button, or Enter while it has focus. There are no other sources.
    WriteButton,
    /// Database name typed in the confirmation dialog.
    ConfirmSubmit(String),
    ConfirmCancel,
    /// Click on the banner.
    BannerClick,
    /// F12 or Esc Esc leaves dangerous mode (leaving by hotkey is safe).
    ExitKey,
    Reconnect,
    SwitchConnection,
}

/// What the UI must do after a transition.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Show the "type the database name" dialog.
    AskConfirm(String),
    /// Reconnect without read-only.
    ReopenWrite(WriteGrant),
    /// Reconnect read-only.
    ReopenReadOnly,
    /// Wrong name in the confirmation.
    WrongName,
}

pub fn apply(mode: &mut WriteMode, db: &str, action: UiAction, reason: &str) -> Effect {
    match action {
        UiAction::WriteButton => {
            if mode.is_write() {
                mode.exit_write();
                return Effect::ReopenReadOnly;
            }
            mode.request_write(ButtonActivated::WriteButton, db);
            match mode {
                WriteMode::ConfirmPending { db } => Effect::AskConfirm(db.clone()),
                _ => Effect::None,
            }
        }
        UiAction::ConfirmSubmit(typed) => {
            let pending = matches!(mode, WriteMode::ConfirmPending { .. });
            match mode.confirm(&typed, reason) {
                Some(g) => Effect::ReopenWrite(g),
                None if pending => Effect::WrongName,
                None => Effect::None,
            }
        }
        UiAction::ConfirmCancel => {
            mode.cancel();
            Effect::None
        }
        UiAction::BannerClick | UiAction::ExitKey => {
            if mode.is_write() {
                mode.exit_write();
                Effect::ReopenReadOnly
            } else {
                Effect::None
            }
        }
        UiAction::Reconnect => {
            mode.reset(ResetCause::Reconnect);
            Effect::ReopenReadOnly
        }
        UiAction::SwitchConnection => {
            mode.reset(ResetCause::SwitchConnection);
            Effect::None
        }
    }
}

/// Key -> dangerous-mode action. `prev_esc`: the previous key was Esc.
/// From the keyboard, writes can be enabled only with Enter on the focused button; never from
/// the query editor, the tree or the grid.
pub fn key_action(
    focus: Focus,
    code: KeyCode,
    _mods: KeyModifiers,
    prev_esc: bool,
) -> Option<UiAction> {
    match code {
        KeyCode::Enter if focus == Focus::WriteButton => Some(UiAction::WriteButton),
        KeyCode::F(12) => Some(UiAction::ExitKey),
        KeyCode::Esc if prev_esc => Some(UiAction::ExitKey),
        _ => None,
    }
}

/// Frame areas. The banner is the first line, outside any scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    pub banner: Option<Rect>,
    pub toolbar: Rect,
    pub body: Rect,
    pub status: Rect,
}

pub fn areas(mode: &WriteMode, area: Rect) -> Areas {
    let bh = u16::from(mode.is_write()).min(area.height);
    let th = (area.height - bh).min(1);
    let rest = area.height - bh - th;
    let sh = u16::from(rest >= 2);
    let bodyh = rest - sh;
    let row = |y: u16, h: u16| Rect::new(area.x, area.y + y, area.width, h);
    Areas {
        banner: mode.is_write().then(|| row(0, bh)),
        toolbar: row(bh, th),
        body: row(bh + th, bodyh),
        status: row(bh + th + bodyh, sh),
    }
}

/// Visible content lines for scroll offset `scroll`: only within `body`.
pub fn body_window(body: Rect, scroll: usize, total: usize) -> std::ops::Range<usize> {
    let start = scroll.min(total);
    start..(start + body.height as usize).min(total)
}

/// Banner text: (main label, explanation). Shortened when narrow; widths are display widths.
pub fn banner_text(mode: &WriteMode, width: u16) -> Option<(String, String)> {
    let WriteMode::Write { db, reason, .. } = mode else {
        return None;
    };
    let full = format!(
        " {} ",
        I18N.f(
            "DANGEROUS MODE (writes to {db} are allowed)",
            &[("db", db)]
        )
    );
    let explain = format!(
        " {}",
        I18N.f(
            "queries change data without read-only protection{reason}; leave: click the label, F12 or Esc Esc",
            &[(
                "reason",
                &if reason.is_empty() { String::new() } else { format!(", {reason}") }
            )]
        )
    );
    if str_width(&full) <= width as usize {
        Some((full, explain))
    } else {
        Some((
            format!(" {} ", I18N.f("DANGEROUS MODE: {db}", &[("db", db)])),
            String::new(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mode() -> WriteMode {
        let mut m = WriteMode::default();
        assert_eq!(
            apply(&mut m, "app.db", UiAction::WriteButton, ""),
            Effect::AskConfirm("app.db".into())
        );
        assert!(matches!(
            apply(
                &mut m,
                "app.db",
                UiAction::ConfirmSubmit("app.db".into()),
                "r"
            ),
            Effect::ReopenWrite(_)
        ));
        m
    }

    #[test]
    fn banner_present_in_every_scroll_state_when_write() {
        let m = write_mode();
        for h in 1..12u16 {
            let area = Rect::new(0, 0, 80, h);
            for scroll in 0..200usize {
                let a = areas(&m, area);
                let b = a.banner.expect("banner is present");
                assert_eq!((b.y, b.height), (0, 1), "h={h} scroll={scroll}");
                assert!(a.body.y >= 1 || a.body.height == 0);
                let w = body_window(a.body, scroll, 500);
                assert!(w.len() <= a.body.height as usize);
                assert!(banner_text(&m, area.width)
                    .unwrap()
                    .0
                    .contains("DANGEROUS MODE (writes to app.db are allowed)"));
            }
        }
        assert!(areas(&WriteMode::ReadOnly, Rect::new(0, 0, 80, 10))
            .banner
            .is_none());
        assert!(banner_text(&WriteMode::ReadOnly, 80).is_none());
        assert_eq!(banner_text(&m, 10).unwrap().0, " DANGEROUS MODE: app.db ");
    }

    #[test]
    fn banner_text_english_matches_spec_and_russian_is_unchanged() {
        let m = write_mode();
        let (main, explain) = banner_text(&m, 80).unwrap();
        assert_eq!(main, " DANGEROUS MODE (writes to app.db are allowed) ");
        assert!(explain.contains("leave: click the label, F12 or Esc Esc"), "{explain}");
        assert!(explain.contains(", r;"), "reason is shown: {explain}");
        let _lang = saysdf_i18n::testing::use_lang("ru");
        let (main, explain) = banner_text(&m, 80).unwrap();
        assert_eq!(main, " ОПАСНЫЙ РЕЖИМ (запись в app.db разрешена) "); // i18n-allow-cyrillic
        assert!(explain.contains("выход: клик по надписи, F12 или Esc Esc"), "{explain}"); // i18n-allow-cyrillic
        assert_eq!(banner_text(&m, 10).unwrap().0, " ОПАСНЫЙ РЕЖИМ: app.db "); // i18n-allow-cyrillic
    }

    #[test]
    fn banner_fits_by_display_width_in_both_languages() {
        // A database name of double-width characters: byte and char counts both lie.
        let mut m = WriteMode::default();
        apply(&mut m, "データベース", UiAction::WriteButton, "");
        apply(&mut m, "データベース", UiAction::ConfirmSubmit("データベース".into()), "");
        for lang in ["en", "ru"] {
            let _lang = saysdf_i18n::testing::use_lang(lang);
            for width in 5..90u16 {
                let (main, _) = banner_text(&m, width).unwrap();
                let full = str_width(&main) <= width as usize;
                // Either the full banner fits, or the short form with the name is used.
                assert!(full || main.contains("データベース"), "{lang} {width}: {main}");
                if main.contains('(') {
                    assert!(full, "{lang} {width}: {main}");
                }
            }
        }
    }

    #[test]
    fn keys_never_enable_write_outside_button() {
        let codes = [
            KeyCode::Enter,
            KeyCode::Char('w'),
            KeyCode::Char('W'),
            KeyCode::F(5),
            KeyCode::F(12),
            KeyCode::Tab,
            KeyCode::Esc,
        ];
        for focus in [Focus::Tree, Focus::Right] {
            for c in codes {
                for mods in [
                    KeyModifiers::NONE,
                    KeyModifiers::CONTROL,
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                    KeyModifiers::ALT,
                ] {
                    for prev in [false, true] {
                        assert_ne!(
                            key_action(focus, c, mods, prev),
                            Some(UiAction::WriteButton)
                        );
                    }
                }
            }
        }
        assert_eq!(
            key_action(
                Focus::WriteButton,
                KeyCode::Enter,
                KeyModifiers::NONE,
                false
            ),
            Some(UiAction::WriteButton)
        );
        assert_eq!(
            key_action(Focus::Right, KeyCode::Esc, KeyModifiers::NONE, false),
            None
        );
        assert_eq!(
            key_action(Focus::Right, KeyCode::Esc, KeyModifiers::NONE, true),
            Some(UiAction::ExitKey)
        );
    }

    #[test]
    fn transitions() {
        let mut m = WriteMode::default();
        assert_eq!(apply(&mut m, "db", UiAction::ExitKey, ""), Effect::None);
        apply(&mut m, "db", UiAction::WriteButton, "");
        assert_eq!(
            apply(&mut m, "db", UiAction::ConfirmSubmit("DB".into()), ""),
            Effect::WrongName
        );
        assert_eq!(m, WriteMode::ReadOnly);
        apply(&mut m, "db", UiAction::WriteButton, "");
        apply(&mut m, "db", UiAction::ConfirmCancel, "");
        assert_eq!(m, WriteMode::ReadOnly);

        let mut m = write_mode();
        assert_eq!(
            apply(&mut m, "app.db", UiAction::BannerClick, ""),
            Effect::ReopenReadOnly
        );
        assert_eq!(m, WriteMode::ReadOnly);
        let mut m = write_mode();
        assert_eq!(
            apply(&mut m, "app.db", UiAction::ExitKey, ""),
            Effect::ReopenReadOnly
        );
        let mut m = write_mode();
        assert_eq!(
            apply(&mut m, "app.db", UiAction::WriteButton, ""),
            Effect::ReopenReadOnly
        );
        assert_eq!(m, WriteMode::ReadOnly);
        let mut m = write_mode();
        apply(&mut m, "app.db", UiAction::Reconnect, "");
        assert_eq!(m, WriteMode::ReadOnly);
        let mut m = write_mode();
        apply(&mut m, "app.db", UiAction::SwitchConnection, "");
        assert_eq!(m, WriteMode::ReadOnly);
    }
}
