//! Layout of the instruments: which key plays which note.
//!
//! Rules of version 0.1:
//! * only the small square keys of the 4 main rows are used (digits, QWERTY, ASDF, ZXCV);
//!   the space bar, Caps Lock, Shift, Enter, the arrows and so on do nothing;
//! * keys are identified by their physical code (evdev), not by the printed letter: "Z" is always
//!   the first key at the bottom left of the letter block, even on an Italian or French keyboard;
//! * no custom mapping: the layouts are fixed.

/// MIDI note (0..=127).
pub type Note = u8;

/// Note of the Z key (C3, MIDI 48). The Q key plays middle C (MIDI 60).
pub const BASE_NOTE: Note = 48;

/// Instrument (keyboard mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Instrument {
    /// Piano on two sets of key rows: ZXCV + ASDF (low octave), QWERTY + digits (high octave).
    Piano,
    /// Button accordion, B system: in the Q-A-Z column the semitones rise going down a row.
    AccordionB,
    /// Button accordion, C system: the mirror of the B system (same keyboard, rows a whole tone apart).
    AccordionC,
}

impl Instrument {
    pub const ALL: [Instrument; 3] = [
        Instrument::Piano,
        Instrument::AccordionB,
        Instrument::AccordionC,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Instrument::Piano => "Piano",
            Instrument::AccordionB => "Button accordion (B system)",
            Instrument::AccordionC => "Button accordion (C system)",
        }
    }

    /// Position in `ALL`: used to pass the instrument to another thread inside a number.
    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|&i| i == self).unwrap_or(0) as u8
    }

    pub fn from_index(index: u8) -> Instrument {
        Self::ALL
            .get(index as usize)
            .copied()
            .unwrap_or(Instrument::Piano)
    }

    /// Distance in semitones from the Z key (BASE_NOTE) for the given key, if it plays in this instrument.
    pub fn offset(self, key: &Key) -> Option<i32> {
        let (row, col) = (key.row as usize, key.col as i32);
        // row counted from the QWERTY row downwards: digits = -1, QWERTY = 0, ASDF = 1, ZXCV = 2
        let r = key.row as i32 - 1;
        match self {
            Instrument::Piano => PIANO[row]
                .get(col as usize)
                .copied()
                .flatten()
                .map(i32::from),
            // 3 semitones for every column to the right, 1 semitone for every row down.
            // The ZXCV row starts on C (as in "the C is in the third row" of the B system).
            Instrument::AccordionB => Some(3 * col + r - 2),
            // 3 semitones for every column to the right, 2 for every row down.
            // The QWERTY row starts on C ("the C is in the first row" of the C system).
            Instrument::AccordionC => Some(3 * col + 2 * r),
        }
    }

    /// MIDI note of the key (without octave shift).
    pub fn note(self, key: &Key) -> Option<Note> {
        let n = BASE_NOTE as i32 + self.offset(key)?;
        (0..=127).contains(&n).then_some(n as Note)
    }

    /// MIDI note of the key with code `code`, shifted by `octave` octaves; `None` if the key does not play.
    pub fn note_for_code(self, code: u16, octave: i8) -> Option<Note> {
        let n = self.note(key_by_code(code)?)? as i32 + 12 * octave as i32;
        (0..=127).contains(&n).then_some(n as Note)
    }

    /// Notes that can be reached with only the keys for which `available` is true:
    /// (lowest, highest, how many different notes).
    pub fn range(self, available: impl Fn(u16) -> bool) -> Option<(Note, Note, usize)> {
        let mut notes: Vec<Note> = KEYS
            .iter()
            .filter(|k| available(k.code))
            .filter_map(|k| self.note(k))
            .collect();
        notes.sort_unstable();
        notes.dedup();
        Some((*notes.first()?, *notes.last()?, notes.len()))
    }
}

/// A small key of the physical keyboard.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    /// evdev code (linux/input-event-codes.h).
    pub code: u16,
    /// Legend of the key on a US keyboard (only used to show it in the interface).
    pub label: &'static str,
    /// Row: 0 = digits, 1 = QWERTY, 2 = ASDF, 3 = ZXCV.
    pub row: u8,
    /// Column inside the row, from the left (0 = `1`, Q, A, Z).
    pub col: u8,
}

/// Centre of the first key of each row, in key widths from the left edge of the keyboard.
/// These are the offsets of an ordinary keyboard (Tab = 1.5 keys, Caps Lock = 1.75, Shift = 2.25).
const ROW_START: [f32; 4] = [1.5, 2.0, 2.25, 2.75];

impl Key {
    /// Horizontal centre of the key, in key widths.
    pub fn x(&self) -> f32 {
        ROW_START[self.row as usize] + self.col as f32
    }
}

const fn k(code: u16, label: &'static str, row: u8, col: u8) -> Key {
    Key {
        code,
        label,
        row,
        col,
    }
}

