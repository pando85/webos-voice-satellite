// Host test of the assistant overlay app (packaging/webos/overlay/; design
// docs/design/assistant-overlay.md). Part 1 checks the model (model.js):
// strict frame parsing, run order with the sequence wrap, what each stage
// shows in Spanish and English, the hold times. Part 2 runs the page
// (overlay.js) in a fake DOM with a fake webOSSystem and a fake clock: the
// first frame from the launch parameters, later ones from webOSRelaunch, the
// empty input region (array, then string), the auto-hide and window.close,
// a frame during the fade, a key that reaches the window, stale and invalid
// frames. Part 3 checks the rules on the source: ES5 only, no sound,
// network, storage or log. Node standard library only.
// usage: node packaging/webos/tests/overlay.js
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { checker } = require('./app_fake.js');

const DIR = path.join(__dirname, '..', 'overlay');
const M = require(path.join(DIR, 'model.js'));
const c = checker();
const done = c.done;
// Each check is a function, run here (an exception is a failure).
function check(name, f) {
  let ok = false;
  let why = '';
  try { ok = !!f(); } catch (e) { why = String(e && e.stack || e).split('\n').slice(0, 2).join(' '); }
  c.check(name, ok, why);
}

const fr = (o) => Object.assign({ overlay: 1, inst: 1762000000000, seq: 1, stage: 'listening', heard: '', answer: '', error: null }, o);

// ---- Part 1: the model ----------------------------------------------------
check('parse: a frame object and its JSON text', () => {
  const f = M.parse(fr({ stage: 'thinking', heard: 'hola' }));
  const g = M.parse(JSON.stringify(fr({ stage: 'thinking', heard: 'hola' })));
  return f && g && f.stage === 'thinking' && f.heard === 'hola' && f.error === null && JSON.stringify(f) === JSON.stringify(g);
});
check('parse: an error frame needs a known error word', () =>
  M.parse(fr({ stage: 'error', error: 'no_speech' })).error === 'no_speech'
  && M.parse(fr({ stage: 'error', error: 'boom' })) === null
  && M.parse(fr({ stage: 'error', error: null })) === null);
check('parse: anything else is refused', () => [
  null, undefined, '', '{', '[]', 'x', 42, {}, fr({ overlay: 2 }), fr({ overlay: '1' }), fr({ seq: -1 }), fr({ seq: 1.5 }),
  fr({ seq: '1' }), fr({ seq: 4294967296 }), fr({ stage: 'speaking' }), fr({ heard: 3 }), fr({ answer: null }),
  fr({ heard: 'x'.repeat(401) }), 'y'.repeat(5000), fr({ inst: undefined }), fr({ inst: -1 }), fr({ inst: 1.5 }), fr({ inst: '7' }),
].every((p) => M.parse(p) === null));
check('parse: the texts may hold quotes, accents and up to 400 characters', () => {
  const f = M.parse(fr({ stage: 'answer', answer: 'Sí, "vale" ñ ' + 'x'.repeat(380) }));
  return f && f.answer.length === 393;
});
check('newer: same run, later run, older run, across the wrap', () => {
  const a = M.parse(fr({ seq: 5 }));
  const ok = (s, cur) => M.newer(M.parse(fr({ seq: s })), cur);
  return ok(5, null) && ok(5, a) && ok(6, a) && !ok(4, a)
    && ok(0, M.parse(fr({ seq: 4294967295 }))) && !ok(4294967295, M.parse(fr({ seq: 0 })));
});
check('newer: a frame of another daemon start is a new run, whatever its seq (a restart counts from 1 again)', () => {
  const cur = M.parse(fr({ seq: 9, stage: 'answer' }));
  return M.newer(M.parse(fr({ inst: 1762000999999, seq: 1, stage: 'hide' })), cur)
    && M.newer(M.parse(fr({ inst: 5, seq: 1 })), cur) && !M.newer(M.parse(fr({ seq: 8 })), cur);
});
check('same: only an identical frame of the same start', () => {
  const a = M.parse(fr({ stage: 'thinking', heard: 'x' }));
  return M.same(M.parse(fr({ stage: 'thinking', heard: 'x' })), a) && !M.same(M.parse(fr({ stage: 'thinking', heard: 'y' })), a)
    && !M.same(M.parse(fr({ stage: 'thinking', heard: 'x', inst: 2 })), a) && !M.same(a, null);
});
check('langOf: English only for an English TV', () =>
  M.langOf('en-US') === 'en' && M.langOf('en') === 'en' && M.langOf('EN_gb') === 'en'
  && M.langOf('es-ES') === 'es' && M.langOf('de-DE') === 'es' && M.langOf('') === 'es' && M.langOf(undefined) === 'es'
  && M.langOf('eng') === 'es');
