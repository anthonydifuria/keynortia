//! Interface (egui): list of keyboards, choice of instrument and on-screen keyboard.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use crate::devices::{
    self, Connection, DeviceId, KeyEvent, KeyboardInfo, OpenErrorKind, Reader, Scanner,
};
use crate::engine::Engine;
use crate::keyboard_view;
use crate::keyd;
use crate::layout::{self, Instrument, KEYS, NoteNames};
use crate::midi::{self, Target};
use crate::theme::{self, BONE, BRONZE, DIM, Edge, TERRACOTTA, dim};

const SCAN_EVERY: Duration = Duration::from_millis(1500);
const ERROR: Color32 = TERRACOTTA;

/// Choices made for a keyboard. They are remembered while the program is open: if the keyboard is
/// unplugged and plugged back in (a Bluetooth one that goes to sleep, for example) it starts again by itself.
#[derive(Clone, Copy)]
struct Prefs {
    enabled: bool,
    instrument: Instrument,
    grab: bool,
    role: Role,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            enabled: false,
            instrument: Instrument::Piano,
            // a keyboard used as an instrument no longer types: every keyboard has its own job
            grab: true,
            role: Role::Auto,
        }
    }
}

/// What kind of keyboard it is: Keynortia decides, but it can be corrected by hand.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Auto,
    Computer,
    External,
}

impl Role {
    const ALL: [Role; 3] = [Role::Auto, Role::Computer, Role::External];

    fn label(self) -> &'static str {
        match self {
            Role::Auto => "Automatic",
            Role::Computer => "Computer keyboard",
            Role::External => "External keyboard",
        }
    }
}

/// Which section of the list a keyboard is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Computer,
    External,
    Virtual,
}

/// The choice made by hand wins over the automatic one.
fn group_for(role: Role, connection: Connection) -> Group {
    match (role, connection) {
        (Role::Computer, _) | (Role::Auto, Connection::Internal) => Group::Computer,
        (Role::External, _) => Group::External,
        (Role::Auto, Connection::Virtual) => Group::Virtual,
        (Role::Auto, _) => Group::External,
    }
}

fn group_of(slot: &Slot) -> Group {
    group_for(slot.prefs.role, slot.info.connection)
}

/// Label and colour of the badge next to the name.
fn badge(slot: &Slot) -> (&'static str, Color32) {
    match group_of(slot) {
        Group::Computer => ("Internal", badge_color(Connection::Internal)),
        Group::Virtual => ("Virtual", badge_color(Connection::Virtual)),
        Group::External => match slot.info.connection {
            Connection::Usb | Connection::Bluetooth => (
                slot.info.connection.label(),
                badge_color(slot.info.connection),
            ),
            _ => ("External", badge_color(Connection::Other)),
        },
    }
}

struct Slot {
    info: KeyboardInfo,
    prefs: Prefs,
    /// Current instrument, shared with the thread that reads the keys.
    instrument: Arc<AtomicU8>,
    reader: Option<Reader>,
    error: Option<String>,
    /// If keyd holds the keyboard: how to free it.
    keyd_fix: Option<keyd::Fix>,
    /// Asking for confirmation before freeing it.
    confirming: bool,
    /// Waiting for the authorization and for keyd to restart.
    applying: bool,
}

/// What is needed to start and stop the readers.
struct Shared {
    engine: Arc<Engine>,
    ctx: egui::Context,
    /// The readers write their errors here, the interface reads them.
    notices_tx: Sender<(DeviceId, String)>,
    /// The outcome of the keyd change arrives here (keyboard, success or error).
    fixes_tx: Sender<(DeviceId, Result<(), String>)>,
}

pub struct KeynortiaApp {
    shared: Shared,
    notices_rx: Receiver<(DeviceId, String)>,
    fixes_rx: Receiver<(DeviceId, Result<(), String>)>,
    scanner: Scanner,
    last_scan: Option<Instant>,
    slots: Vec<Slot>,
    remembered: HashMap<String, Prefs>,
    selected: Option<DeviceId>,
    /// Where the notes go; `None` = nothing chosen yet (only on systems without virtual ports).
    target: Option<Target>,
    ports: Vec<String>,
    midi_error: Option<String>,
    names: NoteNames,
}

