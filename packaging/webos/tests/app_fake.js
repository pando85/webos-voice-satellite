// Shared harness of the app host tests (app_smoke.js, app_matrix.js): a
// minimal fake DOM built from index.html, a fake Homebrew Channel exec
// (PalmServiceBridge) answered by the test, a fake clock driving setTimeout,
// setInterval and Date.now, and element rectangles that follow the 1920x1080
// layout of style.css closely enough for the nearest-in-direction focus.
// Node standard library only. Not a test by itself.
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const APP = path.join(__dirname, '..', 'app');
const APP_PATH = '/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite/index.html';
// The installed ctl.sh when there is one, else the app's own (app.js ctlCommand).
const CTL_PREFIX = 'c=/var/lib/webos-satellite/bin/ctl.sh; [ -f "$c" ] && [ -f /var/lib/webos-satellite/app-dir ] || ' +
  "c='/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite/payload/ctl.sh'; sh \"$c\" ";

// [left, top, width, height] at 1920x1080, from style.css.
const LAYOUT = {
  'h-notice-btn': [1560, 480, 260, 76],
  'h-mic': [96, 594, 320, 284], 'h-voice': [448, 594, 320, 284], 'h-sound': [800, 594, 320, 284], 'h-ha': [1152, 594, 320, 284], 'h-diag': [1504, 594, 320, 284],
  's-row-aec': [96, 156, 1060, 150], 's-row-music': [96, 328, 1060, 150], 's-row-mic': [96, 500, 1060, 150], 's-row-overlay': [96, 672, 1060, 150],
  'v-row-ar': [96, 156, 1060, 150], 'v-row-hl': [96, 328, 1060, 150], 'v-row-keep': [96, 500, 1060, 150], 'v-refresh': [96, 676, 220, 94], 'v-hint-btn': [1212, 640, 380, 84],
  'a-pair': [1184, 156, 640, 130], 'a-phone': [1184, 308, 640, 130], 'a-reset': [1184, 460, 640, 130], 'a-endphone': [1184, 612, 640, 130],
  'q-done': [768, 820, 170, 94], 'q-again': [962, 820, 270, 94],
  'd-check': [1184, 156, 640, 130], 'd-report': [1184, 308, 640, 130], 'd-ack': [1184, 460, 640, 130], 'd-more': [1184, 612, 640, 130],
  'r-back': [768, 860, 170, 94],
  'x-stop': [96, 156, 760, 110], 'x-start': [96, 288, 760, 110], 'x-boot-on': [96, 420, 760, 110], 'x-boot-off': [96, 552, 760, 110],
  'x-crash': [96, 684, 760, 110], 'x-uninstall': [96, 816, 760, 110], 'x-purge': [96, 948, 760, 110],
  'm-allow': [424, 700, 200, 94], 'm-refuse': [648, 700, 220, 94],
};

