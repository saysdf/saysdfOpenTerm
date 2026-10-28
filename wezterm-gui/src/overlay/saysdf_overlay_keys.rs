use config::saysdf_keys::{display, is, Key, ParseError, Pressed};
use termwiz::input::{InputEvent, KeyCode, KeyEvent, Modifiers};

pub fn pressed(event: &InputEvent, phys: Option<Key>) -> Option<Pressed> {
    match (event, phys) {
        (InputEvent::Key(_), Some(key)) => Some(Pressed::Phys(key)),
        (
            InputEvent::Key(KeyEvent {
                key: KeyCode::Char(c),
                modifiers,
            }),
            None,
        ) => Some(Pressed::Letter(*c, *modifiers)),
        _ => None,
    }
}

fn matching_command(pressed: &Pressed, commands: &[&str]) -> Result<Option<char>, ParseError> {
    for command in commands.iter().flat_map(|c| c.chars()) {
        if is(pressed, command.encode_utf8(&mut [0; 4]))? {
            return Ok(Some(command));
        }
    }
    Ok(None)
}

pub fn command_event(
    event: InputEvent,
    phys: Option<Key>,
    commands: &[&str],
) -> Result<InputEvent, ParseError> {
    let modifiers = match &event {
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char(_),
            modifiers,
        }) => *modifiers,
        _ => return Ok(event),
    };
    let command = match pressed(&event, phys) {
        Some(pressed) => matching_command(&pressed, commands)?,
        None => None,
    };
    Ok(match command {
        Some(command) => InputEvent::Key(KeyEvent {
            key: KeyCode::Char(command),
            modifiers,
        }),
        None => event,
    })
}

pub fn confirm_answer(event: &InputEvent, phys: Option<Key>) -> Option<bool> {
    pressed(event, phys).and_then(|p| config::saysdf_keys::confirm_answer(&p))
}

pub fn label_key(key: KeyCode, phys: Option<Key>) -> KeyCode {
    let (KeyCode::Char(_), Some(phys)) = (&key, phys) else {
        return key;
    };
    let name = display(&Key::new(phys.code, Modifiers::NONE));
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() && phys.mods.contains(Modifiers::SHIFT) => {
            KeyCode::Char(c.to_ascii_uppercase())
        }
        (Some(c), None) if c.is_ascii_alphabetic() => KeyCode::Char(c.to_ascii_lowercase()),
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use window::PhysKeyCode;

    fn key_event(c: char, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char(c),
            modifiers,
        })
    }

    fn phys(code: PhysKeyCode, mods: Modifiers) -> Option<Key> {
        Some(Key::new(code, mods))
    }

    #[test]
    fn command_event_uses_physical_key() {
        let event = command_event(
            key_event('о', Modifiers::NONE), // i18n-allow-cyrillic
            phys(PhysKeyCode::J, Modifiers::NONE),
            &["asdf", "jk/"],
        )
        .unwrap();
        assert_eq!(event, key_event('j', Modifiers::NONE));
        let event = command_event(
            key_event('ф', Modifiers::NONE), // i18n-allow-cyrillic
            phys(PhysKeyCode::A, Modifiers::NONE),
            &["asdf", "jk/"],
        )
        .unwrap();
        assert_eq!(event, key_event('a', Modifiers::NONE));
    }

    #[test]
    fn command_event_keeps_unmatched_and_typed_without_phys() {
        let ctrl_p = command_event(
            key_event('P', Modifiers::CTRL),
            phys(PhysKeyCode::P, Modifiers::CTRL),
            &["asdf", "jk/"],
        )
        .unwrap();
        assert_eq!(ctrl_p, key_event('P', Modifiers::CTRL));
        let typed = command_event(key_event('о', Modifiers::NONE), None, &["jk/"]).unwrap(); // i18n-allow-cyrillic
        assert_eq!(typed, key_event('о', Modifiers::NONE)); // i18n-allow-cyrillic
        let typed = command_event(key_event('k', Modifiers::NONE), None, &["jk/"]).unwrap();
        assert_eq!(typed, key_event('k', Modifiers::NONE));
    }

    #[test]
    fn command_event_cyrillic_alphabet_matches_typed_letter() {
        let event = command_event(key_event('ф', Modifiers::NONE), None, &["фыва"]).unwrap(); // i18n-allow-cyrillic
        assert_eq!(event, key_event('ф', Modifiers::NONE)); // i18n-allow-cyrillic
    }

    #[test]
    fn confirm_answer_is_physical_y_without_modifiers() {
        let answer = |c, p| confirm_answer(&key_event(c, Modifiers::NONE), p);
        assert_eq!(
            answer('н', phys(PhysKeyCode::Y, Modifiers::NONE)), // i18n-allow-cyrillic
            Some(true)
        );
        assert_eq!(
            answer('т', phys(PhysKeyCode::N, Modifiers::NONE)), // i18n-allow-cyrillic
            Some(false)
        );
        assert_eq!(answer('д', phys(PhysKeyCode::L, Modifiers::NONE)), None); // i18n-allow-cyrillic
        assert_eq!(answer('y', phys(PhysKeyCode::T, Modifiers::NONE)), None);
        assert_eq!(answer('Y', phys(PhysKeyCode::Y, Modifiers::SHIFT)), None);
        assert_eq!(answer('y', phys(PhysKeyCode::Y, Modifiers::CTRL)), None);
        assert_eq!(answer('y', None), Some(true));
        assert_eq!(answer('Y', None), Some(true));
        assert_eq!(answer('н', None), None); // i18n-allow-cyrillic
        assert_eq!(answer('д', None), None); // i18n-allow-cyrillic
    }

    #[test]
    fn label_key_from_physical_position() {
        let ru = KeyCode::Char('ф'); // i18n-allow-cyrillic
        assert_eq!(
            label_key(ru, phys(PhysKeyCode::A, Modifiers::NONE)),
            KeyCode::Char('a')
        );
        assert_eq!(
            label_key(KeyCode::Char('Ф'), phys(PhysKeyCode::A, Modifiers::SHIFT)), // i18n-allow-cyrillic
            KeyCode::Char('A')
        );
        assert_eq!(label_key(ru, None), ru);
        assert_eq!(
            label_key(KeyCode::Char('!'), phys(PhysKeyCode::K1, Modifiers::SHIFT)),
            KeyCode::Char('!')
        );
        assert_eq!(
            label_key(
                KeyCode::Backspace,
                phys(PhysKeyCode::Backspace, Modifiers::NONE)
            ),
            KeyCode::Backspace
        );
    }
}