impl KeynortiaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);

        let ctx = cc.egui_ctx.clone();
        let repaint_ctx = ctx.clone();
        let engine = Engine::new(move || repaint_ctx.request_repaint());
        let (notices_tx, notices_rx) = mpsc::channel();
        let (fixes_tx, fixes_rx) = mpsc::channel();

        let ports = midi::list_ports();
        let target = midi::default_target(&ports);
        let mut app = KeynortiaApp {
            shared: Shared {
                engine,
                ctx,
                notices_tx,
                fixes_tx,
            },
            notices_rx,
            fixes_rx,
            scanner: Scanner::default(),
            last_scan: None,
            slots: Vec::new(),
            remembered: HashMap::new(),
            selected: None,
            target: None,
            ports,
            midi_error: None,
            names: NoteNames::Solfege,
        };
        app.connect_midi(target);
        app.scan();
        app
    }

    fn connect_midi(&mut self, target: Option<Target>) {
        let result = match &target {
            Some(target) => midi::connect(target).map(Some),
            None => Ok(None),
        };
        match result {
            Ok(connection) => {
                let sink = connection.map(|c| Box::new(c) as Box<dyn crate::engine::MidiSink>);
                self.shared.engine.set_output(sink);
                self.midi_error = None;
            }
            Err(message) => {
                self.shared.engine.set_output(None);
                self.midi_error = Some(message);
            }
        }
        self.target = target;
    }

    fn scan(&mut self) {
        self.last_scan = Some(Instant::now());
        if self.scanner.refresh() {
            self.sync_slots();
        }
    }

    /// keyd has finished being reconfigured (or has failed): try to open the keyboards again.
    fn fix_done(&mut self, id: &DeviceId, result: Result<(), String>) {
        let Some(ids) = self
            .slots
            .iter()
            .find(|s| &s.info.id == id)
            .map(|s| (s.info.vendor, s.info.product))
        else {
            return;
        };
        let same = |s: &Slot| (s.info.vendor, s.info.product) == ids;
        match result {
            Ok(()) => {
                // keyd leaves alone every keyboard with that same identifier
                for slot in self
                    .slots
                    .iter_mut()
                    .filter(|s| same(s) && s.keyd_fix.is_some())
                {
                    slot.applying = false;
                    slot.prefs.enabled = true;
                    start(&self.shared, slot);
                }
                self.selected = Some(id.clone());
            }
            Err(message) => {
                for slot in self.slots.iter_mut().filter(|s| same(s) && s.applying) {
                    slot.applying = false;
                    slot.error = Some(message.clone());
                }
            }
        }
    }

    /// Brings the keyboards shown in line with the ones detected: new, gone, already known.
    fn sync_slots(&mut self) {
        let mut old = std::mem::take(&mut self.slots);
        for info in self.scanner.keyboards() {
            let slot = match old.iter().position(|s| s.info.id == info.id) {
                Some(i) => {
                    let mut slot = old.swap_remove(i);
                    slot.info = info;
                    slot
                }
                None => {
                    let prefs = self
                        .remembered
                        .get(&identity(&info))
                        .copied()
                        .unwrap_or_default();
                    let mut slot = Slot {
                        instrument: Arc::new(AtomicU8::new(prefs.instrument.index())),
                        prefs,
                        reader: None,
                        error: None,
                        keyd_fix: None,
                        confirming: false,
                        applying: false,
                        info,
                    };
                    if slot.prefs.enabled {
                        start(&self.shared, &mut slot);
                    }
                    slot
                }
            };
            self.slots.push(slot);
        }
        for mut gone in old {
            stop(&self.shared, &mut gone);
            self.remembered.insert(identity(&gone.info), gone.prefs);
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        theme::banner(ui);
        ui.add_space(8.0);
        theme::pane(ui, "MIDI", Edge::Dashed, |ui| {
            let virtual_label = format!("Virtual port \"{}\"", midi::PORT_NAME);
            let mut chosen: Option<Option<Target>> = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(dim("Output"));
                let current = match &self.target {
                    Some(Target::Virtual) => virtual_label.clone(),
                    Some(Target::Port(name)) => name.clone(),
                    None => "(none)".to_string(),
                };
                egui::ComboBox::from_id_salt("midi-output")
                    .width(300.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if midi::VIRTUAL_PORTS
                            && ui
                                .selectable_label(
                                    self.target == Some(Target::Virtual),
                                    &virtual_label,
                                )
                                .clicked()
                        {
                            chosen = Some(Some(Target::Virtual));
                        }
                        for name in &self.ports {
                            let this = Some(Target::Port(name.clone()));
                            if ui.selectable_label(self.target == this, name).clicked() {
                                chosen = Some(this);
                            }
                        }
                        if self.ports.is_empty() && !midi::VIRTUAL_PORTS {
                            ui.label(dim("No MIDI port found: see \"No sound?\" below."));
                        }
                    });
                if theme::button(ui, "Refresh list").clicked() {
                    self.ports = midi::list_ports();
                }
                match (&self.midi_error, &self.target) {
                    (Some(message), _) => {
                        ui.colored_label(ERROR, message);
                    }
                    (None, None) => {
                        ui.colored_label(DIM, "○ no output").on_hover_text(
                            "Pick a MIDI port in the list. To reach plugdata or a DAW on this \
                             system you need a virtual port: see \"No sound?\" below.",
                        );
                    }
                    (None, Some(target)) => {
                        ui.colored_label(BONE, "● ready")
                            .on_hover_text(ready_hint(target));
                        if theme::button(ui, "Test note")
                            .on_hover_text(
                                "Plays a middle C for half a second, without using the keyboards",
                            )
                            .clicked()
                        {
                            self.shared.engine.test_note();
                        }
                    }
                }
                if theme::button(ui, "All notes off").clicked() {
                    self.shared.engine.panic();
                }
            });
            if let Some(target) = chosen {
                self.connect_midi(target);
            }

            ui.horizontal(|ui| {
                let mut s = self.shared.engine.settings();
                let before = s;
                ui.label(dim("Octave"));
                if theme::button_if(ui, s.octave > -3, "-").clicked() {
                    s.octave -= 1;
                }
                ui.label(if s.octave == 0 {
                    " 0".to_string()
                } else {
                    format!("{:+}", s.octave)
                });
                if theme::button_if(ui, s.octave < 3, "+").clicked() {
                    s.octave += 1;
                }
                ui.separator();
                ui.label(dim("Velocity"));
                ui.add(egui::Slider::new(&mut s.velocity, 1..=127));
                ui.separator();
                ui.label(dim("Channel"));
                let mut channel = s.channel + 1;
                ui.add(egui::DragValue::new(&mut channel).range(1..=16));
                s.channel = channel - 1;
                if s != before {
                    self.shared.engine.set_settings(s);
                }
                ui.separator();
                ui.label(dim("Note names"));
                ui.selectable_value(&mut self.names, NoteNames::Solfege, " Do Re Mi ");
                ui.selectable_value(&mut self.names, NoteNames::Letters, " C D E ");
            });
        });
        ui.add_space(4.0);
    }

    fn keyboards(&mut self, ui: &mut egui::Ui) {
        if self.slots.is_empty() {
            theme::rule(ui, "Keyboards");
            if let Some(problem) = self.scanner.problem() {
                ui.colored_label(ERROR, problem.headline);
                ui.label(problem.before);
                if let Some(command) = problem.command {
                    ui.label(RichText::new(command).monospace().color(BRONZE));
                }
                ui.label(problem.after);
            } else {
                ui.label(
                    "No keyboard detected. Plug one in, USB or Bluetooth: it shows up by itself.",
                );
            }
            return;
        }
        let octave = self.shared.engine.settings().octave;
        let (mut computer, mut external, mut made_by_programs) =
            (Vec::new(), Vec::new(), Vec::new());
        for (i, slot) in self.slots.iter().enumerate() {
            match group_of(slot) {
                Group::Computer => computer.push(i),
                Group::External => external.push(i),
                Group::Virtual => made_by_programs.push(i),
            }
        }

        if !computer.is_empty() {
            section(ui, "Computer keyboard");
            for i in computer {
                self.row(ui, i, octave);
            }
        }
        section(ui, "External keyboards");
        if external.is_empty() {
            ui.label(dim(
                "No external keyboard detected. Plug one in, USB or Bluetooth: it shows up by itself.",
            ));
        }
        for i in external {
            self.row(ui, i, octave);
        }
        if !made_by_programs.is_empty() {
            ui.add_space(8.0);
            ui.collapsing(
                dim(format!("Virtual devices ({})", made_by_programs.len())),
                |ui| {
                    ui.label(dim(
                        "Created by other programs (keyd, kmonad, remappers...) that grab the real keyboards \
                         and put them back together: playing through them plays everything at once, the \
                         internal keyboard included. Better to exclude your external keyboard from that program.",
                    ));
                    for i in made_by_programs {
                        self.row(ui, i, octave);
                    }
                },
            );
        }
    }

    /// One keyboard of the list: everything that can be decided about it.
    fn row(&mut self, ui: &mut egui::Ui, index: usize, octave: i8) {
        let slot = &mut self.slots[index];
        let shown = self.selected.as_ref() == Some(&slot.info.id);
        let (badge_label, badge_tint) = badge(slot);
        let title = slot.info.name.clone();
        // the keyboard shown on screen has the focus: a solid frame instead of a dashed one
        let edge = if shown { Edge::Solid } else { Edge::Dashed };
        theme::pane(ui, &title, edge, |ui| {
            ui.horizontal(|ui| {
                let mut enabled = slot.prefs.enabled;
                if theme::checkbox(ui, &mut enabled, "Use as instrument").changed() {
                    slot.prefs.enabled = enabled;
                    if enabled {
                        start(&self.shared, slot);
                        self.selected = Some(slot.info.id.clone());
                    } else {
                        stop(&self.shared, slot);
                    }
                }
                ui.label(theme::bold(format!("[{badge_label}]")).color(badge_tint))
                    .on_hover_text(&slot.info.technical);
                ui.label(dim(format!(
                    "{:04x}:{:04x}",
                    slot.info.vendor, slot.info.product
                )));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if shown {
                        " ● On screen "
                    } else {
                        "[ Show on screen ]"
                    };
                    if ui.selectable_label(shown, label).clicked() {
                        self.selected = Some(slot.info.id.clone());
                    }
                });
            });

            let range = match slot.prefs.instrument.range(|c| slot.info.can_play(c)) {
                Some((lo, hi, count)) => {
                    let shift = |n: u8| (n as i32 + 12 * octave as i32).clamp(0, 127) as u8;
                    format!(
                        "{count} notes, from {} to {}",
                        layout::note_name(shift(lo), self.names),
                        layout::note_name(shift(hi), self.names)
                    )
                }
                None => "no notes".to_string(),
            };
            let reported = match slot.info.reported_keys {
                Some(n) => format!("{n} keys reported ({}) · ", slot.info.size_label()),
                None => format!("{} · ", slot.info.size_label()),
            };
            ui.label(dim(format!(
                "{reported}{}/{} playable keys · {range}",
                slot.info.playable.len(),
                KEYS.len(),
            )));

            ui.horizontal_wrapped(|ui| {
                let mut instrument = slot.prefs.instrument;
                egui::ComboBox::from_id_salt(("instrument", &slot.info.id))
                    .width(260.0)
                    .selected_text(instrument.label())
                    .show_ui(ui, |ui| {
                        for choice in Instrument::ALL {
                            ui.selectable_value(&mut instrument, choice, choice.label());
                        }
                    });
                if instrument != slot.prefs.instrument {
                    self.shared.engine.release_device(&slot.info.id);
                    slot.prefs.instrument = instrument;
                    slot.instrument.store(instrument.index(), Ordering::Relaxed);
                }

                ui.label(dim("Type"));
                let mut role = slot.prefs.role;
                egui::ComboBox::from_id_salt(("role", &slot.info.id))
                    .width(190.0)
                    .selected_text(role.label())
                    .show_ui(ui, |ui| {
                        for choice in Role::ALL {
                            ui.selectable_value(&mut role, choice, choice.label());
                        }
                    });
                if role != slot.prefs.role {
                    slot.prefs.role = role;
                }

                let mut grab = slot.prefs.grab;
                let response = theme::checkbox(ui, &mut grab, "Exclusive (does not type)")
                    .on_hover_text(
                        "The keys of this keyboard only play and no longer type in any other program. \
                         If it is the only keyboard of the computer, to type again untick \
                         \"Use as instrument\" with the mouse.",
                    );
                if response.changed() {
                    slot.prefs.grab = grab;
                    if slot.reader.is_some() {
                        stop(&self.shared, slot);
                        start(&self.shared, slot);
                    }
                }

                if let Some(message) = &slot.error {
                    ui.colored_label(ERROR, message);
                } else if let Some(reader) = &slot.reader {
                    ui.colored_label(BONE, "● listening");
                    if let Some(warning) = reader.warning() {
                        ui.colored_label(BRONZE, warning);
                    }
                } else {
                    ui.colored_label(DIM, "○ off");
                }
            });
            if let Some(fix) = slot.keyd_fix.clone() {
                keyd_help(ui, slot, &fix, &self.shared);
            }
        });
        ui.add_space(4.0);
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let slot = self
            .slots
            .iter()
            .find(|s| self.selected.as_ref() == Some(&s.info.id))
            .or_else(|| self.slots.iter().find(|s| s.reader.is_some()))
            .or_else(|| self.slots.first());
        let instrument = slot.map_or(Instrument::Piano, |s| s.prefs.instrument);
        let held: HashSet<u16> = slot
            .map(|s| self.shared.engine.held_keys(&s.info.id))
            .unwrap_or_default();
        let octave = self.shared.engine.settings().octave;
        let names = self.names;

        let title = match slot {
            Some(s) => format!(
                "On-screen keyboard · {} · {}",
                instrument.label(),
                s.info.name
            ),
            None => format!(
                "On-screen keyboard · {} (preview: no keyboard connected)",
                instrument.label()
            ),
        };
        ui.add_space(4.0);
        theme::pane(ui, &title, Edge::Dashed, |ui| {
            keyboard_view::paint(ui, instrument, slot.map(|s| &s.info), octave, names, &held);
            ui.add_space(4.0);
            let legend = if instrument == Instrument::Piano {
                "Low rows (Z, S...) = low octave, high rows (Q, 2...) = high octave."
            } else {
                "Bronze keys are the Cs, like the marked buttons of a real accordion. Each column to the right goes up 3 semitones."
            };
            ui.label(dim(format!(
                "{legend} Space bar, Shift, Caps Lock, Enter and the other big keys do not play."
            )));
        });
        ui.add_space(2.0);
    }

    fn help(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("No sound?", |ui| {
            if cfg!(windows) {
                ui.label("Keynortia only sends MIDI notes: to hear them you need a synthesizer.");
                ui.label("• Quickest test: pick \"Microsoft GS Wavetable Synth\" above, in \"Output\". It comes with Windows and plays piano-like sounds.");
                ui.label("• To play through plugdata, a DAW or any other program Windows needs a virtual MIDI cable: install loopMIDI (free), create a port in it, pick that port above in \"Output\" (then \"Refresh list\"), and enable the same port among the MIDI inputs of the program.");
                ui.label("• In plugdata: in the settings enable the loopMIDI port among the MIDI inputs and put a [notein] in the patch.");
            } else if cfg!(target_os = "macos") {
                ui.label("Keynortia only sends MIDI notes: to hear them you need a synthesizer (plugdata, a DAW such as GarageBand or Logic, a synth app...).");
                ui.label("• Keep the virtual port and enable \"Keynortia\" among the MIDI inputs of that program (open Keynortia first, so that the port exists).");
                ui.label("• or pick the synthesizer's own port above, in \"Output\" (then \"Refresh list\").");
                ui.label("• In plugdata: in the settings enable \"Keynortia\" among the MIDI inputs and put a [notein] in the patch.");
            } else {
                ui.label("Keynortia only sends MIDI notes: to hear them you need a synthesizer (FluidSynth, a DAW, ...).");
                ui.label("• Pick the synthesizer's port above, in \"Output\" (then \"Refresh list\")");
                ui.label("• or keep the virtual port and connect it to the synthesizer with aconnect, qpwgraph or your DAW.");
                ui.label("• In plugdata: open Keynortia first, then in plugdata's settings enable \"Keynortia\" among the MIDI inputs and put a [notein] in the patch.");
            }
            ui.label("• To check the connection without keyboards press \"Test note\".");
        });
    }
}

