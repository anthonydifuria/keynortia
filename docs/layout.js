/* Layout of the instruments: which key plays which note.
   The same rules as src/layout.rs in the program. Keys are identified by their position
   (KeyboardEvent.code), not by the letter printed on them. */
const KeynortiaLayout = (() => {
  'use strict';

  const BASE_NOTE = 48; // the Z key plays C3 (MIDI 48); on the piano Q plays middle C
  const ROW_START = [1.5, 2.0, 2.25, 2.75]; // centre of the first key of each row, in key widths
  const LEFT = 1.0; // left edge of the keyboard, in key widths
  const WIDTH_UNITS = 12.5;

  // [KeyboardEvent.code, legend on a US keyboard, row, column]
  const KEYS = [
    ['Digit1', '1', 0, 0], ['Digit2', '2', 0, 1], ['Digit3', '3', 0, 2], ['Digit4', '4', 0, 3],
    ['Digit5', '5', 0, 4], ['Digit6', '6', 0, 5], ['Digit7', '7', 0, 6], ['Digit8', '8', 0, 7],
    ['Digit9', '9', 0, 8], ['Digit0', '0', 0, 9], ['Minus', '-', 0, 10], ['Equal', '=', 0, 11],
    ['KeyQ', 'Q', 1, 0], ['KeyW', 'W', 1, 1], ['KeyE', 'E', 1, 2], ['KeyR', 'R', 1, 3],
    ['KeyT', 'T', 1, 4], ['KeyY', 'Y', 1, 5], ['KeyU', 'U', 1, 6], ['KeyI', 'I', 1, 7],
    ['KeyO', 'O', 1, 8], ['KeyP', 'P', 1, 9], ['BracketLeft', '[', 1, 10], ['BracketRight', ']', 1, 11],
    ['KeyA', 'A', 2, 0], ['KeyS', 'S', 2, 1], ['KeyD', 'D', 2, 2], ['KeyF', 'F', 2, 3],
    ['KeyG', 'G', 2, 4], ['KeyH', 'H', 2, 5], ['KeyJ', 'J', 2, 6], ['KeyK', 'K', 2, 7],
    ['KeyL', 'L', 2, 8], ['Semicolon', ';', 2, 9], ['Quote', "'", 2, 10],
    ['KeyZ', 'Z', 3, 0], ['KeyX', 'X', 3, 1], ['KeyC', 'C', 3, 2], ['KeyV', 'V', 3, 3],
    ['KeyB', 'B', 3, 4], ['KeyN', 'N', 3, 5], ['KeyM', 'M', 3, 6], ['Comma', ',', 3, 7],
    ['Period', '.', 3, 8], ['Slash', '/', 3, 9],
  ].map(([code, label, row, col]) => ({ code, label, row, col, x: ROW_START[row] + col }));

  const INSTRUMENTS = [
    { id: 'piano', label: 'Piano', short: 'Piano' },
    { id: 'accB', label: 'Button accordion (B system)', short: 'Accordion B' },
    { id: 'accC', label: 'Button accordion (C system)', short: 'Accordion C' },
  ];

  // Piano: semitones above the C of the Z key, by row and column (null = the key does not play).
  const PIANO = [
    [null, 13, 15, null, 18, 20, 22, null, 25, 27, null, 30], // digits: black keys of the high octave
    [12, 14, 16, 17, 19, 21, 23, 24, 26, 28, 29, 31], // QWERTY: white keys of the high octave
    [null, 1, 3, null, 6, 8, 10, null, 13, 15, null], // ASDF: black keys of the low octave
    [0, 2, 4, 5, 7, 9, 11, 12, 14, 16], // ZXCV: white keys of the low octave
  ];

  /** Distance in semitones from the Z key, or null if the key does not play. */
  function offset(inst, key) {
    const r = key.row - 1; // digits = -1, QWERTY = 0, ASDF = 1, ZXCV = 2
    if (inst === 'piano') return PIANO[key.row][key.col];
    if (inst === 'accB') return 3 * key.col + r - 2; // C is in the third row
    if (inst === 'accC') return 3 * key.col + 2 * r; // C is in the first row
    return null;
  }

  /** MIDI note of the key, shifted by `octave` octaves; null if it does not play. */
  function note(inst, key, octave) {
    const o = offset(inst, key);
    if (o === null || o === undefined) return null;
    const n = BASE_NOTE + o + 12 * octave;
    return n >= 0 && n <= 127 ? n : null;
  }

  const SOLFEGE = ['Do', 'Do#', 'Re', 'Re#', 'Mi', 'Fa', 'Fa#', 'Sol', 'Sol#', 'La', 'La#', 'Si'];
  const LETTERS = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];

  /** Name of the note with its octave; middle C (MIDI 60) is C4 / Do4. names: 'solfege' or 'letters'. */
  function noteName(n, names) {
    const table = names === 'letters' ? LETTERS : SOLFEGE;
    return table[n % 12] + (Math.floor(n / 12) - 1);
  }

  /** True for the black keys of the piano (C#, D#, F#, G#, A#). */
  function isBlack(n) {
    return [1, 3, 6, 8, 10].includes(n % 12);
  }

  const byCode = new Map(KEYS.map((k) => [k.code, k]));
  const instrument = (id) => INSTRUMENTS.find((i) => i.id === id) || INSTRUMENTS[0];

  return { BASE_NOTE, LEFT, WIDTH_UNITS, KEYS, INSTRUMENTS, byCode, instrument, offset, note, noteName, isBlack };
})();
if (typeof module !== 'undefined') module.exports = KeynortiaLayout;
