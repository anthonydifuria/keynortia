/* A small synthesizer for the demo: a plucked piano-like tone and a reedy accordion tone.
   The program itself makes no sound; this is only so the page can be played. */
const KeynortiaSynth = (() => {
  'use strict';

  const VELOCITY = 100 / 127; // the program's default velocity
  const hz = (note) => 440 * Math.pow(2, (note - 69) / 12);

  function piano(ac, dest, note, vel) {
    const t = ac.currentTime;
    const f = hz(note);
    const env = ac.createGain();
    const tone = ac.createBiquadFilter();
    tone.type = 'lowpass';
    tone.Q.value = 0.5;
    tone.frequency.setValueAtTime(Math.min(f * 9, 12000), t);
    tone.frequency.exponentialRampToValueAtTime(Math.max(f * 2.5, 500), t + 1.4);
    tone.connect(env);
    env.connect(dest);
    const peak = 0.3 * vel;
    const tau = Math.min(2.6, Math.max(0.45, 2.7 - (note - 36) * 0.04)); // low notes ring longer
    env.gain.setValueAtTime(0.0001, t);
    env.gain.linearRampToValueAtTime(peak, t + 0.006);
    env.gain.setTargetAtTime(0.0001, t + 0.006, tau);
    const oscs = [[1, 'triangle', 0.62], [2, 'sine', 0.26], [3, 'sine', 0.1], [4, 'sine', 0.05]]
      .filter(([m]) => f * m < ac.sampleRate * 0.45)
      .map(([m, type, g]) => {
        const o = ac.createOscillator();
        o.type = type;
        o.frequency.value = f * m;
        const gain = ac.createGain();
        gain.gain.value = g;
        o.connect(gain);
        gain.connect(tone);
        o.start(t);
        o.stop(t + tau * 6 + 1);
        return o;
      });
    return {
      release() {
        const n = ac.currentTime;
        env.gain.cancelScheduledValues(n);
        env.gain.setValueAtTime(Math.max(env.gain.value, 0.0001), n);
        env.gain.setTargetAtTime(0.0001, n, 0.09);
        oscs.forEach((o) => { try { o.stop(n + 0.7); } catch (_) { /* already stopped */ } });
      },
    };
  }

  function accordion(ac, dest, note, vel) {
    const t = ac.currentTime;
    const f = hz(note);
    const env = ac.createGain();
    const tone = ac.createBiquadFilter();
    tone.type = 'lowpass';
    tone.Q.value = 0.9;
    tone.frequency.value = Math.min(6000, 1100 + f * 3.2);
    tone.connect(env);
    env.connect(dest);
    const peak = 0.2 * vel;
    env.gain.setValueAtTime(0.0001, t);
    env.gain.linearRampToValueAtTime(peak, t + 0.04);
    // three reeds, two of them slightly out of tune with the first (the "wet" musette sound),
    // and a quiet one an octave below
    const reeds = [[0, 1, 'sawtooth', 0.34], [9, 1, 'sawtooth', 0.34], [-9, 1, 'sawtooth', 0.34], [0, 0.5, 'square', 0.1]]
      .filter(([, m]) => f * m >= 30 && f * m < ac.sampleRate * 0.4)
      .map(([cents, m, type, g]) => {
        const o = ac.createOscillator();
        o.type = type;
        o.frequency.value = f * m;
        o.detune.value = cents;
        const gain = ac.createGain();
        gain.gain.value = g;
        o.connect(gain);
        gain.connect(tone);
        o.start(t);
        return o;
      });
    return {
      release() {
        const n = ac.currentTime;
        env.gain.cancelScheduledValues(n);
        env.gain.setValueAtTime(Math.max(env.gain.value, 0.0001), n);
        env.gain.setTargetAtTime(0.0001, n, 0.035);
        reeds.forEach((o) => { try { o.stop(n + 0.4); } catch (_) { /* already stopped */ } });
      },
    };
  }

  const VOICES = { piano, accB: accordion, accC: accordion };

  /** The synthesizer proper. The audio context is created on the first note, from a key or a click. */
  function create() {
    let ac = null;
    let master = null;
    const playing = new Map(); // note -> voice

    function unlock() {
      if (!ac) {
        const AC = window.AudioContext || window.webkitAudioContext;
        if (!AC) return null;
        ac = new AC();
        const limiter = ac.createDynamicsCompressor();
        limiter.threshold.value = -14;
        limiter.knee.value = 12;
        limiter.ratio.value = 6;
        limiter.attack.value = 0.004;
        limiter.release.value = 0.2;
        master = ac.createGain();
        master.gain.value = 0.7;
        master.connect(limiter);
        limiter.connect(ac.destination);
      }
      if (ac.state === 'suspended') ac.resume().catch(() => {});
      return ac;
    }

    return {
      unlock,
      state: () => (ac ? ac.state : 'none'),
      on(note, inst) {
        const c = unlock();
        if (!c) return;
        const old = playing.get(note);
        if (old) old.release();
        playing.set(note, (VOICES[inst] || piano)(c, master, note, VELOCITY));
      },
      off(note) {
        const v = playing.get(note);
        if (v) {
          v.release();
          playing.delete(note);
        }
      },
      allOff() {
        playing.forEach((v) => v.release());
        playing.clear();
      },
    };
  }

  return { create, piano, accordion, VELOCITY };
})();
if (typeof module !== 'undefined') module.exports = KeynortiaSynth;