const v = (o, lang) => M.view(M.parse(fr(o)), lang);
check('view es: listening, thinking with what was heard, answer, done', () => {
  const l = v({ stage: 'listening', heard: 'ignored' });
  const t = v({ stage: 'thinking', heard: 'enciende la luz' });
  const a = v({ stage: 'answer', heard: 'enciende la luz', answer: 'Hecho, encendida.' });
  const d = v({ stage: 'done', heard: 'enciende la luz', answer: 'Hecho, encendida.' });
  return l.visible && l.status === 'Te escucho…' && l.heard === '' && l.tone === 'listening' && l.holdMs === 30000
    && t.status === 'Pensando…' && t.heard === 'enciende la luz' && t.answer === ''
    && a.status === '' && a.heard === 'enciende la luz' && a.answer === 'Hecho, encendida.' && a.holdMs === 180000
    && d.answer === 'Hecho, encendida.' && d.status === '' && d.holdMs === 5000 && d.tone === 'done';
});
check('view es: the errors', () =>
  v({ stage: 'error', error: 'no_speech', heard: 'x' }).status === 'No te he entendido'
  && v({ stage: 'error', error: 'no_speech', heard: 'x' }).heard === ''
  && v({ stage: 'error', error: 'disconnected', heard: 'abre' }).status === 'Sin conexión con Home Assistant'
  && v({ stage: 'error', error: 'disconnected', heard: 'abre' }).heard === 'abre'
  && v({ stage: 'error', error: 'failed' }).status === 'Algo ha fallado'
  && v({ stage: 'error', error: 'failed' }).holdMs === 4000 && v({ stage: 'error', error: 'failed' }).tone === 'error');
check('view en: the same stages in English', () =>
  v({ stage: 'listening' }, 'en').status === 'Listening…' && v({ stage: 'thinking' }, 'en').status === 'Thinking…'
  && v({ stage: 'error', error: 'no_speech' }, 'en').status === 'Didn\'t catch that'
  && v({ stage: 'done' }, 'en').status === 'Done');
check('view: done with nothing to read goes sooner; hide hides at once', () => {
  const d = v({ stage: 'done' });
  const h = v({ stage: 'hide', heard: 'x', answer: 'y' });
  return d.status === 'Hecho' && d.holdMs === M.DONE_EMPTY_MS && !h.visible && h.holdMs === 0 && !M.view(null).visible;
});

// ---- Part 2: the page -----------------------------------------------------
function page(opts) {
  const o = opts || {};
  const clock = { now: 0 };
  const timers = [];
  const byId = {};
  const listeners = {};
  const sysCalls = [];
  let closed = 0;
  const html = fs.readFileSync(path.join(DIR, 'index.html'), 'utf8');
  for (const m of html.matchAll(/<(\w+) id="([^"]+)"([^>]*)>/g)) {
    byId[m[2]] = { id: m[2], hidden: /(^|\s)hidden(\s|$)/.test(m[3]), className: (m[3].match(/class="([^"]*)"/) || [])[1] || '', textContent: '', style: {} };
  }
  const sys = {
    launchParams: o.params === undefined ? JSON.stringify(fr()) : o.params,
    setInputRegion(r) { sysCalls.push(['setInputRegion', r]); if (o.arrayThrows && typeof r !== 'string') { throw new TypeError('wants a string'); } },
    setKeyMask(k) { sysCalls.push(['setKeyMask', k]); },
  };
  const win = {
    innerWidth: o.width || 1920,
    webOSSystem: sys,
    addEventListener() {},
    close() { closed++; },
  };
  const doc = {
    getElementById: (id) => byId[id],
    addEventListener(t, f) { listeners[t] = f; },
  };
  const ctx = {
    window: win, document: doc, navigator: { language: o.lang || 'es-ES' }, JSON, Math, String, RegExp,
    setTimeout(f, ms) { const t = { f, at: clock.now + (ms || 0), live: true }; timers.push(t); return t; },
    clearTimeout(t) { if (t) { t.live = false; } },
    setInterval(f, ms) { const t = { f, at: clock.now + ms, ms, live: true, interval: true }; timers.push(t); return t; },
    clearInterval(t) { if (t) { t.live = false; } },
  };
  win.overlayModel = M;
  vm.createContext(ctx);
  vm.runInContext(fs.readFileSync(path.join(DIR, 'overlay.js'), 'utf8'), ctx);
  function advance(ms) {
    const end = clock.now + ms;
    for (;;) {
      const due = timers.filter((t) => t.live && t.at <= end).sort((a, b) => a.at - b.at)[0];
      if (!due) { break; }
      clock.now = due.at;
      if (due.interval) { due.at += due.ms; } else { due.live = false; }
      due.f();
    }
    clock.now = end;
  }
  return {
    byId, sysCalls, advance,
    closed: () => closed,
    relaunch(p, viaDetail) {
      sys.launchParams = JSON.stringify(p);
      listeners.webOSRelaunch(viaDetail ? { detail: p } : {});
    },
    key(code) { let prevented = false; listeners.keydown({ keyCode: code, preventDefault() { prevented = true; } }); return prevented; },
    panel: () => byId.panel,
    tickers: () => timers.filter((t) => t.live && t.interval),
    bars: () => [0, 1, 2, 3, 4].map((i) => byId['b' + i].style.transform || '').join('|'),
    dots: () => [0, 1, 2].map((i) => byId['d' + i].className).join('|'),
    shown: (id) => !byId[id].hidden && byId[id].textContent,
  };
}