function load(opts) {
  const o = opts || {};
  const clock = { now: 1_000_000 };
  const timers = []; // { f, at, ms, live, interval }
  const calls = [];
  const byId = {};
  const doc = {
    activeElement: null, keydown: null,
    getElementById: (id) => byId[id],
    createElement: (t) => el('', t),
    addEventListener(t, f) { if (t === 'keydown') this.keydown = f; },
  };
  function el(id, tag) {
    return {
      id, tagName: tag, hidden: false, className: '', textContent: '', attrs: {}, listeners: {}, style: {}, ops: [], scrollTop: 0,
      width: 0, height: 0,
      getAttribute(n) { return n in this.attrs ? this.attrs[n] : null; },
      setAttribute(n, v) { this.attrs[n] = String(v); },
      addEventListener(t, f) { this.listeners[t] = f; },
      focus() { doc.activeElement = this; },
      getBoundingClientRect() {
        const r = LAYOUT[this.id] || [0, 0, 10, 10];
        return { left: r[0], top: r[1], width: r[2], height: r[3], right: r[0] + r[2], bottom: r[1] + r[3] };
      },
      getContext() {
        const ops = this.ops;
        return { set fillStyle(v) { this.fs = v; }, fillRect(x, y, w, h) { ops.push({ x, y, w, h, fs: this.fs }); } };
      },
    };
  }
  doc.body = el('body', 'body');
  doc.activeElement = doc.body;
  const html = fs.readFileSync(path.join(APP, 'index.html'), 'utf8');
  for (const m of html.matchAll(/<(\w+) id="([^"]+)"([^>]*)>([^<]*)/g)) {
    const e = el(m[2], m[1]);
    for (const a of m[3].matchAll(/([\w-]+)="([^"]*)"/g)) e.attrs[a[1]] = a[2];
    e.hidden = /(^|\s)hidden(\s|$)/.test(m[3]);
    e.className = e.attrs.class || '';
    e.textContent = m[4];
    byId[m[2]] = e;
  }

  // The fake ctl.sh: o.exec(args) returns stdout (a string) or a reply object.
  class Bridge {
    call(uri, params) {
      const p = JSON.parse(params);
      calls.push({ uri, p, at: clock.now });
      let reply;
      if (uri.endsWith('/exec')) {
        const args = p.command.startsWith(CTL_PREFIX) ? p.command.slice(CTL_PREFIX.length).replace(/ 2>&1$/, '') : p.command;
        const out = o.exec ? o.exec(args) : 'webos-satellite: done\n';
        reply = typeof out === 'string' ? { returnValue: true, stdoutString: out } : out;
      } else {
        reply = o.conn || { returnValue: true, wired: { state: 'connected', ipAddress: '192.0.2.10' } };
      }
      setImmediate(() => this.onservicecallback(JSON.stringify(reply)));
    }
  }
  const window = { location: { pathname: APP_PATH }, PalmServiceBridge: Bridge, innerWidth: 1920, innerHeight: 1080, addEventListener() {} };
  const qrCtx = vm.createContext({});
  vm.runInContext(fs.readFileSync(path.join(APP, 'qrcodegen.js'), 'utf8'), qrCtx);
  window.qrcodegen = vm.runInContext('qrcodegen', qrCtx);
  const sandbox = {
    window, document: doc, JSON, Math, decodeURIComponent, String, Object, Array, Infinity,
    Date: { now: () => clock.now },
    setTimeout: (f, ms) => { const t = { f, at: clock.now + (ms || 0), ms, live: true }; timers.push(t); return t; },
    setInterval: (f, ms) => { const t = { f, at: clock.now + ms, ms, live: true, interval: true }; timers.push(t); return t; },
  };
  vm.runInNewContext(fs.readFileSync(path.join(APP, 'voicepower.js'), 'utf8'), sandbox);
  vm.runInNewContext(fs.readFileSync(path.join(APP, 'app.js'), 'utf8'), sandbox);

  const settle = async (n) => { for (let i = 0; i < (n || 8); i++) await new Promise((r) => setImmediate(r)); };
  // Moves the clock forward, running due timers in order (replies of the
  // fake exec arrive between steps of `step`).
  function advance(ms) {
    const end = clock.now + ms;
    for (;;) {
      const due = timers.filter((t) => t.live && t.at <= end).sort((a, b) => a.at - b.at)[0];
      if (!due) break;
      clock.now = Math.max(clock.now, due.at);
      if (due.interval) due.at += due.ms; else due.live = false;
      due.f();
    }
    clock.now = end;
  }
  async function step(ms, each) {
    const n = Math.max(1, Math.round(ms / (each || 1000)));
    for (let i = 0; i < n; i++) { advance(ms / n); await settle(); }
  }
  const execs = () => calls.filter((c) => c.uri.endsWith('/exec')).map((c) => c.p.command.startsWith(CTL_PREFIX)
    ? c.p.command.slice(CTL_PREFIX.length).replace(/ 2>&1$/, '') : c.p.command);
  const key = (k) => { let prevented = false; doc.keydown({ keyCode: k, preventDefault() { prevented = true; } }); return prevented; };
  const click = (id) => byId[id].listeners.click();
  // OK on the remote: the browser turns Enter on a focused button into a click.
  const ok = () => { key(13); doc.activeElement.listeners.click(); };
  const visible = (ids) => ids.filter((id) => !byId[id].hidden);
  const screen = () => ['home', 'voice', 'sound', 'ha', 'setup', 'diag', 'report', 'more'].filter((s) => !byId[s].hidden);
  return { byId, doc, window, calls, timers, clock, settle, advance, step, execs, key, click, ok, visible, screen,
    VP: window.voicePower, app: window.voiceSatelliteApp, CTL_PREFIX };
}

function checker() {
  let fails = 0;
  return {
    check(name, ok, extra) { console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${ok || !extra ? '' : ` (${extra})`}`); if (!ok) fails += 1; },
    done() { if (fails) { console.log(`${fails} check(s) failed`); process.exit(1); } console.log('all checks passed'); },
  };
}

module.exports = { load, checker, APP, LAYOUT, CTL_PREFIX };
