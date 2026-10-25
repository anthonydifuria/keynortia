//! Keyboards on Linux, through evdev.
//!
//! Every connected keyboard (USB, Bluetooth, the laptop's own) shows up as a `/dev/input/eventN`
//! file: from it we read the name, how it is connected, the keys it reports and the key press
//! events, keyboard by keyboard. Opening those files requires membership of the `input` group.

use std::collections::HashMap;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use evdev::{Device, EventType, KeyCode};

use super::{
    Connection, DeviceId, KeyEvent, KeyboardInfo, OpenError, OpenErrorKind, Problem, UNPLUGGED,
};
use crate::layout::KEYS;

/// From the kernel bus type (`BUS_*` in linux/input.h) to the kind of connection.
pub fn classify_bus(bus: u16) -> Connection {
    match bus {
        0x03 => Connection::Usb,
        0x05 => Connection::Bluetooth,
        0x06 => Connection::Virtual,
        // ISA, i8042 (PS/2: the keyboard of most laptops), XT, I2C, host, SPI
        0x10 | 0x11 | 0x12 | 0x18 | 0x19 | 0x1c => Connection::Internal,
        _ => Connection::Other,
    }
}

/// How the system physically connects the device. The bus type alone is not enough: a keyboard
/// created by a program (keyd, kmonad...) presents itself as USB, and some internal keyboards sit on USB.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origin {
    /// Kernel bus type (`BUS_*`).
    pub bus: u16,
    /// Physical location declared by the kernel (`phys`), for example `isa0060/serio0/input0`.
    pub phys: String,
    /// Real path under `/sys/devices` (empty if it cannot be read).
    pub sysfs: String,
    /// USB only: `fixed` (a port inside the computer), `removable` or `unknown`.
    pub removable: String,
}

/// Devices created by a program (uinput) live here, whatever bus they declare.
const VIRTUAL_DIR: &str = "/sys/devices/virtual/input/";

/// Internal, USB, Bluetooth or created by a program: uses everything that is known about the device.
pub fn classify(name: &str, origin: &Origin) -> Connection {
    if origin.sysfs.starts_with(VIRTUAL_DIR) {
        return Connection::Virtual;
    }
    match classify_bus(origin.bus) {
        Connection::Usb => {
            let name = name.to_lowercase();
            // a USB port soldered into the computer (the firmware declares it "fixed"),
            // or a keyboard that calls itself internal
            if origin.removable == "fixed" || name.contains("internal") || name.contains("built-in")
            {
                Connection::Internal
            } else {
                Connection::Usb
            }
        }
        other => other,
    }
}

