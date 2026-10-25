/* Wires the page: the playable keyboard, the copy buttons and the download button. */
(() => {
  'use strict';
  const L = KeynortiaLayout;
  const synth = KeynortiaSynth.create();
  const $ = (id) => document.getElementById(id);

  const board = $('board');
  const tab = $('pane-tab');
  const cap = $('cap');
  const readout = $('readout');
  const octVal = $('oct-val');
  const listenBtn = $('listen');
  const listenBox = $('listen-box');
  const status = $('status');
  const audioNote = $('audio-note');

  const state = { inst: 'piano', octave: 0, names: 'solfege', listening: true };
  const held = new Map(); // key code -> { note, by: 'kbd' | 'ptr' }
  const holders = new Map(); // note -> Set of key codes holding it (a note ends when the last one is released)
  const els = new Map(); // key code -> button

  const LEGEND = {
    piano: 'Low rows (Z, S...) = low octave, high rows (Q, 2...) = high octave.',
    acc: 'Bronze keys are the Cs, like the marked buttons of a real accordion. Each column to the right goes up 3 semitones.',
  };
  const BIG_KEYS = 'Space bar, Shift, Caps Lock, Enter and the other big keys do not play.';

  // ---- the keys -------------------------------------------------------------------------
  for (const key of L.KEYS) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'key';
    b.style.left = (((key.x - 0.45 - L.LEFT) / L.WIDTH_UNITS) * 100).toFixed(3) + '%';
    b.style.top = (key.row * 25 + 1.25).toFixed(3) + '%';
    const lg = document.createElement('span');
    lg.className = 'lg';
    lg.textContent = key.label;
    const nm = document.createElement('span');
    nm.className = 'nm';
    b.append(lg, nm);
    b.addEventListener('pointerdown', (e) => {
      if (e.button > 0) return;
      if (b.setPointerCapture) b.setPointerCapture(e.pointerId);
      press(key.code, 'ptr');
    });
    const up = () => release(key.code, 'ptr');
    b.addEventListener('pointerup', up);
    b.addEventListener('pointercancel', up);
    b.addEventListener('lostpointercapture', up);
    b.addEventListener('contextmenu', (e) => e.preventDefault());
    board.appendChild(b);
    els.set(key.code, b);
  }

  function look(key) {
    const n = L.note(state.inst, key, state.octave);
    if (n === null) return { n, cls: 'is-silent' };
    if (state.inst !== 'piano' && n % 12 === 0) return { n, cls: 'is-mark' };
    return { n, cls: L.isBlack(n) ? 'is-sharp' : 'is-natural' };
  }

  function render() {
    for (const key of L.KEYS) {
      const el = els.get(key.code);
      const { n, cls } = look(key);
      el.classList.remove('is-silent', 'is-mark', 'is-sharp', 'is-natural');
      el.classList.add(cls);
      const nm = el.querySelector('.nm');
      if (n === null) {
        nm.textContent = '';
        el.title = 'Key ' + key.label + ': does not play in this mode';
        el.setAttribute('aria-label', el.title);
      } else {
        const name = L.noteName(n, state.names);
        nm.textContent = name;
        el.style.setProperty('--len', String(name.length));
        el.title = 'Key ' + key.label + ' = ' + name + ' (MIDI note ' + n + ')';
        el.setAttribute('aria-label', 'Key ' + key.label + ': ' + name);
      }
    }
    tab.textContent = 'On-screen keyboard · ' + L.instrument(state.inst).label;
    cap.textContent = (state.inst === 'piano' ? LEGEND.piano : LEGEND.acc) + ' ' + BIG_KEYS;
    octVal.textContent = String(state.octave);
    $('oct-down').disabled = state.octave <= -3;
    $('oct-up').disabled = state.octave >= 3;
    document.querySelectorAll('[data-inst]').forEach((b) => b.setAttribute('aria-checked', String(b.dataset.inst === state.inst)));
    document.querySelectorAll('[data-names]').forEach((b) => b.setAttribute('aria-checked', String(b.dataset.names === state.names)));
  }

  function showReadout() {
    const notes = [...holders.keys()].sort((a, b) => a - b);
    readout.textContent = notes.length ? notes.map((n) => L.noteName(n, state.names)).join(' · ') : '—';
  }

  // ---- playing --------------------------------------------------------------------------
  function press(code, by) {
    if (held.has(code)) return;
    const key = L.byCode.get(code);
    const n = key ? L.note(state.inst, key, state.octave) : null;
    if (n === null) return;
    held.set(code, { note: n, by });
    els.get(code).classList.add('is-down');
    let set = holders.get(n);
    if (!set) holders.set(n, (set = new Set()));
    const first = set.size === 0;
    set.add(code);
    if (first) synth.on(n, state.inst);
    showReadout();
    checkAudio();
  }

  function release(code, by) {
    const h = held.get(code);
    if (!h || h.by !== by) return;
    held.delete(code);
    els.get(code).classList.remove('is-down');
    const set = holders.get(h.note);
    if (set) {
      set.delete(code);
      if (set.size === 0) {
        holders.delete(h.note);
        synth.off(h.note);
      }
    }
    showReadout();
  }

  function releaseAll(only) {
    for (const [code, h] of [...held]) if (!only || h.by === only) release(code, h.by);
  }

  // if the browser is still holding the sound back a moment after a key, say what to do
  let audioTimer = 0;
  function checkAudio() {
    clearTimeout(audioTimer);
    audioTimer = setTimeout(() => { audioNote.hidden = synth.state() !== 'suspended'; }, 450);
  }

  const typing = (t) => !!t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));

  addEventListener('keydown', (e) => {
    if (!state.listening || e.ctrlKey || e.metaKey || e.altKey || e.isComposing || typing(e.target)) return;
    if (!L.byCode.has(e.code)) return;
    e.preventDefault(); // keeps "'" and "/" from opening the quick-find bar in some browsers
    if (!e.repeat) press(e.code, 'kbd');
  });
  addEventListener('keyup', (e) => {
    if (!L.byCode.has(e.code)) return;
    const h = held.get(e.code);
    if (h && h.by === 'kbd') {
      e.preventDefault();
      release(e.code, 'kbd');
    }
  });
  addEventListener('blur', () => releaseAll());
  document.addEventListener('visibilitychange', () => { if (document.hidden) releaseAll(); });
  // a tap or a click is what lets the browser start the sound on phones
  board.addEventListener('click', () => { synth.unlock(); checkAudio(); });

  // ---- the controls ---------------------------------------------------------------------
  function change(fn) {
    releaseAll();
    synth.allOff();
    fn();
    render();
    showReadout();
  }
  document.querySelectorAll('[data-inst]').forEach((b) => b.addEventListener('click', () => change(() => { state.inst = b.dataset.inst; })));
  document.querySelectorAll('[data-names]').forEach((b) => b.addEventListener('click', () => change(() => { state.names = b.dataset.names; })));
  $('oct-down').addEventListener('click', () => change(() => { state.octave = Math.max(-3, state.octave - 1); }));
  $('oct-up').addEventListener('click', () => change(() => { state.octave = Math.min(3, state.octave + 1); }));

  function setListening(on) {
    state.listening = on;
    if (!on) releaseAll('kbd');
    listenBtn.setAttribute('aria-checked', String(on));
    listenBox.textContent = on ? '[x]' : '[ ]';
    status.textContent = on ? '● listening' : '○ off';
    status.classList.toggle('on', on);
  }
  listenBtn.addEventListener('click', () => setListening(!state.listening));

  render();
  showReadout();

  // ---- download button: say which system the visitor is on -------------------------------
  function detectOS() {
    const platform = ((navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || '').toLowerCase();
    const all = (platform + ' ' + (navigator.userAgent || '')).toLowerCase();
    if (/android|iphone|ipad|ipod/.test(all)) return null; // no keyboard to play on a phone
    if (platform.startsWith('win') || /windows/.test(all)) return 'windows';
    if (platform.startsWith('mac') || /macintosh|mac os/.test(all)) return 'macos';
    if (/linux|x11|cros/.test(all)) return 'linux';
    return null;
  }
  const os = detectOS();
  if (os) {
    const names = { linux: 'Linux', windows: 'Windows', macos: 'macOS' };
    $('dl').textContent = 'Download for ' + names[os];
    document.querySelectorAll('[data-os="' + os + '"]').forEach((row) => row.classList.add('mine'));
  }

  // ---- copy buttons -----------------------------------------------------------------------
  document.addEventListener('click', async (e) => {
    const btn = e.target.closest && e.target.closest('button.copy');
    if (!btn) return;
    const code = btn.previousElementSibling;
    const text = btn.dataset.copy || code.textContent;
    let ok = false;
    try {
      await navigator.clipboard.writeText(text);
      ok = true;
    } catch (_) { /* some viewers refuse: select the text instead */ }
    if (!ok) {
      try {
        const range = document.createRange();
        range.selectNodeContents(code);
        const sel = getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
        ok = document.execCommand('copy');
      } catch (_) { /* the text stays selected */ }
    }
    btn.textContent = ok ? 'copied' : 'selected';
    setTimeout(() => { btn.textContent = 'copy'; }, 1500);
  });
})();