impl eframe::App for KeynortiaApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok((id, message)) = self.notices_rx.try_recv() {
            if let Some(slot) = self.slots.iter_mut().find(|s| s.info.id == id) {
                slot.reader = None;
                if message != devices::UNPLUGGED {
                    // a real error: the keyboard stays off until you turn it on again
                    slot.prefs.enabled = false;
                }
                slot.error = Some(message);
            }
        }
        while let Ok((id, result)) = self.fixes_rx.try_recv() {
            self.fix_done(&id, result);
        }
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_EVERY) {
            self.scan();
        }
        ctx.request_repaint_after(SCAN_EVERY);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header")
            .frame(theme::panel_frame())
            .show_separator_line(false)
            .show(ui, |ui| self.header(ui));
        // the on-screen keyboard is always in view; the list of keyboards scrolls above it
        egui::Panel::bottom("on-screen-keyboard")
            .frame(theme::panel_frame())
            .show_separator_line(false)
            .show(ui, |ui| self.preview(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.keyboards(ui);
                    ui.add_space(14.0);
                    self.help(ui);
                });
        });
    }
}

/// Title of a group in the list.
fn section(ui: &mut egui::Ui, title: &str) {
    theme::rule(ui, title);
    ui.add_space(2.0);
}

fn identity(info: &KeyboardInfo) -> String {
    format!("{}|{:04x}:{:04x}", info.name, info.vendor, info.product)
}