/// Reads from `/sys` where the device `/dev/input/eventN` sits. Anything unreadable stays empty.
fn read_origin(event_file: &Path, bus: u16, phys: &str) -> Origin {
    let sysfs = event_file
        .file_name()
        .and_then(|name| std::fs::canonicalize(Path::new("/sys/class/input").join(name)).ok());
    // the USB device is the first ancestor that has an idVendor: that is where the kernel says
    // whether the port is internal
    let removable = sysfs
        .as_deref()
        .and_then(|p| {
            p.ancestors()
                .find(|a| a.join("idVendor").exists())
                .and_then(|a| std::fs::read_to_string(a.join("removable")).ok())
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    Origin {
        bus,
        phys: phys.to_string(),
        sysfs: sysfs.map(|p| p.display().to_string()).unwrap_or_default(),
        removable,
    }
}

/// Codes of the 26 letters (evdev goes by position: they are the Q..P, A..L, Z..M keys of every keyboard).
fn letter_codes() -> impl Iterator<Item = u16> {
    (16..=25).chain(30..=38).chain(44..=50)
}

/// A real keyboard has all the letters; power buttons, mice and media controls do not.
pub fn is_keyboard(supports: impl Fn(u16) -> bool) -> bool {
    letter_codes().all(supports)
}

/// Technical line: how the system sees the keyboard (file, bus, location, kind of port).
fn technical(path: &Path, origin: &Origin) -> String {
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    let mut line = format!("{file} · bus {:#04x}", origin.bus);
    if !origin.phys.is_empty() {
        line.push_str(&format!(" · {}", origin.phys));
    }
    if origin.sysfs.starts_with(VIRTUAL_DIR) {
        line.push_str(" · created by a program");
    }
    match origin.removable.as_str() {
        "fixed" => line.push_str(" · internal port"),
        "removable" => line.push_str(" · external port"),
        _ => {}
    }
    line
}

fn probe(path: &Path) -> io::Result<Option<KeyboardInfo>> {
    let dev = Device::open(path)?;
    let Some(keys) = dev.supported_keys() else {
        return Ok(None);
    };
    let supports = |code: u16| keys.contains(KeyCode::new(code));
    if !is_keyboard(supports) {
        return Ok(None);
    }
    let id = dev.input_id();
    let name = dev.name().unwrap_or("Unnamed keyboard").to_string();
    let origin = read_origin(path, id.bus_type().0, dev.physical_path().unwrap_or(""));
    Ok(Some(KeyboardInfo {
        id: DeviceId::new(path.display().to_string()),
        connection: classify(&name, &origin),
        technical: technical(path, &origin),
        name,
        vendor: id.vendor(),
        product: id.product(),
        // below 0x100 are the keys; from there up, the buttons (mouse, joystick)
        reported_keys: Some(keys.iter().filter(|k| k.code() < 0x100).count()),
        has_keypad: [KeyCode::KEY_KP0, KeyCode::KEY_KP5, KeyCode::KEY_KPENTER]
            .into_iter()
            .all(|k| keys.contains(k)),
        playable: KEYS
            .iter()
            .map(|k| k.code)
            .filter(|&c| supports(c))
            .collect(),
    }))
}

/// Keeps the list of connected keyboards, updated by rereading `/dev/input`.
#[derive(Default)]
pub struct Scanner {
    /// `None` = device already examined, and it is not a keyboard.
    seen: HashMap<PathBuf, Option<KeyboardInfo>>,
    /// Devices that cannot be opened (almost always: the user is not in the `input` group).
    unreadable: usize,
}

impl Scanner {
    /// Rereads `/dev/input`. Returns true if the list of keyboards changed.
    pub fn refresh(&mut self) -> bool {
        let before = self.keyboards();
        let mut paths: Vec<PathBuf> = std::fs::read_dir("/dev/input")
            .map(|dir| {
                dir.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();
        paths.sort();
        self.seen.retain(|p, _| paths.contains(p));
        self.unreadable = 0;
        for path in paths {
            if self.seen.contains_key(&path) {
                continue;
            }
            match probe(&path) {
                Ok(info) => {
                    self.seen.insert(path, info);
                }
                // not remembered: we try again on the next round (sometimes the permissions arrive later)
                Err(_) => self.unreadable += 1,
            }
        }
        self.keyboards() != before
    }

    /// The connected keyboards, in the order the system attached them (event2, event3, ..., event10).
    pub fn keyboards(&self) -> Vec<KeyboardInfo> {
        let mut list: Vec<KeyboardInfo> = self.seen.values().flatten().cloned().collect();
        list.sort_by_key(|k| event_number(Path::new(k.id.as_str())));
        list
    }

    /// Why no keyboard can be read, if there is a reason.
    pub fn problem(&self) -> Option<Problem> {
        (self.unreadable > 0 && self.keyboards().is_empty()).then_some(Problem {
            headline: "Cannot read the system keyboards.",
            before: "Reading the input devices needs permission. From a terminal:",
            command: Some("sudo usermod -aG input $USER"),
            after: "then log out and back in (or reboot) and reopen Keynortia.",
        })
    }
}

fn event_number(path: &Path) -> u32 {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.trim_start_matches("event").parse().ok())
        .unwrap_or(u32::MAX)
}

/// Reads the keys of a keyboard in a thread, until it is stopped or the keyboard disappears.
pub struct Reader {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Reader {
    /// Stops reading and waits for the thread to end (at most ~0.1 s).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    /// Something that does not work as asked, to show next to "listening". Never on Linux:
    /// if the keyboard cannot be grabbed, opening it fails.
    pub fn warning(&self) -> Option<String> {
        None
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The words for an error on opening a keyboard.
fn open_error(error: &io::Error, vendor: u16, product: u16) -> OpenError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => OpenError {
            kind: OpenErrorKind::Other,
            message: "permission denied: add your user to the input group".to_string(),
        },
        // EBUSY: another program already holds it exclusively
        _ if error.raw_os_error() == Some(libc::EBUSY) => OpenError {
            kind: OpenErrorKind::Busy,
            message: format!(
                "already taken by another program (keyd, kmonad...). With keyd: under [ids] add the line -{vendor:04x}:{product:04x} and restart it"
            ),
        },
        _ => OpenError {
            kind: OpenErrorKind::Other,
            message: error.to_string(),
        },
    }
}

/// Starts reading keyboard `info`. Errors on opening come back at once; later ones (for example the
/// keyboard being unplugged) are passed to `on_exit` when the thread ends.
///
/// With `grab` the keyboard becomes exclusive: its keys no longer reach any other program
/// (the kernel releases it by itself if Keynortia closes).
pub fn spawn_reader(
    info: &KeyboardInfo,
    grab: bool,
    mut on_event: impl FnMut(KeyEvent) + Send + 'static,
    on_exit: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<Reader, OpenError> {
    let fail = |e: io::Error| open_error(&e, info.vendor, info.product);
    let mut dev = Device::open(info.id.as_str()).map_err(fail)?;
    if grab {
        dev.grab().map_err(fail)?;
    } else {
        // A program like keyd already holds the keyboard exclusively: no key would arrive from here.
        // Better to find out right away (grabbing it and letting go in a moment) than to sit
        // "listening" in silence.
        match dev.grab() {
            Ok(()) => {
                let _ = dev.ungrab();
            }
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => return Err(fail(e)),
            Err(_) => {}
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("keynortia-keyboard".into())
        .spawn(move || {
            let result = read_loop(&mut dev, &stop_flag, &mut on_event);
            if grab {
                let _ = dev.ungrab();
            }
            on_exit(result);
        })
        .map_err(fail)?;
    Ok(Reader {
        stop,
        handle: Some(handle),
    })
}

fn read_loop(
    dev: &mut Device,
    stop: &AtomicBool,
    on_event: &mut impl FnMut(KeyEvent),
) -> Result<(), String> {
    let fd = dev.as_raw_fd();
    while !stop.load(Ordering::Relaxed) {
        // wait for keys, but at most 100 ms, so that `stop` can be checked
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is a valid structure for the whole duration of the call, and we pass just one.
        let ready = unsafe { libc::poll(&mut pfd, 1, 100) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.to_string());
        }
        if ready == 0 {
            continue;
        }
        if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(UNPLUGGED.into());
        }
        match dev.fetch_events() {
            Ok(events) => {
                for ev in events {
                    if ev.event_type() != EventType::KEY {
                        continue;
                    }
                    // 1 = pressed, 0 = released, 2 = auto-repeat (ignored)
                    match ev.value() {
                        1 => on_event(KeyEvent::Press(ev.code())),
                        0 => on_event(KeyEvent::Release(ev.code())),
                        _ => {}
                    }
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.raw_os_error() == Some(libc::ENODEV) => return Err(UNPLUGGED.into()),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyCode as K;

    #[test]
    fn layout_codes_match_evdev_constants() {
        // same sequence as layout::KEYS: digits, QWERTY, ASDF, ZXCV
        let expected = [
            K::KEY_1,
            K::KEY_2,
            K::KEY_3,
            K::KEY_4,
            K::KEY_5,
            K::KEY_6,
            K::KEY_7,
            K::KEY_8,
            K::KEY_9,
            K::KEY_0,
            K::KEY_MINUS,
            K::KEY_EQUAL, //
            K::KEY_Q,
            K::KEY_W,
            K::KEY_E,
            K::KEY_R,
            K::KEY_T,
            K::KEY_Y,
            K::KEY_U,
            K::KEY_I,
            K::KEY_O,
            K::KEY_P,
            K::KEY_LEFTBRACE,
            K::KEY_RIGHTBRACE, //
            K::KEY_A,
            K::KEY_S,
            K::KEY_D,
            K::KEY_F,
            K::KEY_G,
            K::KEY_H,
            K::KEY_J,
            K::KEY_K,
            K::KEY_L,
            K::KEY_SEMICOLON,
            K::KEY_APOSTROPHE, //
            K::KEY_Z,
            K::KEY_X,
            K::KEY_C,
            K::KEY_V,
            K::KEY_B,
            K::KEY_N,
            K::KEY_M,
            K::KEY_COMMA,
            K::KEY_DOT,
            K::KEY_SLASH,
        ];
        assert_eq!(expected.len(), KEYS.len());
        for (key, want) in KEYS.iter().zip(expected) {
            assert_eq!(key.code, want.code(), "key {}", key.label);
        }
        let letters: Vec<u16> = letter_codes().collect();
        assert_eq!(letters.len(), 26);
        for (letter, want) in letters.iter().zip(("QWERTYUIOPASDFGHJKLZXCVBNM").chars()) {
            let key = KEYS.iter().find(|k| k.label == want.to_string()).unwrap();
            assert_eq!(*letter, key.code);
        }
    }

    #[test]
    fn bus_types() {
        assert_eq!(classify_bus(0x03), Connection::Usb);
        assert_eq!(classify_bus(0x05), Connection::Bluetooth);
        assert_eq!(classify_bus(0x11), Connection::Internal, "PS/2 of laptops");
        assert_eq!(classify_bus(0x18), Connection::Internal, "I2C");
        assert_eq!(classify_bus(0x06), Connection::Virtual);
        assert_eq!(classify_bus(0x01), Connection::Other);
    }

    fn origin(bus: u16, sysfs: &str, removable: &str) -> Origin {
        Origin {
            bus,
            sysfs: sysfs.into(),
            removable: removable.into(),
            ..Origin::default()
        }
    }

    #[test]
    fn laptop_keyboard_is_internal() {
        let o = origin(
            0x11,
            "/sys/devices/platform/i8042/serio0/input/input3/event3",
            "",
        );
        assert_eq!(
            classify("AT Translated Set 2 keyboard", &o),
            Connection::Internal
        );
    }

    #[test]
    fn usb_keyboard_on_a_fixed_port_is_internal() {
        let usb = "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-5/1-5:1.0/0003:0B05:19B6.0003/input/input9/event9";
        assert_eq!(
            classify("ITE Device(8910) Keyboard", &origin(0x03, usb, "fixed")),
            Connection::Internal
        );
        assert_eq!(
            classify("Keychron K2", &origin(0x03, usb, "removable")),
            Connection::Usb
        );
        assert_eq!(
            classify("Keychron K2", &origin(0x03, usb, "unknown")),
            Connection::Usb
        );
        assert_eq!(
            classify("Keychron K2", &origin(0x03, usb, "")),
            Connection::Usb
        );
        assert_eq!(
            classify(
                "Apple Internal Keyboard / Trackpad",
                &origin(0x03, usb, "unknown")
            ),
            Connection::Internal
        );
    }

    #[test]
    fn keyboards_made_by_programs_are_virtual_even_if_they_say_usb() {
        // keyd, kmonad, input-remapper... declare a USB bus but live under /sys/devices/virtual/input
        let v = origin(0x03, "/sys/devices/virtual/input/input31/event14", "");
        assert_eq!(classify("keyd virtual keyboard", &v), Connection::Virtual);
        assert_eq!(
            classify("py-evdev-uinput", &origin(0x06, "", "")),
            Connection::Virtual
        );
    }

    #[test]
    fn bluetooth_keyboards_stay_bluetooth_even_through_uhid() {
        let bt = origin(
            0x05,
            "/sys/devices/virtual/misc/uhid/0005:05AC:024F.000A/input/input40/event16",
            "",
        );
        assert_eq!(classify("Keychron K2", &bt), Connection::Bluetooth);
    }

    #[test]
    fn technical_line_tells_how_the_system_sees_the_keyboard() {
        let origin = Origin {
            bus: 0x11,
            phys: "isa0060/serio0/input0".into(),
            ..Origin::default()
        };
        assert_eq!(
            technical(Path::new("/dev/input/event4"), &origin),
            "event4 · bus 0x11 · isa0060/serio0/input0"
        );
    }

    #[test]
    fn only_devices_with_all_letters_are_keyboards() {
        let letters: Vec<u16> = letter_codes().collect();
        assert!(is_keyboard(|c| letters.contains(&c)));
        assert!(
            !is_keyboard(|c| letters.contains(&c) && c != 50),
            "the M is missing"
        );
        assert!(!is_keyboard(|_| false), "mouse, power button...");
    }

    #[test]
    fn event_nodes_sort_numerically() {
        let mut v = [
            PathBuf::from("/dev/input/event10"),
            PathBuf::from("/dev/input/event2"),
        ];
        v.sort_by_key(|p| event_number(p));
        assert_eq!(v[0], PathBuf::from("/dev/input/event2"));
    }

    #[test]
    fn a_keyboard_taken_by_another_program_says_which_line_to_add_to_keyd() {
        let busy = io::Error::from_raw_os_error(libc::EBUSY);
        let error = open_error(&busy, 0x1a2c, 0x9b09);
        assert_eq!(error.kind, OpenErrorKind::Busy);
        assert!(error.message.contains("-1a2c:9b09"), "{}", error.message);
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let error = open_error(&denied, 0x1a2c, 0x9b09);
        assert_eq!(error.kind, OpenErrorKind::Other);
        assert!(error.message.contains("input group"));
    }

    #[test]
    fn missing_permissions_are_explained_only_when_no_keyboard_can_be_read() {
        let mut scanner = Scanner::default();
        assert_eq!(scanner.problem(), None);
        scanner.unreadable = 3;
        let problem = scanner.problem().expect("nothing can be read");
        assert!(problem.command.is_some_and(|c| c.contains("usermod")));
    }
}
