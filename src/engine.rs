//! Engine: turns pressed keys into MIDI notes.
//!
//! The state sits behind a single `Mutex` and is used both by the threads that read the keyboards
//! (to play right away, with no hand-offs) and by the interface (to show the pressed keys).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::devices::DeviceId;
use crate::layout::{Instrument, Note};

/// Where the MIDI messages go.
pub trait MidiSink: Send {
    fn send(&mut self, message: &[u8]);
}

impl MidiSink for midir::MidiOutputConnection {
    fn send(&mut self, message: &[u8]) {
        let _ = midir::MidiOutputConnection::send(self, message);
    }
}

/// Settings shared by all keyboards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Octave shift, from -3 to +3.
    pub octave: i8,
    /// Note velocity, from 1 to 127.
    pub velocity: u8,
    /// MIDI channel from 0 to 15 (shown as 1-16 in the interface).
    pub channel: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            octave: 0,
            velocity: 100,
            channel: 0,
        }
    }
}

pub struct Engine {
    inner: Mutex<Inner>,
    /// Called after every change of the pressed keys (used to redraw the window).
    on_change: Box<dyn Fn() + Send + Sync>,
}

struct Inner {
    out: Option<Box<dyn MidiSink>>,
    settings: Settings,
    /// How many times each note is held down, per channel: two keys (or two keyboards) can play
    /// the same note, and the note only stops when all of them are released.
    counts: [[u8; 128]; 16],
    /// Pressed key -> (channel, note) actually sent: the release stops the right note
    /// even if the octave or the channel changed in the meantime.
    held: HashMap<(DeviceId, u16), (u8, Note)>,
}

impl Inner {
    fn send(&mut self, message: &[u8]) {
        if let Some(out) = self.out.as_mut() {
            out.send(message);
        }
    }

    fn release_key(&mut self, dev: &DeviceId, code: u16) -> bool {
        let Some((channel, note)) = self.held.remove(&(dev.clone(), code)) else {
            return false;
        };
        let count = &mut self.counts[channel as usize][note as usize];
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.send(&[0x80 | channel, note, 0]);
        }
        true
    }

    fn all_off(&mut self) {
        self.held.clear();
        self.counts = [[0; 128]; 16];
        for channel in 0..16u8 {
            self.send(&[0xB0 | channel, 120, 0]); // all sound off
            self.send(&[0xB0 | channel, 123, 0]); // all notes off
        }
    }
}