fn badge_color(connection: Connection) -> Color32 {
    match connection {
        Connection::Usb | Connection::Bluetooth => BONE,
        Connection::Internal => BRONZE,
        Connection::Virtual | Connection::Other => DIM,
    }
}

/// What the hover text says about the output that is open.
fn ready_hint(target: &Target) -> String {
    match target {
        Target::Virtual => "The MIDI port exists. In the program that should make the sound \
                            (plugdata, a DAW, FluidSynth...) enable it among the MIDI inputs."
            .to_string(),
        Target::Port(name) => format!(
            "Sending the notes to \"{name}\". If that is a virtual cable (loopMIDI, IAC...), \
             enable the same port among the MIDI inputs of the program that makes the sound."
        ),
    }
}

/// Starts reading the keys of the keyboard.
fn start(shared: &Shared, slot: &mut Slot) {
    slot.error = None;
    slot.keyd_fix = None;
    slot.confirming = false;
    let id = slot.info.id.clone();

    let on_key = {
        let engine = Arc::clone(&shared.engine);
        let dev = id.clone();
        let instrument = Arc::clone(&slot.instrument);
        move |event: KeyEvent| match event {
            KeyEvent::Press(code) => engine.press(
                &dev,
                Instrument::from_index(instrument.load(Ordering::Relaxed)),
                code,
            ),
            KeyEvent::Release(code) => engine.release(&dev, code),
        }
    };
    let on_exit = {
        let engine = Arc::clone(&shared.engine);
        let dev = id.clone();
        let tx = shared.notices_tx.clone();
        let ctx = shared.ctx.clone();
        move |result: Result<(), String>| {
            engine.release_device(&dev);
            if let Err(message) = result {
                let _ = tx.send((dev, message));
            }
            ctx.request_repaint();
        }
    };
    match devices::spawn_reader(&slot.info, slot.prefs.grab, on_key, on_exit) {
        Ok(reader) => slot.reader = Some(reader),
        Err(error) => {
            if error.kind == OpenErrorKind::Busy {
                // another program is holding it: if it is keyd, we know how to free it
                slot.keyd_fix = keyd::find_fix(
                    Path::new(keyd::CONFIG_DIR),
                    slot.info.vendor,
                    slot.info.product,
                );
            }
            slot.error = Some(error.message);
            slot.prefs.enabled = false;
        }
    }
}

