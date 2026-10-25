//! The parts of the macOS backend that need no system calls, so they are tested on every platform:
//!
//! * the keys of the USB HID standard (usage page 0x07) and the evdev codes they correspond to;
//! * what the usages of a keyboard say about it, how it is connected, how it is identified;
//! * turning the values IOKit reports into presses and releases.
//!
//! HID and evdev both identify a key by its position on a US keyboard, so the translation is
//! simple: the HID usage tells which legend the key has on a US keyboard, and `layout::KEYS` knows
//! the code of the key with that legend. Only the 45 playable keys are translated.

#![cfg_attr(test, allow(dead_code))]

use std::collections::{BTreeSet, HashSet};

use super::{Connection, KeyEvent};
use crate::layout::KEYS;

/// HID usage page of the keyboard keys.
pub const KEYBOARD_PAGE: u32 = 0x07;

/// Legend, on a US keyboard, of the key with HID usage `usage` (only the playable ones).
fn legend(usage: u32) -> Option<char> {
    Some(match usage {
        // usages 0x04..=0x1D are the letters A..Z
        0x04..=0x1D => (b'A' + (usage - 0x04) as u8) as char,
        // then 1..9 and 0
        0x1E..=0x26 => (b'1' + (usage - 0x1E) as u8) as char,
        0x27 => '0',
        0x2D => '-',
        0x2E => '=',
        0x2F => '[',
        0x30 => ']',
        0x33 => ';',
        0x34 => '\'',
        0x36 => ',',
        0x37 => '.',
        0x38 => '/',
        _ => return None,
    })
}

/// evdev code of the playable key with HID usage `usage` (keyboard page), if it is one of them.
pub fn code_for_usage(usage: u32) -> Option<u16> {
    let legend = legend(usage)?;
    let mut buffer = [0u8; 4];
    let legend: &str = legend.encode_utf8(&mut buffer);
    KEYS.iter().find(|k| k.label == legend).map(|k| k.code)
}

/// True if the usages of a device (keyboard page) include every letter: a real keyboard, not a
/// media-keys interface or a mouse with a few keys.
pub fn has_letters(usages: &BTreeSet<u32>) -> bool {
    (0x04..=0x1D).all(|usage| usages.contains(&usage))
}

/// True if they include the numeric keypad.
pub fn has_keypad(usages: &BTreeSet<u32>) -> bool {
    (0x59..=0x63).all(|usage| usages.contains(&usage))
}

/// Codes of the playable keys among `usages`, in the order of `layout::KEYS`.
pub fn playable_codes(usages: &BTreeSet<u32>) -> Vec<u16> {
    let codes: BTreeSet<u16> = usages.iter().filter_map(|&u| code_for_usage(u)).collect();
    KEYS.iter()
        .map(|k| k.code)
        .filter(|code| codes.contains(code))
        .collect()
}

/// How a keyboard is connected, from what IOKit says about it: the transport ("USB", "Bluetooth",
/// "SPI"...), whether it is built in, and its name.
pub fn connection(transport: &str, built_in: bool, name: &str) -> Connection {
    let transport = transport.to_lowercase();
    if transport.contains("virtual") {
        Connection::Virtual
    } else if built_in
        || name.to_lowercase().contains("internal keyboard")
        || matches!(transport.as_str(), "spi" | "i2c")
    {
        Connection::Internal
    } else if transport.contains("bluetooth") {
        Connection::Bluetooth
    } else if transport.contains("usb") {
        Connection::Usb
    } else {
        Connection::Other
    }
}

/// A keyboard is identified by the number of its entry in the IORegistry, in hexadecimal.
pub fn id_of_entry(entry: u64) -> String {
    format!("{entry:#x}")
}

/// The IORegistry entry that an identifier made by `id_of_entry` stands for.
pub fn entry_of_id(id: &str) -> Option<u64> {
    u64::from_str_radix(id.strip_prefix("0x")?, 16).ok()
}

/// Turns the values IOKit reports (one per key: 1 = down, 0 = up) into presses and releases.
#[derive(Default)]
pub struct KeyTracker {
    held: HashSet<u16>,
}