/// All the usable small keys (45), from left to right, row by row.
pub const KEYS: [Key; 45] = [
    // digits
    k(2, "1", 0, 0),
    k(3, "2", 0, 1),
    k(4, "3", 0, 2),
    k(5, "4", 0, 3),
    k(6, "5", 0, 4),
    k(7, "6", 0, 5),
    k(8, "7", 0, 6),
    k(9, "8", 0, 7),
    k(10, "9", 0, 8),
    k(11, "0", 0, 9),
    k(12, "-", 0, 10),
    k(13, "=", 0, 11),
    // QWERTY
    k(16, "Q", 1, 0),
    k(17, "W", 1, 1),
    k(18, "E", 1, 2),
    k(19, "R", 1, 3),
    k(20, "T", 1, 4),
    k(21, "Y", 1, 5),
    k(22, "U", 1, 6),
    k(23, "I", 1, 7),
    k(24, "O", 1, 8),
    k(25, "P", 1, 9),
    k(26, "[", 1, 10),
    k(27, "]", 1, 11),
    // ASDF
    k(30, "A", 2, 0),
    k(31, "S", 2, 1),
    k(32, "D", 2, 2),
    k(33, "F", 2, 3),
    k(34, "G", 2, 4),
    k(35, "H", 2, 5),
    k(36, "J", 2, 6),
    k(37, "K", 2, 7),
    k(38, "L", 2, 8),
    k(39, ";", 2, 9),
    k(40, "'", 2, 10),
    // ZXCV
    k(44, "Z", 3, 0),
    k(45, "X", 3, 1),
    k(46, "C", 3, 2),
    k(47, "V", 3, 3),
    k(48, "B", 3, 4),
    k(49, "N", 3, 5),
    k(50, "M", 3, 6),
    k(51, ",", 3, 7),
    k(52, ".", 3, 8),
    k(53, "/", 3, 9),
];

pub fn key_by_code(code: u16) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.code == code)
}

/// Piano: semitones above the C of the Z key, by row and column (None = the key does not play).
///
/// The black keys sit above and between the white ones, like on a piano:
/// S = C#, D = D#, G = F#... and, in the high octave, 2 = C#, 3 = D#, 5 = F#...
/// Notes 12..=16 can be played both from the low rows (`,` `.` `/`) and from the high ones (Q W E).
const PIANO: [&[Option<u8>]; 4] = [
    // digits: black keys of the high octave
    &[
        None,
        Some(13),
        Some(15),
        None,
        Some(18),
        Some(20),
        Some(22),
        None,
        Some(25),
        Some(27),
        None,
        Some(30),
    ],
    // QWERTY: white keys of the high octave (Q = C, I = the C above)
    &[
        Some(12),
        Some(14),
        Some(16),
        Some(17),
        Some(19),
        Some(21),
        Some(23),
        Some(24),
        Some(26),
        Some(28),
        Some(29),
        Some(31),
    ],
    // ASDF: black keys of the low octave
    &[
        None,
        Some(1),
        Some(3),
        None,
        Some(6),
        Some(8),
        Some(10),
        None,
        Some(13),
        Some(15),
        None,
    ],
    // ZXCV: white keys of the low octave (Z = C)
    &[
        Some(0),
        Some(2),
        Some(4),
        Some(5),
        Some(7),
        Some(9),
        Some(11),
        Some(12),
        Some(14),
        Some(16),
    ],
];

/// How to write the names of the notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteNames {
    /// Do Re Mi Fa Sol La Si
    Solfege,
    /// C D E F G A B
    Letters,
}

/// Name of the note with the octave in scientific notation (middle C, MIDI 60, is C4 / Do4).
pub fn note_name(note: Note, names: NoteNames) -> String {
    const SOLFEGE: [&str; 12] = [
        "Do", "Do#", "Re", "Re#", "Mi", "Fa", "Fa#", "Sol", "Sol#", "La", "La#", "Si",
    ];
    const LETTERS: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let table = match names {
        NoteNames::Solfege => &SOLFEGE,
        NoteNames::Letters => &LETTERS,
    };
    format!("{}{}", table[(note % 12) as usize], note as i32 / 12 - 1)
}