/// Stops reading and switches off the notes that are still on.
fn stop(shared: &Shared, slot: &mut Slot) {
    if let Some(reader) = slot.reader.take() {
        reader.stop();
    }
    shared.engine.release_device(&slot.info.id);
}

/// Buttons to free a keyboard that keyd holds exclusively.
fn keyd_help(ui: &mut egui::Ui, slot: &mut Slot, fix: &keyd::Fix, shared: &Shared) {
    if slot.applying {
        ui.label(dim(
            "Waiting for the authorization and for keyd to restart…",
        ));
    } else if slot.confirming {
        ui.group(|ui| {
            let backup = if fix.new_content.is_some() {
                format!(" Backup copy: {}.keynortia-backup.", fix.file.display())
            } else {
                String::new()
            };
            ui.label(format!(
                "Keynortia will {} and restart keyd. Your other keyboards stay as they are.{backup} \
                 It will ask for your administrator password.",
                fix.summary
            ));
            ui.horizontal(|ui| {
                if theme::button(ui, "Go ahead").clicked() {
                    slot.confirming = false;
                    slot.applying = true;
                    run_fix(shared, slot.info.id.clone(), fix.clone());
                }
                if theme::button(ui, "Cancel").clicked() {
                    slot.confirming = false;
                }
            });
        });
    } else {
        ui.horizontal(|ui| {
            if theme::button(ui, "Free it from keyd…").clicked() {
                slot.confirming = true;
            }
            if theme::button(ui, "Copy command")
                .on_hover_text(
                    "Copies the command to paste into a terminal, if you prefer to do it by hand",
                )
                .clicked()
            {
                ui.ctx().copy_text(fix.command.clone());
            }
        });
    }
}

/// Applies the keyd change in a thread: it asks for the password and waits for the restart.
fn run_fix(shared: &Shared, id: DeviceId, fix: keyd::Fix) {
    let tx = shared.fixes_tx.clone();
    let ctx = shared.ctx.clone();
    std::thread::spawn(move || {
        let result = keyd::apply(&fix);
        if result.is_ok() {
            // keyd needs a moment to settle down
            std::thread::sleep(Duration::from_millis(800));
        }
        let _ = tx.send((id, result));
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_keyboard_is_exclusive_by_default() {
        // a keyboard used as an instrument no longer types, internal or external
        assert!(Prefs::default().grab);
    }

    #[test]
    fn what_the_user_chooses_beats_the_automatic_choice() {
        assert_eq!(group_for(Role::Computer, Connection::Usb), Group::Computer);
        assert_eq!(
            group_for(Role::External, Connection::Internal),
            Group::External
        );
        assert_eq!(group_for(Role::Auto, Connection::Virtual), Group::Virtual);
        assert_eq!(group_for(Role::Auto, Connection::Other), Group::External);
    }
}