impl KeyTracker {
    /// `down`: the value is not 0. Anything that is not a playable key, and any press or release
    /// that changes nothing, gives `None`.
    pub fn update(&mut self, page: u32, usage: u32, down: bool) -> Option<KeyEvent> {
        if page != KEYBOARD_PAGE {
            return None;
        }
        let code = code_for_usage(usage)?;
        if down {
            self.held.insert(code).then_some(KeyEvent::Press(code))
        } else {
            self.held.remove(&code).then_some(KeyEvent::Release(code))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(usages: impl IntoIterator<Item = u32>) -> BTreeSet<u32> {
        usages.into_iter().collect()
    }

    #[test]
    fn every_playable_key_has_exactly_one_usage() {
        let mut seen = BTreeSet::new();
        for usage in 0..=0xFF {
            if let Some(code) = code_for_usage(usage) {
                assert!(seen.insert(code), "usage {usage:#x} repeats code {code}");
            }
        }
        let all: BTreeSet<u16> = KEYS.iter().map(|k| k.code).collect();
        assert_eq!(seen, all);
    }

    #[test]
    fn known_keys() {
        // usages from the HID usage tables, codes from linux/input-event-codes.h
        assert_eq!(code_for_usage(0x04), Some(30), "A");
        assert_eq!(code_for_usage(0x14), Some(16), "Q");
        assert_eq!(code_for_usage(0x1D), Some(44), "Z");
        assert_eq!(code_for_usage(0x1E), Some(2), "1");
        assert_eq!(code_for_usage(0x27), Some(11), "0");
        assert_eq!(code_for_usage(0x2D), Some(12), "-");
        assert_eq!(code_for_usage(0x30), Some(27), "]");
        assert_eq!(code_for_usage(0x34), Some(40), "'");
        assert_eq!(code_for_usage(0x38), Some(53), "/");
    }

    #[test]
    fn keys_that_do_not_play_are_left_out() {
        for usage in [
            0x00, 0x28, // Enter
            0x29, // Escape
            0x2A, // Backspace
            0x2B, // Tab
            0x2C, // space bar
            0x31, // backslash
            0x32, // the extra key of European keyboards
            0x35, // grave accent
            0x39, // Caps Lock
            0xE0, // left Control
            0xE1, // left Shift
        ] {
            assert_eq!(code_for_usage(usage), None, "usage {usage:#x}");
        }
    }

    #[test]
    fn a_keyboard_has_all_the_letters() {
        assert!(has_letters(&set(0x00..=0x65)));
        assert!(has_letters(&set(0x04..=0x1D)));
        assert!(!has_letters(&set(0x04..=0x1C)), "no Z");
        // a media-keys interface or a mouse with a couple of keys
        assert!(!has_letters(&set([0x04, 0x05])));
        assert!(!has_letters(&set([])));
    }

    #[test]
    fn the_keypad_is_the_usages_from_keypad_1_to_the_dot() {
        assert!(has_keypad(&set(0x00..=0x65)));
        assert!(!has_keypad(&set(0x00..=0x52)));
        assert!(!has_keypad(&set(0x00..=0x58)));
    }

    #[test]
    fn playable_codes_follow_the_order_of_the_layout() {
        let all = playable_codes(&set(0x00..=0xFF));
        let expected: Vec<u16> = KEYS.iter().map(|k| k.code).collect();
        assert_eq!(all, expected);

        // only A and 1: the code of each, in the order of the layout (the digits come first)
        assert_eq!(playable_codes(&set([0x04, 0x1E])), vec![2, 30]);
        assert!(playable_codes(&set([0x28, 0x29])).is_empty());
    }

    #[test]
    fn connection_from_the_words_of_iokit() {
        assert_eq!(connection("USB", false, "USB Keyboard"), Connection::Usb);
        assert_eq!(
            connection("Bluetooth", false, "Keys"),
            Connection::Bluetooth
        );
        assert_eq!(
            connection("BluetoothLowEnergy", false, "Keys"),
            Connection::Bluetooth
        );
        // the laptop's own keyboard: built in, whatever it is connected through
        assert_eq!(
            connection("SPI", true, "Apple Internal Keyboard"),
            Connection::Internal
        );
        assert_eq!(connection("USB", true, "Keys"), Connection::Internal);
        assert_eq!(
            connection("USB", false, "Apple Internal Keyboard / Trackpad"),
            Connection::Internal
        );
        assert_eq!(connection("SPI", false, "x"), Connection::Internal);
        // made by a program (Karabiner-Elements and the like)
        assert_eq!(
            connection("Virtual", false, "Virtual Keyboard"),
            Connection::Virtual
        );
        assert_eq!(connection("", false, "Keys"), Connection::Other);
    }

    #[test]
    fn identifiers_go_back_and_forth() {
        assert_eq!(id_of_entry(0x1_0000_0a2b), "0x100000a2b");
        assert_eq!(entry_of_id("0x100000a2b"), Some(0x1_0000_0a2b));
        assert_eq!(entry_of_id(&id_of_entry(u64::MAX)), Some(u64::MAX));
        assert_eq!(entry_of_id("100000a2b"), None);
        assert_eq!(entry_of_id("0xzz"), None);
        assert_eq!(entry_of_id(""), None);
    }

    #[test]
    fn presses_and_releases() {
        let mut keys = KeyTracker::default();
        // the Z key (usage 0x1D) goes down, then up
        assert_eq!(keys.update(7, 0x1D, true), Some(KeyEvent::Press(44)));
        assert_eq!(keys.update(7, 0x1D, false), Some(KeyEvent::Release(44)));
        // twice in a row counts once
        assert_eq!(keys.update(7, 0x04, true), Some(KeyEvent::Press(30)));
        assert_eq!(keys.update(7, 0x04, true), None);
        assert_eq!(keys.update(7, 0x04, false), Some(KeyEvent::Release(30)));
        assert_eq!(keys.update(7, 0x04, false), None);
    }

    #[test]
    fn what_is_not_a_playable_key_is_ignored() {
        let mut keys = KeyTracker::default();
        assert_eq!(keys.update(7, 0x2C, true), None, "the space bar");
        assert_eq!(
            keys.update(7, 0xFFFF_FFFF, true),
            None,
            "the 'array' element"
        );
        // the same usage on another page is another key (the consumer page, an Apple page...)
        assert_eq!(keys.update(0x0C, 0x1D, true), None);
        assert_eq!(keys.update(0xFF, 0x04, true), None);
    }
}
