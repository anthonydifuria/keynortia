//! MIDI output: a virtual "Keynortia" port to connect a synthesizer or a music program to
//! (Linux and macOS), or a port that already exists (FluidSynth, loopMIDI, the synthesizer
//! that comes with Windows...).

#[cfg(unix)]
use midir::os::unix::VirtualOutput;
use midir::{MidiOutput, MidiOutputConnection};

/// Name of the virtual port created by Keynortia (and of the MIDI client).
pub const PORT_NAME: &str = "Keynortia";

/// Whether the system lets a program create its own MIDI port. Windows does not: there, to reach
/// another program (plugdata, a DAW...) you need a port made by a driver such as loopMIDI.
pub const VIRTUAL_PORTS: bool = cfg!(unix);

/// Where to send the notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Creates the virtual "Keynortia" port.
    Virtual,
    /// Connects to a port that already exists, chosen by name.
    Port(String),
}

/// Names of the MIDI output ports that already exist (synthesizers, DAWs...).
pub fn list_ports() -> Vec<String> {
    let Ok(out) = MidiOutput::new("Keynortia (port list)") else {
        return Vec::new();
    };
    out.ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect()
}

/// The output to open when the program starts: the virtual port where there is one; elsewhere
/// the best of the ports that exist (see `pick_port`), if any.
pub fn default_target(ports: &[String]) -> Option<Target> {
    if VIRTUAL_PORTS {
        Some(Target::Virtual)
    } else {
        pick_port(ports).map(Target::Port)
    }
}

/// Where a system without virtual ports should send the notes at the start: a port that was made
/// for Keynortia, else a loopMIDI one, else the synthesizer built into Windows, which at least makes
/// a sound right away. Any other port (a hardware synthesizer, say) is for the user to choose.
pub fn pick_port(ports: &[String]) -> Option<String> {
    let find = |needle: &str| {
        ports
            .iter()
            .find(|p| p.to_lowercase().contains(needle))
            .cloned()
    };
    find("keynortia")
        .or_else(|| find("loopmidi"))
        .or_else(|| find("wavetable"))
}

/// Opens the MIDI output. The error is already worded for the user.
pub fn connect(target: &Target) -> Result<MidiOutputConnection, String> {
    let out = MidiOutput::new(PORT_NAME).map_err(|e| format!("MIDI not available: {e}"))?;
    match target {
        Target::Virtual => create_virtual(out),
        Target::Port(name) => {
            let port = out
                .ports()
                .into_iter()
                .find(|p| out.port_name(p).is_ok_and(|n| &n == name))
                .ok_or_else(|| format!("the MIDI port \"{name}\" is gone"))?;
            out.connect(&port, PORT_NAME)
                .map_err(|e| format!("could not connect to \"{name}\": {e}"))
        }
    }
}

#[cfg(unix)]
fn create_virtual(out: MidiOutput) -> Result<MidiOutputConnection, String> {
    out.create_virtual(PORT_NAME)
        .map_err(|e| format!("could not create the virtual port: {e}"))
}

#[cfg(not(unix))]
fn create_virtual(_out: MidiOutput) -> Result<MidiOutputConnection, String> {
    Err("this system cannot create virtual MIDI ports: pick an existing one".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ports(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_port_made_for_keynortia_wins() {
        let list = ports(&[
            "Microsoft GS Wavetable Synth",
            "loopMIDI Port",
            "Keynortia 1",
        ]);
        assert_eq!(pick_port(&list).as_deref(), Some("Keynortia 1"));
    }

    #[test]
    fn then_loopmidi_then_the_windows_synth() {
        let list = ports(&["Microsoft GS Wavetable Synth", "loopMIDI Port 2"]);
        assert_eq!(pick_port(&list).as_deref(), Some("loopMIDI Port 2"));
        let list = ports(&["Microsoft GS Wavetable Synth"]);
        assert_eq!(
            pick_port(&list).as_deref(),
            Some("Microsoft GS Wavetable Synth")
        );
    }

    #[test]
    fn other_ports_are_left_for_the_user_to_choose() {
        assert_eq!(pick_port(&ports(&["Roland JV-1080", "USB MIDI"])), None);
        assert_eq!(pick_port(&[]), None);
    }
}