/// True for the black keys of the piano (C#, D#, F#, G#, A#).
pub fn is_black(note: Note) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(label: &str) -> u16 {
        KEYS.iter().find(|k| k.label == label).unwrap().code
    }

    fn note(inst: Instrument, label: &str) -> i32 {
        inst.note_for_code(code(label), 0).unwrap() as i32
    }

    #[test]
    fn key_table_is_sane() {
        let mut codes: Vec<u16> = KEYS.iter().map(|k| k.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), KEYS.len(), "duplicate codes");
        for pair in KEYS.windows(2) {
            if pair[0].row == pair[1].row {
                assert_eq!(pair[0].col + 1, pair[1].col, "columns are not consecutive");
            }
        }
        for (row, expected) in [12usize, 12, 11, 10].into_iter().enumerate() {
            assert_eq!(
                KEYS.iter().filter(|k| k.row as usize == row).count(),
                expected
            );
            assert_eq!(PIANO[row].len(), expected, "piano table, row {row}");
        }
    }

    #[test]
    fn piano_examples_from_the_spec() {
        use Instrument::Piano;
        assert_eq!(note(Piano, "Z"), 48, "Z = C");
        assert_eq!(note(Piano, "Q"), 60, "Q = the C above");
        assert_eq!(note(Piano, "2"), 61, "if Q is C, 2 is C#");
        assert_eq!(note(Piano, "S"), 49, "S = C#");
        assert_eq!(note(Piano, ","), 60, "the comma is the same C as Q");
        assert_eq!(note(Piano, "]"), 79);
        assert_eq!(note_name(48, NoteNames::Letters), "C3");
        assert_eq!(note_name(61, NoteNames::Letters), "C#4");
        assert_eq!(note_name(60, NoteNames::Solfege), "Do4");
    }

    #[test]
    fn piano_covers_32_contiguous_notes() {
        let mut notes: Vec<i32> = KEYS
            .iter()
            .filter_map(|k| Instrument::Piano.note(k))
            .map(i32::from)
            .collect();
        notes.sort_unstable();
        notes.dedup();
        assert_eq!(notes, (48..=79).collect::<Vec<_>>());
        assert_eq!(Instrument::Piano.range(|_| true), Some((48, 79, 32)));
    }

    #[test]
    fn piano_black_keys_are_where_a_piano_has_them() {
        for k in &KEYS {
            let Some(n) = Instrument::Piano.note(k) else {
                continue;
            };
            // rows 0 and 2 = black keys, rows 1 and 3 = white keys
            assert_eq!(is_black(n), k.row % 2 == 0, "key {}", k.label);
            if is_black(n) {
                // the black key sits exactly between the white key below it on the left and the one on the right
                let white_row = k.row + 1;
                for (dn, dx) in [(-1i32, -0.5f32), (1, 0.5)] {
                    let w = KEYS.iter().find(|w| {
                        w.row == white_row
                            && Instrument::Piano.note(w) == Some((n as i32 + dn) as u8)
                    });
                    let w = w.unwrap_or_else(|| panic!("no white key next to {}", k.label));
                    assert!(
                        (w.x() - (k.x() + dx)).abs() < 1e-6,
                        "{} is not aligned",
                        k.label
                    );
                }
            }
        }
    }

    #[test]
    fn accordion_rows_climb_by_minor_thirds() {
        for inst in [Instrument::AccordionB, Instrument::AccordionC] {
            for pair in KEYS.windows(2) {
                if pair[0].row == pair[1].row {
                    let d = inst.offset(&pair[1]).unwrap() - inst.offset(&pair[0]).unwrap();
                    assert_eq!(d, 3, "{inst:?}: {} -> {}", pair[0].label, pair[1].label);
                }
            }
        }
    }

    #[test]
    fn accordion_b_first_column_and_diagonals() {
        use Instrument::AccordionB as B;
        // column 1: Q-A-Z = A#, B, C (the C is in the third row)
        assert_eq!([note(B, "Q"), note(B, "A"), note(B, "Z")], [46, 47, 48]);
        // column 2: W-S-X, three semitones higher
        assert_eq!([note(B, "W"), note(B, "S"), note(B, "X")], [49, 50, 51]);
        // down and to the right +1 semitone, up and to the right +2 (a whole tone)
        assert_eq!(note(B, "Z") - note(B, "A"), 1);
        assert_eq!(note(B, "W") - note(B, "A"), 2);
        assert_eq!(
            note(B, "1"),
            45,
            "the digits row is the fourth row, above the Q"
        );
    }

    #[test]
    fn accordion_c_is_the_mirror_of_b() {
        use Instrument::AccordionC as C;
        // the C is in the first row; down and to the right +2, up and to the right +1
        assert_eq!([note(C, "Q"), note(C, "A"), note(C, "Z")], [48, 50, 52]);
        assert_eq!(note(C, "A") - note(C, "Q"), 2);
        assert_eq!(note(C, "W") - note(C, "A"), 1);
    }

    #[test]
    fn instrument_index_round_trip() {
        for inst in Instrument::ALL {
            assert_eq!(Instrument::from_index(inst.index()), inst);
        }
        assert_eq!(Instrument::from_index(200), Instrument::Piano);
    }

    #[test]
    fn octave_shift_and_limits() {
        let z = code("Z");
        assert_eq!(Instrument::Piano.note_for_code(z, 1), Some(60));
        assert_eq!(Instrument::Piano.note_for_code(z, -4), Some(0));
        assert_eq!(Instrument::Piano.note_for_code(z, -5), None);
        assert_eq!(
            Instrument::Piano.note_for_code(code("F"), 0),
            None,
            "F is not a black key"
        );
        assert_eq!(
            Instrument::Piano.note_for_code(57, 0),
            None,
            "the space bar does not play"
        );
    }
}
