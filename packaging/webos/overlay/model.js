/*
 * model.js: the model of the assistant overlay (docs/design/assistant-overlay.md).
 * ES5 only, no DOM, no timers, no I/O: the overlay page (window.overlayModel)
 * and the node tests (module.exports) run it unchanged.
 *
 * The daemon sends one frame per change as the overlay app's launch
 * parameters (crates/satellite/src/overlay.rs, Frame::params_json):
 *   {"overlay":1,"inst":I,"seq":N,"stage":"listening|thinking|answer|error|done|hide",
 *    "heard":"...","answer":"...","error":"no_speech|disconnected|failed"|null}
 * `inst` is fixed per daemon start (a restarted daemon counts `seq` from 1
 * again). parse() checks a frame strictly (anything else is ignored),
 * newer() orders them, same() spots a frame sent again, view() turns one
 * into what the page draws and how long it stays.
 *
 * Texts: Spanish first (the household language); English only when the TV's
 * own language is English (langOf). The heard and answer texts are shown
 * only; the page never logs them.
 */
(function (root, factory) {
  'use strict';
  var api = factory();
  if (typeof module === 'object' && module.exports) { module.exports = api; }
  if (root) { root.overlayModel = api; }
}(typeof window !== 'undefined' ? window : null, function () {
  'use strict';

  var STAGES = ['listening', 'thinking', 'answer', 'error', 'done', 'hide'];
  var ERRORS = ['no_speech', 'disconnected', 'failed'];
  var MAX_TEXT = 400; // the daemon sends at most 280 characters

  var TEXT = {
    es: {
      listening: 'Te escucho…',
      thinking: 'Pensando…',
      no_speech: 'No te he entendido',
      disconnected: 'Sin conexión con Home Assistant',
      failed: 'Algo ha fallado',
      done: 'Hecho'
    },
    en: {
      listening: 'Listening…',
      thinking: 'Thinking…',
      no_speech: 'Didn\'t catch that',
      disconnected: 'Home Assistant is not connected',
      failed: 'Something went wrong',
      done: 'Done'
    }
  };

  // How long a stage stays on screen without a newer frame. Done and error
  // are the normal ends (the auto-hide); the others are safety limits for a
  // daemon that stopped sending (the daemon closes a run itself after 90 s).
  var HOLD_MS = {
    listening: 30000,
    thinking: 45000,
    answer: 180000,
    error: 4000,
    done: 5000,
    hide: 0
  };
  // Done with nothing to read (no text heard, no reply) goes sooner.
  var DONE_EMPTY_MS = 1500;

  function isText(s) { return typeof s === 'string' && s.length <= MAX_TEXT; }

  // One frame from the launch parameters (an object, or its JSON text), or
  // null when it is not a valid overlay frame.
  function parse(raw) {
    var p = raw;
    if (typeof p === 'string') {
      if (p.length > 4096) { return null; }
      try { p = JSON.parse(p); } catch (e) { return null; }
    }
    if (!p || typeof p !== 'object' || p.overlay !== 1) { return null; }
    if (typeof p.seq !== 'number' || p.seq % 1 !== 0 || p.seq < 0 || p.seq > 4294967295) { return null; }
    if (typeof p.inst !== 'number' || p.inst % 1 !== 0 || p.inst < 0 || p.inst > 9007199254740991) { return null; }
    if (STAGES.indexOf(p.stage) < 0 || !isText(p.heard) || !isText(p.answer)) { return null; }
    var error = null;
    if (p.stage === 'error') {
      if (ERRORS.indexOf(p.error) < 0) { return null; }
      error = p.error;
    }
    return { inst: p.inst, seq: p.seq, stage: p.stage, heard: p.heard, answer: p.answer, error: error };
  }

  // Whether frame `f` replaces the one shown (`cur`, null when none): a frame
  // of another daemon start (a restart counts from 1 again), a newer run, or
  // the same run (the daemon sends a run's frames in order). A frame of an
  // older run of the same start, delivered late, is dropped. The sequence
  // wraps at 2^32.
  function newer(f, cur) {
    if (!cur || f.inst !== cur.inst) { return true; }
    if (f.seq === cur.seq) { return true; }
    var d = (f.seq - cur.seq + 4294967296) % 4294967296;
    return d > 0 && d < 2147483648;
  }

  // The same frame again (the daemon's re-send after a cold start): nothing
  // to draw, and the hold time must not start over.
  function same(f, cur) {
    return !!cur && f.inst === cur.inst && f.seq === cur.seq && f.stage === cur.stage
      && f.heard === cur.heard && f.answer === cur.answer && f.error === cur.error;
  }

  // What the page draws for frame f:
  //   visible  - show the panel
  //   status   - the status line (empty: none)
  //   heard    - the recognised text, in quotes (empty: none)
  //   answer   - the reply text (empty: none)
  //   tone     - 'listening' | 'thinking' | 'answer' | 'error' | 'done' (CSS class)
  //   holdMs   - hide after this long without a newer frame
  // The texts' language for the TV's language tag (navigator.language, for
  // example "es-ES" on c3-a): English for an English TV, Spanish otherwise.
  function langOf(tag) { return /^en(-|_|$)/i.test(String(tag || '')) ? 'en' : 'es'; }

  function view(f, lang) {
    var t = TEXT[lang === 'en' ? 'en' : 'es'];
    if (!f || f.stage === 'hide') {
      return { visible: false, status: '', heard: '', answer: '', tone: 'done', holdMs: 0 };
    }
    var v = { visible: true, status: '', heard: f.heard, answer: '', tone: f.stage, holdMs: HOLD_MS[f.stage] };
    switch (f.stage) {
    case 'listening':
      v.status = t.listening;
      v.heard = '';
      break;
    case 'thinking':
      v.status = t.thinking;
      break;
    case 'answer':
      v.answer = f.answer;
      break;
    case 'error':
      v.status = t[f.error] || t.failed;
      v.heard = f.error === 'no_speech' ? '' : f.heard;
      break;
    default: // done
      v.answer = f.answer;
      if (!f.answer && !f.heard) {
        v.status = t.done;
        v.holdMs = DONE_EMPTY_MS;
      }
    }
    return v;
  }

  return {
    parse: parse, newer: newer, same: same, view: view, langOf: langOf,
    TEXT: TEXT, HOLD_MS: HOLD_MS, DONE_EMPTY_MS: DONE_EMPTY_MS, STAGES: STAGES, ERRORS: ERRORS
  };
}));
