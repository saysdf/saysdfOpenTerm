use std::fmt;
use wezterm_input_types::{us_layout_unshift, KeyCode, Modifiers, PhysKeyCode, RawKeyEvent};

const MODIFIER_NAMES: [(Modifiers, &str); 4] = [
    (Modifiers::CTRL, "Ctrl"),
    (Modifiers::ALT, "Alt"),
    (Modifiers::SHIFT, "Shift"),
    (Modifiers::SUPER, "Super"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: PhysKeyCode,
    pub mods: Modifiers,
}

impl Key {
    pub fn new(code: PhysKeyCode, mods: Modifiers) -> Self {
        Self {
            code,
            mods: strip(mods),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub spec: String,
    pub reason: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "invalid key spec {:?}: {}", self.spec, self.reason)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Phys(Key),
    Letter(char, Modifiers),
}

fn strip(mods: Modifiers) -> Modifiers {
    mods.remove_positional_mods() - Modifiers::LEADER
}

fn shift(c: char) -> char {
    (' '..='~')
        .find(|&s| s != c && us_layout_unshift(s) == c)
        .unwrap_or(c)
}

pub fn from_binding(key: &KeyCode, mods: Modifiers) -> Option<Key> {
    let (key, mods) = match key {
        KeyCode::Char(c) if c.is_ascii_graphic() && us_layout_unshift(*c) != *c => (
            KeyCode::Char(us_layout_unshift(*c)),
            mods | Modifiers::SHIFT,
        ),
        _ => (key.clone(), mods),
    };
    key.to_phys().map(|code| Key::new(code, mods))
}

fn split_spec(spec: &str) -> (Option<&str>, &str) {
    if let Some(mods) = spec.strip_suffix("++") {
        return (Some(mods), "+");
    }
    match spec.rsplit_once('+') {
        Some((mods, key)) if !key.is_empty() => (Some(mods), key),
        _ => (None, spec),
    }
}

pub fn parse(spec: &str) -> Result<Key, ParseError> {
    let error = |reason: String| ParseError {
        spec: spec.to_string(),
        reason,
    };
    let (mods_part, key_part) = split_spec(spec);
    let mut mods = Modifiers::NONE;
    for name in mods_part.into_iter().flat_map(|m| m.split('+')) {
        let (bit, _) = MODIFIER_NAMES
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .ok_or_else(|| error(format!("unknown modifier {name:?}")))?;
        mods |= *bit;
    }
    let mut chars = key_part.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => from_binding(&KeyCode::Char(c), mods)
            .ok_or_else(|| error(format!("{c:?} is not a key on the US-ANSI layout"))),
        _ => PhysKeyCode::try_from(key_part)
            .map(|code| Key::new(code, mods))
            .map_err(error),
    }
}

fn key_name(code: PhysKeyCode) -> String {
    match code.to_key_code() {
        KeyCode::Char(c) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase().to_string(),
        _ => code.to_string(),
    }
}

pub fn display(key: &Key) -> String {
    MODIFIER_NAMES
        .iter()
        .filter(|(bit, _)| key.mods.contains(*bit))
        .map(|(_, name)| name.to_string())
        .chain(std::iter::once(key_name(key.code)))
        .collect::<Vec<_>>()
        .join("+")
}

pub fn from_raw(raw: &RawKeyEvent) -> Option<Key> {
    raw.phys_code.map(|code| Key::new(code, raw.modifiers))
}

fn kitty_mods(field: Option<&str>) -> Option<Modifiers> {
    let value: u16 = match field.and_then(|f| f.split(':').next()) {
        None | Some("") => return Some(Modifiers::NONE),
        Some(v) => v.parse().ok()?,
    };
    let bits = value.checked_sub(1)?;
    let mut mods = Modifiers::NONE;
    for (bit, flag) in [
        (1, Modifiers::SHIFT),
        (2, Modifiers::ALT),
        (4, Modifiers::CTRL),
        (8, Modifiers::SUPER),
    ] {
        if bits & bit != 0 {
            mods |= flag;
        }
    }
    Some(mods)
}

pub fn from_kitty(params: &str) -> Option<Key> {
    let mut fields = params.split(';');
    let mut codes = fields.next()?.split(':');
    let unicode = codes.next()?;
    let base = codes.nth(1).filter(|b| !b.is_empty()).unwrap_or(unicode);
    let c = char::from_u32(base.parse().ok()?)?;
    if !c.is_ascii() {
        return None;
    }
    let code = match c {
        '\u{7f}' => PhysKeyCode::Backspace,
        c => KeyCode::Char(c.to_ascii_lowercase()).to_phys()?,
    };
    Some(Key::new(code, kitty_mods(fields.next())?))
}

fn us_char(key: &Key) -> Option<char> {
    match key.code.to_key_code() {
        KeyCode::Char(c) if key.mods.contains(Modifiers::SHIFT) => Some(shift(c)),
        KeyCode::Char(c) => Some(c),
        _ => None,
    }
}

pub fn is(pressed: &Pressed, spec: &str) -> Result<bool, ParseError> {
    let expected = match parse(spec) {
        Ok(expected) => expected,
        Err(err) => {
            let mut chars = spec.chars();
            return match (chars.next(), chars.next()) {
                (Some(literal), None) => {
                    Ok(matches!(pressed, Pressed::Letter(c, _) if *c == literal))
                }
                _ => Err(err),
            };
        }
    };
    Ok(match pressed {
        Pressed::Phys(key) => *key == expected,
        Pressed::Letter(c, mods) => {
            us_char(&expected) == Some(*c)
                && strip(*mods) - Modifiers::SHIFT == expected.mods - Modifiers::SHIFT
        }
    })
}

pub const CONFIRM_HINT: &str = "Press Y to confirm, N or Esc to cancel";

pub fn confirm_answer(pressed: &Pressed) -> Option<bool> {
    match pressed {
        Pressed::Phys(key) if key.mods == Modifiers::NONE => match key.code {
            PhysKeyCode::Y => Some(true),
            PhysKeyCode::N | PhysKeyCode::Escape => Some(false),
            _ => None,
        },
        Pressed::Letter(c, mods) if strip(*mods) - Modifiers::SHIFT == Modifiers::NONE => match c {
            'y' | 'Y' => Some(true),
            'n' | 'N' => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pressed: &Pressed, spec: &str) -> bool {
        is(pressed, spec).unwrap()
    }

    fn key(code: PhysKeyCode, mods: Modifiers) -> Key {
        Key { code, mods }
    }

    #[test]
    fn uppercase_letter_binding_adds_shift() {
        assert_eq!(
            from_binding(&KeyCode::Char('P'), Modifiers::CTRL),
            Some(key(PhysKeyCode::P, Modifiers::CTRL | Modifiers::SHIFT))
        );
    }

    #[test]
    fn shifted_punctuation_is_base_key_with_shift() {
        assert_eq!(parse("?"), Ok(key(PhysKeyCode::Slash, Modifiers::SHIFT)));
        assert_eq!(
            from_binding(&KeyCode::Char('?'), Modifiers::CTRL),
            Some(key(PhysKeyCode::Slash, Modifiers::CTRL | Modifiers::SHIFT))
        );
    }

    #[test]
    fn lowercase_letter_is_physical_key() {
        assert_eq!(parse("y"), Ok(key(PhysKeyCode::Y, Modifiers::NONE)));
    }

    #[test]
    fn parse_modifiers_and_names() {
        assert_eq!(
            parse("Ctrl+Shift+E"),
            Ok(key(PhysKeyCode::E, Modifiers::CTRL | Modifiers::SHIFT))
        );
        assert_eq!(
            parse("ctrl+Slash"),
            Ok(key(PhysKeyCode::Slash, Modifiers::CTRL))
        );
        assert_eq!(
            parse("Ctrl++"),
            Ok(key(PhysKeyCode::Equal, Modifiers::CTRL | Modifiers::SHIFT))
        );
    }

    #[test]
    fn parse_errors_name_the_spec() {
        let err = parse("Hyper+y").unwrap_err();
        assert_eq!(err.spec, "Hyper+y");
        assert!(err.to_string().contains("Hyper"));
        assert!(parse("й").is_err());
        assert!(parse("NoSuchKey").is_err());
    }

    #[test]
    fn display_names_physical_position() {
        assert_eq!(
            display(&key(PhysKeyCode::E, Modifiers::CTRL | Modifiers::SHIFT)),
            "Ctrl+Shift+E"
        );
        assert_eq!(display(&key(PhysKeyCode::Q, Modifiers::NONE)), "Q");
        assert_eq!(display(&key(PhysKeyCode::K1, Modifiers::ALT)), "Alt+1");
        assert_eq!(display(&key(PhysKeyCode::Slash, Modifiers::NONE)), "Slash");
    }

    #[test]
    fn display_round_trips_through_parse() {
        let k = key(PhysKeyCode::Slash, Modifiers::CTRL | Modifiers::SHIFT);
        assert_eq!(parse(&display(&k)), Ok(k));
    }

    #[test]
    fn kitty_base_layout_key() {
        assert_eq!(
            from_kitty("1085::121"),
            Some(key(PhysKeyCode::Y, Modifiers::NONE))
        );
        assert_eq!(
            from_kitty("1085:1053:121;2"),
            Some(key(PhysKeyCode::Y, Modifiers::SHIFT))
        );
        assert_eq!(
            from_kitty("121;5"),
            Some(key(PhysKeyCode::Y, Modifiers::CTRL))
        );
        assert_eq!(
            from_kitty("113;1:3"),
            Some(key(PhysKeyCode::Q, Modifiers::NONE))
        );
        assert_eq!(from_kitty("1085"), None);
        assert_eq!(from_kitty("57399"), None);
        assert_eq!(from_kitty("abc"), None);
    }

    #[test]
    fn is_matches_physical_or_us_letter() {
        assert!(matches(&Pressed::Letter('y', Modifiers::NONE), "y"));
        assert!(!matches(&Pressed::Letter('н', Modifiers::NONE), "y"));
        assert!(matches(&Pressed::Letter('G', Modifiers::SHIFT), "Shift+g"));
        assert!(!matches(&Pressed::Letter('y', Modifiers::CTRL), "y"));
        assert!(matches(
            &Pressed::Phys(key(PhysKeyCode::Y, Modifiers::NONE)),
            "y"
        ));
        assert!(!matches(
            &Pressed::Phys(key(PhysKeyCode::Y, Modifiers::SHIFT)),
            "y"
        ));
        assert!(!matches(
            &Pressed::Phys(key(PhysKeyCode::L, Modifiers::NONE)),
            "y"
        ));
    }

    #[test]
    fn unparseable_single_char_spec_matches_literally() {
        assert!(matches(&Pressed::Letter('й', Modifiers::NONE), "й"));
        assert!(!matches(&Pressed::Letter('q', Modifiers::NONE), "й"));
        assert!(!matches(
            &Pressed::Phys(key(PhysKeyCode::Q, Modifiers::NONE)),
            "й"
        ));
    }

    #[test]
    fn unparseable_multi_char_spec_is_an_error() {
        let err = is(&Pressed::Letter('y', Modifiers::NONE), "NoSuchKey").unwrap_err();
        assert_eq!(err.spec, "NoSuchKey");
    }

    #[test]
    fn leader_and_positional_bits_are_stripped() {
        let mods =
            Modifiers::CTRL | Modifiers::LEFT_CTRL | Modifiers::LEADER | Modifiers::ENHANCED_KEY;
        assert_eq!(
            from_binding(&KeyCode::Char('e'), mods),
            Some(key(PhysKeyCode::E, Modifiers::CTRL))
        );
        let raw = RawKeyEvent {
            key: KeyCode::Char('у'),
            modifiers: mods | Modifiers::RIGHT_SHIFT | Modifiers::SHIFT,
            leds: Default::default(),
            phys_code: Some(PhysKeyCode::E),
            raw_code: 0,
            #[cfg(windows)]
            scan_code: 0,
            repeat_count: 1,
            key_is_down: true,
            handled: wezterm_input_types::Handled::new(),
        };
        assert_eq!(
            from_raw(&raw),
            Some(key(PhysKeyCode::E, Modifiers::CTRL | Modifiers::SHIFT))
        );
        assert!(matches(&Pressed::Letter('y', Modifiers::LEFT_ALT), "y"));
    }

    #[test]
    fn confirm_is_physical_y_n_esc_without_modifiers() {
        let phys = |code, mods| confirm_answer(&Pressed::Phys(key(code, mods)));
        assert_eq!(phys(PhysKeyCode::Y, Modifiers::NONE), Some(true));
        assert_eq!(phys(PhysKeyCode::N, Modifiers::NONE), Some(false));
        assert_eq!(phys(PhysKeyCode::Escape, Modifiers::NONE), Some(false));
        assert_eq!(phys(PhysKeyCode::Y, Modifiers::SHIFT), None);
        assert_eq!(phys(PhysKeyCode::Y, Modifiers::CTRL), None);
        assert_eq!(phys(PhysKeyCode::L, Modifiers::NONE), None);
    }

    #[test]
    fn confirm_without_phys_is_typed_us_letter_any_case() {
        let typed = |c, mods| confirm_answer(&Pressed::Letter(c, mods));
        assert_eq!(typed('y', Modifiers::NONE), Some(true));
        assert_eq!(typed('Y', Modifiers::NONE), Some(true));
        assert_eq!(typed('Y', Modifiers::SHIFT), Some(true));
        assert_eq!(typed('N', Modifiers::SHIFT), Some(false));
        assert_eq!(typed('y', Modifiers::CTRL), None);
        assert_eq!(typed('н', Modifiers::NONE), None);
        assert_eq!(typed('д', Modifiers::NONE), None);
    }
}
