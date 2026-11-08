use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use saysdf_keys::{Key, Pressed};
use std::io;
use wezterm_input_types::Modifiers;

pub use saysdf_keys::CONFIRM_HINT;

pub fn read() -> io::Result<(Event, Option<Key>)> {
    let ev = event::read()?;
    let phys = match &ev {
        Event::Key(k) => phys(k),
        _ => None,
    };
    Ok((ev, phys))
}

// base_layout: kitty alternate keys on unix, the virtual key code on Windows (vendored crossterm)
#[cfg(any(unix, windows))]
pub(crate) fn phys(k: &KeyEvent) -> Option<Key> {
    let base = k.base_layout? as u32;
    saysdf_keys::from_kitty(&format!(
        "{base}::{base};{}",
        1 + kitty_mod_bits(k.modifiers)
    ))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn phys(_k: &KeyEvent) -> Option<Key> {
    None
}

#[cfg(any(unix, windows))]
fn kitty_mod_bits(m: KeyModifiers) -> u8 {
    [
        (KeyModifiers::SHIFT, 1),
        (KeyModifiers::ALT, 2),
        (KeyModifiers::CONTROL, 4),
        (KeyModifiers::SUPER, 8),
    ]
    .iter()
    .filter(|(flag, _)| m.contains(*flag))
    .map(|(_, bit)| bit)
    .sum()
}

fn modifiers(m: KeyModifiers) -> Modifiers {
    [
        (KeyModifiers::SHIFT, Modifiers::SHIFT),
        (KeyModifiers::ALT, Modifiers::ALT),
        (KeyModifiers::CONTROL, Modifiers::CTRL),
        (KeyModifiers::SUPER, Modifiers::SUPER),
    ]
    .iter()
    .filter(|(flag, _)| m.contains(*flag))
    .fold(Modifiers::NONE, |acc, (_, bit)| acc | *bit)
}

pub fn pressed(k: &KeyEvent, phys: Option<Key>) -> Option<Pressed> {
    match (phys, k.code) {
        (Some(key), _) => Some(Pressed::Phys(key)),
        (None, KeyCode::Char(c)) => Some(Pressed::Letter(c, modifiers(k.modifiers))),
        _ => None,
    }
}

pub fn is(k: &KeyEvent, phys: Option<Key>, spec: &str) -> io::Result<bool> {
    let Some(p) = pressed(k, phys) else {
        return Ok(false);
    };
    saysdf_keys::is(&p, spec)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("saysdf_keys::is: {e}")))
}

pub fn confirmed(k: &KeyEvent, phys: Option<Key>) -> bool {
    pressed(k, phys).and_then(|p| saysdf_keys::confirm_answer(&p)) == Some(true)
}

#[cfg(unix)]
pub fn push_flags() -> io::Result<()> {
    use ratatui::crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    ratatui::crossterm::execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    )
}

#[cfg(unix)]
pub fn pop_flags() -> io::Result<()> {
    ratatui::crossterm::execute!(
        io::stdout(),
        ratatui::crossterm::event::PopKeyboardEnhancementFlags
    )
}

#[cfg(not(unix))]
pub fn push_flags() -> io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
pub fn pop_flags() -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wezterm_input_types::PhysKeyCode;

    fn ev(c: char, base: Option<char>) -> KeyEvent {
        let mut k = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        k.base_layout = base;
        k
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn base_layout_gives_physical_key() {
        assert_eq!(
            phys(&ev('н', Some('y'))),
            Some(Key::new(PhysKeyCode::Y, Modifiers::NONE))
        );
        assert_eq!(phys(&ev('y', None)), None);
    }

    #[cfg(windows)]
    #[test]
    fn ctrl_with_cyrillic_char_is_physical_key() {
        let mut k = KeyEvent::new(KeyCode::Char('к'), KeyModifiers::CONTROL);
        k.base_layout = Some('r');
        assert_eq!(phys(&k), Some(Key::new(PhysKeyCode::R, Modifiers::CTRL)));
        // key specs name letters in lowercase; "Ctrl+R" means Ctrl+Shift+R
        assert!(is(&k, phys(&k), "Ctrl+r").unwrap());
        assert!(!is(&k, phys(&k), "Ctrl+R").unwrap());
    }

    // every character the vendored Windows parser puts into base_layout from a virtual key code
    // (crossterm `base_layout_for_vk`; the vendored crate's own test cannot run outside the workspace)
    #[cfg(any(unix, windows))]
    #[test]
    fn every_vk_base_layout_char_is_a_physical_key() {
        use PhysKeyCode as P;
        let table = [
            (' ', P::Space),
            (';', P::Semicolon),
            ('=', P::Equal),
            (',', P::Comma),
            ('-', P::Minus),
            ('.', P::Period),
            ('/', P::Slash),
            ('`', P::Grave),
            ('[', P::LeftBracket),
            ('\\', P::Backslash),
            (']', P::RightBracket),
            ('\'', P::Quote),
            ('0', P::K0),
            ('9', P::K9),
            ('a', P::A),
            ('r', P::R),
            ('z', P::Z),
        ];
        for (c, code) in table {
            let mut k = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
            k.base_layout = Some(c);
            assert_eq!(phys(&k), Some(Key::new(code, Modifiers::CTRL)), "base {c:?}");
        }
        for c in ('0'..='9').chain('a'..='z') {
            assert!(phys(&ev(c, Some(c))).is_some(), "base {c:?}");
        }
    }

    #[test]
    fn confirm_is_physical_y_or_typed_y() {
        let y = Some(Key::new(PhysKeyCode::Y, Modifiers::NONE));
        assert!(confirmed(&ev('н', Some('y')), y));
        assert!(confirmed(&ev('y', None), None));
        assert!(!confirmed(&ev('д', None), None));
        assert!(!confirmed(&ev('n', None), None));
    }
}