check('page: the first frame comes with the launch; the input region is empty and keys are unmasked', () => {
  const p = page();
  p.advance(20);
  const r = p.sysCalls.find((c) => c[0] === 'setInputRegion');
  return !p.panel().hidden && / in$/.test(p.panel().className) && /t-listening/.test(p.panel().className)
    && p.shown('status') === 'Te escucho…' && !p.shown('heard') && !p.shown('answer')
    && JSON.stringify(r[1]) === '[{"x":0,"y":0,"width":0,"height":0}]'
    && p.sysCalls.some((c) => c[0] === 'setKeyMask' && Array.isArray(c[1]) && c[1].length === 0) && p.closed() === 0;
});
check('page: bars move in 200 ms steps while listening, dots while thinking, nothing moves with the answer or after close', () => {
  const p = page();
  const t1 = p.tickers();
  const b1 = p.bars();
  p.advance(200);
  const b2 = p.bars();
  const listening = t1.length === 1 && t1[0].ms === 200 && /scaleY\(/.test(b1) && b1 !== b2;
  p.relaunch(fr({ stage: 'thinking', heard: 'x' }));
  const d1 = p.dots();
  p.advance(200);
  const thinking = p.tickers().length === 1 && d1 !== p.dots() && p.dots().split('|').filter((c) => c === 'on').length === 1;
  p.relaunch(fr({ stage: 'answer', answer: 'y' }));
  const still = p.tickers().length === 0;
  p.relaunch(fr({ stage: 'listening', seq: 2 }));
  const again = p.tickers().length === 1;
  p.key(13);
  return listening && thinking && still && again && p.tickers().length === 0;
});
check('page: a runtime that wants the region as text gets the JSON string', () => {
  const p = page({ arrayThrows: true });
  const rs = p.sysCalls.filter((c) => c[0] === 'setInputRegion');
  return rs.length === 2 && rs[1][1] === '[{"x":0,"y":0,"width":0,"height":0}]';
});
check('page: relaunches update the run (event detail or launchParams), done hides after 5 s and closes the window', () => {
  const p = page();
  p.advance(20);
  p.relaunch(fr({ stage: 'thinking', heard: 'qué hora es' }));
  const think = p.shown('status') === 'Pensando…' && p.shown('heard') === '«qué hora es»';
  p.relaunch(fr({ stage: 'answer', heard: 'qué hora es', answer: 'Son las diez.' }), true);
  const ans = p.shown('answer') === 'Son las diez.' && !p.shown('status') && /t-answer/.test(p.panel().className);
  p.relaunch(fr({ stage: 'done', heard: 'qué hora es', answer: 'Son las diez.' }));
  p.advance(4900);
  const still = p.closed() === 0 && !/out/.test(p.panel().className);
  p.advance(100);
  const fading = /out/.test(p.panel().className) && p.closed() === 0;
  p.advance(400);
  return think && ans && still && fading && p.closed() === 1;
});
check('page: a new run during the fade keeps the window', () => {
  const p = page();
  p.relaunch(fr({ stage: 'error', error: 'no_speech' }));
  p.advance(4000 + 100);
  const fading = /out/.test(p.panel().className);
  p.relaunch(fr({ seq: 2, stage: 'listening' }));
  p.advance(1000);
  return fading && p.closed() === 0 && p.shown('status') === 'Te escucho…' && !/out/.test(p.panel().className);
});
check('page: hide closes at once; a frame of an older run and an invalid one are ignored', () => {
  const p = page({ params: JSON.stringify(fr({ seq: 7, stage: 'thinking', heard: 'a' })) });
  p.relaunch(fr({ seq: 6, stage: 'answer', answer: 'old' }));
  p.relaunch({ overlay: 1, seq: 7, stage: 'bogus' });
  const kept = p.shown('heard') === '«a»' && !p.shown('answer');
  p.relaunch(fr({ seq: 7, stage: 'hide' }));
  p.advance(400);
  return kept && p.closed() === 1;
});
check('page: a frame sent again does not restart the hold time', () => {
  const p = page({ params: JSON.stringify(fr({ stage: 'answer', answer: 'larga' })) });
  p.advance(170000);
  p.relaunch(fr({ stage: 'answer', answer: 'larga' }));
  p.advance(10000 + 400);
  return p.closed() === 1;
});
check('page: after a daemon restart the new start\'s hide closes the stale panel at once', () => {
  const p = page({ params: JSON.stringify(fr({ seq: 12, stage: 'answer', answer: 'vieja' })) });
  p.relaunch(fr({ inst: 1762000555555, seq: 1, stage: 'hide' }));
  p.advance(400);
  return p.closed() === 1;
});
check('page: without a valid frame at launch the window closes', () => {
  const p = page({ params: 'not json' });
  p.advance(400);
  return p.closed() === 1 && p.panel().hidden;
});
check('page: a key that reaches the window closes it at once (the app in front gets the next one)', () => {
  const p = page();
  p.advance(20);
  const prevented = p.key(13);
  p.advance(400);
  return prevented && p.closed() === 1;
});
check('page: a listening run left alone closes after its safety limit', () => {
  const p = page();
  p.advance(29000);
  const open = p.closed() === 0;
  p.advance(1500);
  return open && p.closed() === 1;
});
check('page: English TV, English texts; a 1280-wide window scales the stage', () => {
  const p = page({ lang: 'en-GB', width: 1280 });
  return p.shown('status') === 'Listening…' && /scale\(0\.666/.test(p.byId.stage.style.transform);
});

// ---- Part 3: rules on the source ------------------------------------------
const src = ['model.js', 'overlay.js'].map((f) => fs.readFileSync(path.join(DIR, f), 'utf8'));
const code = src.map((s) => s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"])\/\/.*$/gm, '$1')).join('\n');
check('ES5 only (no arrows, let, const, classes, templates, spread)', () =>
  !/=>|\blet\s|\bconst\s|\bclass\s|`|\.\.\./.test(code));
check('no sound, network, storage or log in the page', () =>
  !/\bAudio\b|<audio|<video|XMLHttpRequest|fetch\(|WebSocket|EventSource|localStorage|sessionStorage|indexedDB|console\.|PalmServiceBridge/.test(code + fs.readFileSync(path.join(DIR, 'index.html'), 'utf8')));
check('index.html loads the model before the page script, and no other script file', () => {
  const html = fs.readFileSync(path.join(DIR, 'index.html'), 'utf8');
  const scripts = [...html.matchAll(/<script src="([^"]+)"/g)].map((m) => m[1]);
  return JSON.stringify(scripts) === '["model.js","overlay.js"]' && /<html lang="es">/.test(html);
});
const inline = (fs.readFileSync(path.join(DIR, 'index.html'), 'utf8').match(/<script>([\s\S]*?)<\/script>/) || [])[1] || '';
function runInline(ready) {
  let closed = 0;
  const timers = [];
  const win = { close() { closed++; } };
  const ctx = { window: win, setTimeout(f, ms) { timers.push({ f, ms }); return timers.length; } };
  vm.createContext(ctx);
  vm.runInContext(inline, ctx);
  if (ready) { win.overlayReady = true; }
  timers.forEach((t) => t.f());
  return { closed, ms: timers.map((t) => t.ms) };
}
check('index.html fallback: the window closes after 4 s when the scripts never started, not when they did', () => {
  const failed = runInline(false);
  const fine = runInline(true);
  return failed.closed === 1 && JSON.stringify(failed.ms) === '[4000]' && fine.closed === 0
    && !/=>|\blet\s|\bconst\s|`/.test(inline);
});
check('the page clears the fallback once it runs', () => {
  const src = fs.readFileSync(path.join(DIR, 'overlay.js'), 'utf8');
  return /window\.overlayReady = true;/.test(src) && /clearTimeout\(window\.overlayFallback\)/.test(src);
});
check('the stylesheet keeps the page transparent and has no CSS animation (60 fps cost a third of a core, EXP-024)', () => {
  const css = fs.readFileSync(path.join(DIR, 'overlay.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  return /html, body \{[^}]*background: transparent/.test(css) && /pointer-events: none/.test(css)
    && !/animation|@keyframes/.test(css);
});

done();
