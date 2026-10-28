use super::KeyboardWithFallback;
use crate::{Handled, KeyCode, RawKeyEvent};
use std::cell::RefCell;
use std::collections::VecDeque;
use xkbcommon::xkb;

pub(crate) struct ImePress {
    window: u32,
    text: String,
    raw: RawKeyEvent,
}

#[derive(Default)]
pub(crate) struct ImePressTracker {
    pending: RefCell<VecDeque<ImePress>>,
}

const MAX_PENDING_PRESSES: usize = 32;

impl ImePressTracker {
    pub(crate) fn remember(&self, press: ImePress) {
        let mut pending = self.pending.borrow_mut();
        if pending.len() == MAX_PENDING_PRESSES {
            pending.pop_front();
        }
        pending.push_back(press);
    }

    pub(crate) fn clear(&self) {
        self.pending.borrow_mut().clear();
    }

    pub(crate) fn take_for_commit(&self, window: u32, committed: &str) -> Option<RawKeyEvent> {
        let mut pending = self.pending.borrow_mut();
        while let Some(press) = pending.pop_front() {
            if press.window == window && !press.text.is_empty() && press.text == committed {
                return Some(press.raw);
            }
        }
        None
    }
}

impl KeyboardWithFallback {
    pub(crate) fn saysdf_ime_press(&self, window: u32, ev: &xcb::x::KeyPressEvent) -> ImePress {
        let xcode = xkb::Keycode::from(ev.detail());
        let state = ev.state();
        let phys_code = self.selected.phys_code_map.borrow().get(&xcode).copied();
        self.merge_current_xcb_modifiers(state.bits());
        let text = self.selected.state.borrow().key_get_utf8(xcode);
        self.reapply_last_xcb_state();
        ImePress {
            window,
            text,
            raw: RawKeyEvent {
                key: match phys_code {
                    Some(phys) => KeyCode::Physical(phys),
                    None => KeyCode::RawCode(xcode.into()),
                },
                phys_code,
                raw_code: xcode.into(),
                modifiers: Self::modifiers_from_btn_mask(state),
                leds: self.get_led_status(),
                repeat_count: 1,
                key_is_down: true,
                handled: Handled::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyboardLedStatus, Modifiers, PhysKeyCode};

    fn press(window: u32, text: &str) -> ImePress {
        press_on(window, text, PhysKeyCode::Y)
    }

    fn press_on(window: u32, text: &str, phys: PhysKeyCode) -> ImePress {
        ImePress {
            window,
            text: text.to_string(),
            raw: RawKeyEvent {
                key: KeyCode::Physical(phys),
                phys_code: Some(phys),
                raw_code: 29,
                modifiers: Modifiers::NONE,
                leds: KeyboardLedStatus::empty(),
                repeat_count: 1,
                key_is_down: true,
                handled: Handled::new(),
            },
        }
    }

    #[test]
    fn single_press_commit_gets_raw_once() {
        let tracker = ImePressTracker::default();
        tracker.remember(press(1, "н")); // i18n-allow-cyrillic
        let raw = tracker.take_for_commit(1, "н").expect("raw attached"); // i18n-allow-cyrillic
        assert_eq!(raw.phys_code, Some(PhysKeyCode::Y));
        assert!(tracker.take_for_commit(1, "н").is_none()); // i18n-allow-cyrillic
    }

    #[test]
    fn composed_commit_does_not_get_last_press() {
        let tracker = ImePressTracker::default();
        tracker.remember(press(1, " "));
        assert!(tracker.take_for_commit(1, "你好").is_none());
        tracker.remember(press(1, "e"));
        assert!(tracker.take_for_commit(1, "é").is_none());
    }

    #[test]
    fn cleared_or_other_window_press_is_not_attached() {
        let tracker = ImePressTracker::default();
        tracker.remember(press(1, "н")); // i18n-allow-cyrillic
        tracker.clear();
        assert!(tracker.take_for_commit(1, "н").is_none()); // i18n-allow-cyrillic
        tracker.remember(press(2, "н")); // i18n-allow-cyrillic
        assert!(tracker.take_for_commit(1, "н").is_none()); // i18n-allow-cyrillic
        tracker.remember(press(1, ""));
        assert!(tracker.take_for_commit(1, "").is_none());
    }

    #[test]
    fn burst_of_presses_before_commits_keeps_each_raw() {
        let tracker = ImePressTracker::default();
        tracker.remember(press_on(1, "а", PhysKeyCode::F)); // i18n-allow-cyrillic
        tracker.remember(press_on(1, "в", PhysKeyCode::D)); // i18n-allow-cyrillic
        tracker.remember(press_on(1, "ы", PhysKeyCode::S)); // i18n-allow-cyrillic
        for (text, phys) in [
            ("а", PhysKeyCode::F), // i18n-allow-cyrillic
            ("в", PhysKeyCode::D), // i18n-allow-cyrillic
            ("ы", PhysKeyCode::S), // i18n-allow-cyrillic
        ] {
            let raw = tracker.take_for_commit(1, text).expect("raw attached");
            assert_eq!(raw.phys_code, Some(phys));
        }
        assert!(tracker.take_for_commit(1, "ы").is_none()); // i18n-allow-cyrillic
    }

    #[test]
    fn unmatched_presses_before_commit_are_dropped() {
        let tracker = ImePressTracker::default();
        tracker.remember(press_on(1, "\u{3}", PhysKeyCode::C));
        tracker.remember(press_on(1, "в", PhysKeyCode::D)); // i18n-allow-cyrillic
        let raw = tracker.take_for_commit(1, "в").expect("raw attached"); // i18n-allow-cyrillic
        assert_eq!(raw.phys_code, Some(PhysKeyCode::D));
        assert!(tracker.pending.borrow().is_empty());
    }

    #[test]
    fn pending_presses_are_bounded() {
        let tracker = ImePressTracker::default();
        for _ in 0..MAX_PENDING_PRESSES + 5 {
            tracker.remember(press(1, "н")); // i18n-allow-cyrillic
        }
        assert_eq!(tracker.pending.borrow().len(), MAX_PENDING_PRESSES);
    }
}
