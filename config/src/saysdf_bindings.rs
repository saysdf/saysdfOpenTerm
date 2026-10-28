use crate::keyassignment::KeyTable;
use crate::keys::{KeyMapPreference, KeyNoAction};
use crate::{Config, ConfigHandle};
use std::sync::Arc;
use wezterm_input_types::{KeyCode, Modifiers};

pub use saysdf_keys;

fn physical_binding(key: &KeyCode, mods: Modifiers) -> Option<(KeyCode, Modifiers)> {
    saysdf_keys::from_binding(key, mods).map(|k| {
        (
            KeyCode::Physical(k.code),
            mods | (k.mods & Modifiers::SHIFT),
        )
    })
}

impl KeyNoAction {
    pub fn saysdf_resolve(
        &self,
        preference: KeyMapPreference,
        resolved: (KeyCode, Modifiers),
    ) -> (KeyCode, Modifiers) {
        if preference != KeyMapPreference::Physical {
            return resolved;
        }
        physical_binding(&self.key.resolve(KeyMapPreference::Mapped), self.mods).unwrap_or(resolved)
    }
}

pub fn saysdf_physical_bindings(
    preference: KeyMapPreference,
    keys: Vec<(Modifiers, KeyCode)>,
) -> Vec<(Modifiers, KeyCode)> {
    if preference != KeyMapPreference::Physical {
        return keys;
    }
    let mut physical = Vec::with_capacity(keys.len());
    for (mods, key) in keys {
        let (key, mods) = physical_binding(&key, mods).unwrap_or((key, mods));
        if !physical.contains(&(mods, key.clone())) {
            physical.push((mods, key));
        }
    }
    physical
}

pub fn saysdf_physical_twins(preference: KeyMapPreference, mut table: KeyTable) -> KeyTable {
    if preference != KeyMapPreference::Physical {
        return table;
    }
    let twins: Vec<_> = table
        .iter()
        .filter_map(|((key, mods), entry)| {
            physical_binding(key, *mods).map(|twin| (twin, entry.clone()))
        })
        .collect();
    for (twin, entry) in twins {
        table.entry(twin).or_insert(entry);
    }
    table
}

impl ConfigHandle {
    pub fn saysdf_from_config(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            generation: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyassignment::{KeyAssignment, KeyTableEntry};
    use crate::keys::{DeferredKeyCode, Key};
    use std::convert::TryFrom;
    use wezterm_input_types::PhysKeyCode;

    fn bindings(
        preference: KeyMapPreference,
        key: &str,
        mods: Modifiers,
    ) -> Vec<(KeyCode, Modifiers)> {
        let config = Config {
            key_map_preference: preference,
            keys: vec![Key {
                key: KeyNoAction {
                    key: DeferredKeyCode::try_from(key).unwrap(),
                    mods,
                },
                action: KeyAssignment::Nop,
            }],
            ..Config::default()
        };
        let tables = config.key_bindings();
        tables.default.into_keys().collect()
    }

    #[test]
    fn uppercase_without_shift_keeps_shift() {
        assert_eq!(
            bindings(KeyMapPreference::Physical, "P", Modifiers::CTRL),
            vec![(
                KeyCode::Physical(PhysKeyCode::P),
                Modifiers::CTRL | Modifiers::SHIFT
            )]
        );
    }

    #[test]
    fn shifted_punctuation_is_base_key_with_shift() {
        assert_eq!(
            bindings(KeyMapPreference::Physical, "?", Modifiers::NONE),
            vec![(KeyCode::Physical(PhysKeyCode::Slash), Modifiers::SHIFT)]
        );
    }

    #[test]
    fn leader_mod_is_kept() {
        assert_eq!(
            bindings(KeyMapPreference::Physical, "y", Modifiers::LEADER),
            vec![(KeyCode::Physical(PhysKeyCode::Y), Modifiers::LEADER)]
        );
    }

    #[test]
    fn mapped_is_unchanged() {
        assert_eq!(
            bindings(KeyMapPreference::Mapped, "P", Modifiers::CTRL),
            vec![(KeyCode::Char('P'), Modifiers::CTRL)]
        );
        assert_eq!(
            bindings(KeyMapPreference::Mapped, "?", Modifiers::NONE),
            vec![(KeyCode::Char('?'), Modifiers::NONE)]
        );
    }

    #[test]
    fn twins_do_not_overwrite_existing_entries() {
        let mut table = KeyTable::default();
        table.insert(
            (KeyCode::Char('h'), Modifiers::NONE),
            KeyTableEntry {
                action: KeyAssignment::Nop,
            },
        );
        table.insert(
            (KeyCode::Physical(PhysKeyCode::H), Modifiers::NONE),
            KeyTableEntry {
                action: KeyAssignment::DisableDefaultAssignment,
            },
        );
        let table = saysdf_physical_twins(KeyMapPreference::Physical, table);
        assert_eq!(table.len(), 2);
        assert_eq!(
            table[&(KeyCode::Physical(PhysKeyCode::H), Modifiers::NONE)].action,
            KeyAssignment::DisableDefaultAssignment
        );
    }
}