impl Engine {
    pub fn new(on_change: impl Fn() + Send + Sync + 'static) -> Arc<Engine> {
        Arc::new(Engine {
            inner: Mutex::new(Inner {
                out: None,
                settings: Settings::default(),
                counts: [[0; 128]; 16],
                held: HashMap::new(),
            }),
            on_change: Box::new(on_change),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Changes the MIDI output (first switching off the notes left on the old one).
    pub fn set_output(&self, out: Option<Box<dyn MidiSink>>) {
        {
            let mut g = self.lock();
            g.all_off();
            g.out = out;
        }
        (self.on_change)();
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings
    }

    pub fn set_settings(&self, settings: Settings) {
        self.lock().settings = settings;
    }

    /// Key pressed on keyboard `dev` while playing `instrument`.
    pub fn press(&self, dev: &DeviceId, instrument: Instrument, code: u16) {
        {
            let mut g = self.lock();
            let Settings {
                octave,
                velocity,
                channel,
            } = g.settings;
            let Some(note) = instrument.note_for_code(code, octave) else {
                return;
            };
            let key = (dev.clone(), code);
            if g.held.contains_key(&key) {
                return;
            }
            g.held.insert(key, (channel, note));
            let count = &mut g.counts[channel as usize][note as usize];
            *count = count.saturating_add(1);
            if *count == 1 {
                g.send(&[0x90 | channel, note, velocity]);
            }
        }
        (self.on_change)();
    }

    pub fn release(&self, dev: &DeviceId, code: u16) {
        if self.lock().release_key(dev, code) {
            (self.on_change)();
        }
    }

    /// Releases all the keys of a keyboard (instrument change, keyboard turned off or unplugged).
    pub fn release_device(&self, dev: &DeviceId) {
        let changed = {
            let mut g = self.lock();
            let codes: Vec<u16> = g
                .held
                .keys()
                .filter(|(d, _)| d == dev)
                .map(|(_, c)| *c)
                .collect();
            for &code in &codes {
                g.release_key(dev, code);
            }
            !codes.is_empty()
        };
        if changed {
            (self.on_change)();
        }
    }

    /// Plays a middle C for half a second: it checks the connection to the synthesizer
    /// (plugdata, a DAW...) without using the keyboards.
    pub fn test_note(self: &Arc<Self>) {
        self.pulse(60, Duration::from_millis(500));
    }

    fn pulse(self: &Arc<Self>, note: Note, length: Duration) {
        let channel = {
            let mut g = self.lock();
            let Settings {
                velocity, channel, ..
            } = g.settings;
            g.send(&[0x90 | channel, note, velocity]);
            channel
        };
        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(length);
            engine.lock().send(&[0x80 | channel, note, 0]);
        });
    }

    /// Switches off all notes at once, on every channel.
    pub fn panic(&self) {
        self.lock().all_off();
        (self.on_change)();
    }

    /// Codes of the keys currently held down on keyboard `dev` (to light them up on screen).
    pub fn held_keys(&self, dev: &DeviceId) -> HashSet<u16> {
        self.lock()
            .held
            .keys()
            .filter(|(d, _)| d == dev)
            .map(|(_, c)| *c)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Vec<u8>>>>);

    impl MidiSink for Recorder {
        fn send(&mut self, message: &[u8]) {
            self.0.lock().unwrap().push(message.to_vec());
        }
    }

    impl Recorder {
        fn take(&self) -> Vec<Vec<u8>> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    fn setup() -> (Arc<Engine>, Recorder) {
        let rec = Recorder::default();
        let engine = Engine::new(|| {});
        engine.set_output(Some(Box::new(rec.clone())));
        (engine, rec)
    }

    const Z: u16 = 44;
    const Q: u16 = 16;
    const COMMA: u16 = 51;
    const SPACE: u16 = 57;

    fn kb() -> DeviceId {
        DeviceId::new("/dev/input/event5")
    }

    #[test]
    fn note_on_and_off() {
        let (e, rec) = setup();
        e.press(&kb(), Instrument::Piano, Z);
        e.release(&kb(), Z);
        assert_eq!(rec.take(), vec![vec![0x90, 48, 100], vec![0x80, 48, 0]]);
    }

    #[test]
    fn test_note_plays_and_then_stops_by_itself() {
        let (e, rec) = setup();
        e.pulse(60, Duration::from_millis(100));
        assert_eq!(rec.take(), vec![vec![0x90, 60, 100]]);
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(rec.take(), vec![vec![0x80, 60, 0]]);
    }

    #[test]
    fn keys_that_do_nothing_stay_silent() {
        let (e, rec) = setup();
        e.press(&kb(), Instrument::Piano, SPACE);
        e.release(&kb(), SPACE);
        assert!(rec.take().is_empty());
    }

    #[test]
    fn repeated_press_does_not_retrigger() {
        let (e, rec) = setup();
        e.press(&kb(), Instrument::Piano, Z);
        e.press(&kb(), Instrument::Piano, Z);
        assert_eq!(rec.take().len(), 1);
    }

    #[test]
    fn two_keys_with_the_same_note_switch_it_off_only_when_both_are_released() {
        let (e, rec) = setup();
        e.press(&kb(), Instrument::Piano, COMMA); // C4
        e.press(&kb(), Instrument::Piano, Q); // C4
        e.release(&kb(), COMMA);
        assert_eq!(
            rec.take(),
            vec![vec![0x90, 60, 100]],
            "the note must stay on"
        );
        e.release(&kb(), Q);
        assert_eq!(rec.take(), vec![vec![0x80, 60, 0]]);
    }

    #[test]
    fn release_switches_off_the_note_that_was_started() {
        let (e, rec) = setup();
        e.press(&kb(), Instrument::Piano, Z);
        e.set_settings(Settings {
            octave: 1,
            ..e.settings()
        });
        e.release(&kb(), Z);
        assert_eq!(rec.take(), vec![vec![0x90, 48, 100], vec![0x80, 48, 0]]);
    }

    #[test]
    fn channel_and_velocity_are_used() {
        let (e, rec) = setup();
        e.set_settings(Settings {
            octave: 0,
            velocity: 64,
            channel: 2,
        });
        e.press(&kb(), Instrument::Piano, Z);
        assert_eq!(rec.take(), vec![vec![0x92, 48, 64]]);
    }

    #[test]
    fn unplugging_a_keyboard_releases_its_notes() {
        let (e, rec) = setup();
        e.press(&kb(), Instrument::Piano, Z);
        e.press(&kb(), Instrument::Piano, Q);
        rec.take();
        e.release_device(&kb());
        let mut off = rec.take();
        off.sort();
        assert_eq!(off, vec![vec![0x80, 48, 0], vec![0x80, 60, 0]]);
        assert!(e.held_keys(&kb()).is_empty());
    }

    #[test]
    fn keyboards_are_independent() {
        let (e, _rec) = setup();
        let other = DeviceId::new("/dev/input/event9");
        e.press(&kb(), Instrument::Piano, Z);
        e.press(&other, Instrument::AccordionB, Z);
        e.release_device(&kb());
        assert!(e.held_keys(&kb()).is_empty());
        assert_eq!(e.held_keys(&other), HashSet::from([Z]));
    }

    #[test]
    fn panic_switches_everything_off() {
        let (e, rec) = setup();
        e.press(&kb(), Instrument::Piano, Z);
        rec.take();
        e.panic();
        let sent = rec.take();
        assert_eq!(
            sent.len(),
            32,
            "all sound off + all notes off on 16 channels"
        );
        assert!(e.held_keys(&kb()).is_empty());
    }
}
