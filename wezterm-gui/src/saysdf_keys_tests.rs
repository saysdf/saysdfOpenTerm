use crate::inputmap::InputMap;
use config::KeyMapPreference;
use window::{KeyCode, Modifiers, PhysKeyCode};

fn input_map(preference: KeyMapPreference) -> InputMap {
    InputMap::new(&config::ConfigHandle::saysdf_from_config(config::Config {
        key_map_preference: preference,
        ..config::Config::default_config()
    }))
}

#[test]
fn copy_mode_has_physical_twins() {
    let map = input_map(KeyMapPreference::Physical);
    let copy = &map.keys.by_name["copy_mode"];
    for key in [
        (KeyCode::Physical(PhysKeyCode::H), Modifiers::NONE),
        (KeyCode::Physical(PhysKeyCode::V), Modifiers::SHIFT),
        (KeyCode::Char('h'), Modifiers::NONE),
        (KeyCode::Char('V'), Modifiers::SHIFT),
    ] {
        assert!(copy.contains_key(&key), "{:?}", key);
    }
}

#[test]
fn mapped_copy_mode_has_no_physical_twins() {
    let map = input_map(KeyMapPreference::Mapped);
    let copy = &map.keys.by_name["copy_mode"];
    assert!(!copy.contains_key(&(KeyCode::Physical(PhysKeyCode::H), Modifiers::NONE)));
}
