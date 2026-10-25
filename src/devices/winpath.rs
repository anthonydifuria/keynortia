//! What can be read from the path Windows gives to a keyboard, for example
//! `\\?\HID#VID_1A2C&PID_9B09&MI_00#7&1ae4b5c&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}`:
//! who made it, how it is connected and a name to show when the device does not give one.

#![cfg_attr(test, allow(dead_code))]

use super::Connection;

/// Bluetooth keyboards (classic and low energy) have the Bluetooth HID service in their path.
const BLUETOOTH_HID: [&str; 2] = [
    "00001124-0000-1000-8000-00805f9b34fb",
    "00001812-0000-1000-8000-00805f9b34fb",
];

/// The hexadecimal digits that follow `tag` (`VID_`, `VID&`, `PID_`...), as a number made of the last four.
fn id_after(path: &str, tag: &str) -> Option<u16> {
    let at = path.find(tag)? + tag.len();
    let digits: String = path[at..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    // USB: `VID_1A2C`. Bluetooth: `VID&0002046d`, where the first four digits say who assigned the id.
    let tail = digits.get(digits.len().checked_sub(4)?..)?;
    u16::from_str_radix(tail, 16).ok()
}

/// Vendor and product id read from the path (0 where it does not say).
pub fn ids(path: &str) -> (u16, u16) {
    let path = path.to_ascii_uppercase();
    let vendor = id_after(&path, "VID_").or_else(|| id_after(&path, "VID&"));
    let product = id_after(&path, "PID_").or_else(|| id_after(&path, "PID&"));
    (vendor.unwrap_or(0), product.unwrap_or(0))
}

/// How the keyboard is connected, as far as the path and the name say.
pub fn connection(path: &str, name: &str) -> Connection {
    let path = path.to_ascii_lowercase();
    let name = name.to_lowercase();
    if path.contains("rdp_kbd") || path.contains("\\root#") || path.contains("vmbus") {
        // the keyboard of a remote desktop session, of a virtual machine, a driver without hardware
        Connection::Virtual
    } else if BLUETOOTH_HID.iter().any(|guid| path.contains(guid)) || path.contains("bth") {
        Connection::Bluetooth
    } else if path.starts_with("\\\\?\\acpi#") {
        // the PS/2 keyboard of a laptop
        Connection::Internal
    } else if path.contains("vid_") || path.contains("vid&") {
        if name.contains("internal") || name.contains("built-in") {
            Connection::Internal
        } else {
            Connection::Usb
        }
    } else if path.starts_with("\\\\?\\hid#") {
        // an HID keyboard with no USB or Bluetooth ids: it sits on the board of the computer (I2C)
        Connection::Internal
    } else {
        Connection::Other
    }
}

/// A name for a keyboard that did not say its own.
pub fn fallback_name(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.contains("rdp_kbd") {
        "Remote desktop keyboard".to_string()
    } else if lower.starts_with("\\\\?\\acpi#") {
        "PS/2 keyboard".to_string()
    } else {
        match ids(path) {
            (0, 0) => "Keyboard".to_string(),
            (vendor, product) => format!("Keyboard {vendor:04x}:{product:04x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USB: &str =
        r"\\?\HID#VID_1A2C&PID_9B09&MI_00#7&1ae4b5c&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const BLUETOOTH: &str = r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002046d_PID&b35b#a&3a7bea1&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const BLE: &str = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02046d_PID&b35b_REV&0016_cc3ba2f2e1c9#a&ad4bdb4&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const PS2: &str = r"\\?\ACPI#PNP0303#4&1f2d9e7e&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const I2C: &str =
        r"\\?\HID#ELAN0678&Col01#5&2f9d1b2&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
    const REMOTE: &str = r"\\?\Root#RDP_KBD#0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";

    #[test]
    fn ids_of_usb_and_bluetooth_paths() {
        assert_eq!(ids(USB), (0x1a2c, 0x9b09));
        assert_eq!(ids(BLUETOOTH), (0x046d, 0xb35b));
        assert_eq!(ids(BLE), (0x046d, 0xb35b));
        assert_eq!(ids(PS2), (0, 0));
        assert_eq!(ids(""), (0, 0));
    }

    #[test]
    fn broken_ids_do_not_panic() {
        assert_eq!(ids(r"\\?\HID#VID_#PID_12"), (0, 0));
        assert_eq!(ids(r"\\?\HID#VID_é"), (0, 0));
    }

    #[test]
    fn how_each_kind_of_keyboard_is_connected() {
        assert_eq!(connection(USB, "USB Gaming Keyboard"), Connection::Usb);
        assert_eq!(connection(BLUETOOTH, "K380"), Connection::Bluetooth);
        assert_eq!(connection(BLE, "MX Keys"), Connection::Bluetooth);
        assert_eq!(connection(PS2, "PS/2 keyboard"), Connection::Internal);
        assert_eq!(connection(I2C, "HID Keyboard Device"), Connection::Internal);
        assert_eq!(
            connection(REMOTE, "Remote desktop keyboard"),
            Connection::Virtual
        );
        assert_eq!(
            connection(USB, "Apple Internal Keyboard / Trackpad"),
            Connection::Internal
        );
        assert_eq!(connection(r"\\?\SOMETHING", "x"), Connection::Other);
    }

    #[test]
    fn names_for_keyboards_that_do_not_say_theirs() {
        assert_eq!(fallback_name(PS2), "PS/2 keyboard");
        assert_eq!(fallback_name(REMOTE), "Remote desktop keyboard");
        assert_eq!(fallback_name(USB), "Keyboard 1a2c:9b09");
        assert_eq!(fallback_name(I2C), "Keyboard");
    }
}
