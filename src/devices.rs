//! Keyboard detection and key reading.
//!
//! One backend per operating system, all behind the same small interface:
//!
//! * Linux: evdev (`/dev/input/event*`), with an exclusive grab per keyboard;
//! * Windows: Raw Input tells the keyboards apart, a keyboard hook keeps an exclusive one from typing;
//! * macOS: IOHID, which can also seize a keyboard exclusively.
//!
//! Whatever the system, a key is identified by its *position* using the Linux/evdev numbers
//! (`layout::KEYS`): the backends translate their own scan codes into those.

use std::fmt;

#[cfg(any(windows, target_os = "macos", test))]
use crate::layout::KEYS;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

// Pieces of logic that do not touch the system: they are tested on every platform.
#[cfg(any(windows, test))]
mod correlate;
#[cfg(any(target_os = "macos", test))]
mod hid_usage;
#[cfg(any(windows, test))]
mod winpath;

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
compile_error!("Keynortia works on Linux, Windows and macOS.");

pub use platform::{Reader, Scanner, spawn_reader};

/// Identifies a keyboard for as long as it stays plugged in (the system's own name for the device:
/// a file such as `/dev/input/event5` on Linux, a device path on Windows, an IOKit id on macOS).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(id: impl Into<String>) -> Self {
        DeviceId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How the keyboard is connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    Usb,
    Bluetooth,
    /// The laptop's own keyboard (PS/2, I2C, SPI...).
    Internal,
    /// Created by a program (for example a key remapper, or a remote desktop session).
    Virtual,
    Other,
}

impl Connection {
    pub fn label(self) -> &'static str {
        match self {
            Connection::Usb => "USB",
            Connection::Bluetooth => "Bluetooth",
            Connection::Internal => "Internal",
            Connection::Virtual => "Virtual",
            Connection::Other => "Other",
        }
    }
}

/// What the operating system can tell about a keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardInfo {
    pub id: DeviceId,
    pub name: String,
    pub connection: Connection,
    /// How the system sees the keyboard (file, bus, location...), for the tooltip: it helps
    /// to understand errors.
    pub technical: String,
    pub vendor: u16,
    pub product: u16,
    /// Keys reported by the device (not necessarily all of them physically present), if the system says.
    pub reported_keys: Option<usize>,
    /// Reports the numeric keypad.
    pub has_keypad: bool,
    /// Codes of the playable keys (among the 45 of `layout::KEYS`) that the keyboard reports.
    pub playable: Vec<u16>,
}

impl KeyboardInfo {
    pub fn can_play(&self, code: u16) -> bool {
        self.playable.contains(&code)
    }

    /// Short description of the size, worked out from the reported keys.
    pub fn size_label(&self) -> &'static str {
        match (self.reported_keys, self.has_keypad) {
            (_, true) => "with numeric keypad",
            // the system does not count the keys: the size cannot be told
            (None, false) => "without numeric keypad",
            (Some(n), false) if n >= 80 => "without numeric keypad",
            _ => "compact",
        }
    }
}

/// The codes of all 45 playable keys: for systems that cannot say which keys a keyboard has.
#[cfg(any(windows, target_os = "macos", test))]
pub fn all_playable() -> Vec<u16> {
    KEYS.iter().map(|k| k.code).collect()
}

/// A key pressed or released (evdev code).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Press(u16),
    Release(u16),
}

/// Error message of the reader when the keyboard is unplugged.
pub const UNPLUGGED: &str = "keyboard unplugged";

/// Why a keyboard could not be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenErrorKind {
    /// Another program holds the keyboard exclusively (on Linux: keyd, kmonad...).
    Busy,
    Other,
}

/// A keyboard could not be opened. The message is already worded for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenError {
    pub kind: OpenErrorKind,
    pub message: String,
}

impl OpenError {
    #[cfg(any(windows, target_os = "macos"))]
    pub fn other(message: impl Into<String>) -> Self {
        OpenError {
            kind: OpenErrorKind::Other,
            message: message.into(),
        }
    }
}

/// Something that stops every keyboard from being read, and what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub headline: &'static str,
    pub before: &'static str,
    /// A command to type, shown in monospace between `before` and `after`.
    pub command: Option<&'static str>,
    pub after: &'static str,
}

/// True if `code` is one of the 45 playable keys.
#[cfg(any(windows, test))]
pub fn is_playable_code(code: u16) -> bool {
    KEYS.iter().any(|k| k.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(reported_keys: Option<usize>, has_keypad: bool) -> KeyboardInfo {
        KeyboardInfo {
            id: DeviceId::new("x"),
            name: "x".into(),
            connection: Connection::Usb,
            technical: String::new(),
            vendor: 1,
            product: 2,
            reported_keys,
            has_keypad,
            playable: all_playable(),
        }
    }

    #[test]
    fn all_45_keys_are_playable_and_known() {
        let codes = all_playable();
        assert_eq!(codes.len(), 45);
        assert!(codes.iter().all(|&c| is_playable_code(c)));
        assert!(!is_playable_code(57), "the space bar");
        assert!(info(None, false).can_play(44), "the Z key");
    }

    #[test]
    fn size_label_goes_by_keys_and_keypad() {
        assert_eq!(info(Some(163), true).size_label(), "with numeric keypad");
        assert_eq!(info(Some(90), false).size_label(), "without numeric keypad");
        assert_eq!(info(Some(60), false).size_label(), "compact");
        assert_eq!(info(None, false).size_label(), "without numeric keypad");
    }

    #[test]
    fn device_ids_compare_by_name() {
        assert_eq!(DeviceId::new("a"), DeviceId::new(String::from("a")));
        assert_eq!(
            DeviceId::new("/dev/input/event4").to_string(),
            "/dev/input/event4"
        );
    }
}
