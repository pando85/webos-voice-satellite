// Host test of the "Voice and power" screen of the status app
// (packaging/webos/app/app.js, design: docs/design/voice-and-power-screen.md,
// ADR 0008). Part 1 checks the outcome matrix (the sentence for the remote's
// power button and the microphone line) against literal strings for every
// cell and against an independently written oracle for every combination of
// Always Ready, Hi LG, Keep listening, microphone state, kill switch and
// daemon state. Part 2 drives the screen with a fake DOM and a fake
// Homebrew Channel: focus order, Back, the two-press rule (and one press to
// turn Keep listening off), arming timeout and disarm keys, pending rows, the
// result lines (applied, already, failed, refused, mismatch), the delayed
// re-read that catches a revert, and that only whitelisted commands are sent.
// usage: node packaging/webos/tests/app_matrix.js
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const APP = path.join(__dirname, '..', 'app');
const html = fs.readFileSync(path.join(APP, 'index.html'), 'utf8');
const appJs = fs.readFileSync(path.join(APP, 'app.js'), 'utf8');

let fails = 0;
function check(name, ok, extra) { console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${ok || !extra ? '' : ` (${extra})`}`); if (!ok) fails += 1; }
const tick = () => new Promise((r) => setImmediate(r));

// ---- fake DOM and Homebrew Channel -----------------------------------------
function el(id, tag) {
  return {
    id, tagName: tag, hidden: false, className: '', textContent: '', children: [], attrs: {}, listeners: {},
    get firstChild() { return this.children[0] || null; },
    appendChild(c) { this.children.push(c); return c; },
    removeChild(c) { this.children.splice(this.children.indexOf(c), 1); },
    getAttribute(n) { return n in this.attrs ? this.attrs[n] : null; },
    setAttribute(n, v) { this.attrs[n] = String(v); },
    addEventListener(t, f) { this.listeners[t] = f; },
    focus() { doc.activeElement = this; },
    getElementsByTagName() { return this.children; },
    scrollTop: 0, style: {}, ops: [],
    getContext() { return { fillRect() {} }; },
  };
}
const byId = {};
for (const m of html.matchAll(/<(\w+) id="([^"]+)"([^>]*)>([^<]*)/g)) {
  const e = el(m[2], m[1]);
  for (const a of m[3].matchAll(/([\w-]+)="([^"]*)"/g)) e.attrs[a[1]] = a[2];
  e.textContent = m[4];
  byId[m[2]] = e;
}
byId.actions.children = Object.values(byId).filter((e) => e.tagName === 'button' && e.id.startsWith('b-'));
const doc = {
  body: el('body', 'body'), activeElement: null, keydown: null,
  getElementById: (id) => byId[id], createElement: (t) => el('', t),
  addEventListener(t, f) { this.keydown = f; },
};
doc.activeElement = doc.body;

const calls = [];
const STATUS = { uid: 0, installed: true, installed_version: '0.2.3', package_version: '0.2.3', update_pending: false, running: true,
  pid: 42, supervised: true, autostart: true, disabled: false, crashloop: false, mic_off: false, firmware: 'firmware 03.30.74',
  firmware_ok: true, key: 'paired', name: 'webos-satellite-test', port: '6053', prefix: '/var/lib/webos-satellite',
  machine: 'aarch64', build: '', log: [], remote_off: 'normal' };
// What the fake ctl.sh answers: `tv` is the tv-settings reply, `setReply(cmd)` the reply of a set command.
const fake = { tv: null, setReply: null, tvFails: false };
class Bridge {
  call(uri, params) {
    const p = JSON.parse(params);
    calls.push({ uri, p });
    let reply;
    if (uri.endsWith('/exec')) {
      let out = 'webos-satellite: done\n';
      if (/ status --json /.test(p.command)) { out = JSON.stringify(STATUS) + '\n'; }
      else if (/ tv-settings --json /.test(p.command)) { out = fake.tvFails ? 'ctl.sh: broken\n' : JSON.stringify(fake.tv) + '\n'; }
      else if (/ set /.test(p.command)) { out = fake.setReply(p.command.replace(/^.*ctl\.sh' /, '').replace(/ 2>&1$/, '')); }
      reply = { returnValue: true, stdoutString: out };
    } else {
      reply = { returnValue: true, wired: { state: 'connected', ipAddress: '192.0.2.10' } };
    }
    setImmediate(() => this.onservicecallback(JSON.stringify(reply)));
  }
}
const window = {
  location: { pathname: '/media/developer/apps/usr/palm/applications/io.github.pando85.voicesatellite/index.html' },
  PalmServiceBridge: Bridge,
};
const timers = []; // { f, ms, live }
const intervals = [];
const qrCtx = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(APP, 'qrcodegen.js'), 'utf8'), qrCtx);
window.qrcodegen = vm.runInContext('qrcodegen', qrCtx);
vm.runInNewContext(appJs, {
  window, document: doc, JSON, Object, Array, String,
  setTimeout: (f, ms) => { const t = { f, ms, live: true }; timers.push(t); return t; },
  setInterval: (f, ms) => { intervals.push({ f, ms }); return 0; },
  decodeURIComponent,
});
const VP = window.voicePower;

// ---- Part 1: the matrix ----------------------------------------------------
const P_DEEP = 'Power button: the TV goes to deep standby. The satellite cannot hear until the TV is turned on.';
const P_STBY = 'Power button: the TV goes to standby. The satellite still hears you, but you will not hear its replies.';
const P_KEEP = 'Power button: the screen goes dark but the TV stays on. The satellite hears you and replies. Press power again to turn the picture on.';
const P_X = 'Power button: not known. The TV\'s settings could not be read. Press Refresh.';
const L = {
  ok: 'Listening now: yes.',
  hilg: 'Listening now: no. The satellite cannot hear: LG\'s voice assistant has the microphone. Turn Hi LG off to use the satellite.',
  busy: 'Listening now: no. Another program has the microphone.',
  muted: 'Listening now: no. The microphone is muted in Home Assistant.',
  kill: 'Listening now: no. The microphone is off (kill switch).',
  stopped: 'Listening now: no. The satellite is not running.',
  closed: 'Listening now: no. The microphone is closed.',
  unknown: 'Listening now: not known.',
};
const N_LISTEN = 'Hi LG is on but this satellite has the microphone now; after a restart LG may take it. Turn Hi LG off to be sure.';
const N_ON = 'Hi LG is on. If the satellite cannot hear you, turn it off.';
const N_OTHERS = 'Alexa/other hands-free is on: turn it off in LG\'s menu.';
const N_X = 'Always Ready or Hi LG could not be read, so the sentence above may be wrong.';
const X_KILL = 'Keep listening does nothing while the microphone is off with the kill switch.';
const X_WATCH = 'Keep listening needs the power watch, which is off in the config.';

// ctl.sh tv-settings: state off for `off`, on for `alwaysReady`, unknown for any other word (none, allEnabled, lifeOnScreen).
const arOf = (raw) => raw === 'error' ? { state: 'error', raw: '', error: 'timeout' }
  : { state: raw === 'off' ? 'off' : (raw === 'alwaysReady' ? 'on' : 'unknown'), raw, error: '' };
// The shape of ctl.sh tv-settings: state is `on` when ANY provider is on; the flags say which.
const hlOf = (st, others) => (st === 'error' || st === 'unknown')
  ? { state: st, thinq: null, amazon: others ? true : null, dangbei: null, error: st === 'error' ? 'timeout' : '' }
  : { state: st === 'on' || others ? 'on' : 'off', thinq: st === 'on', amazon: !!others, dangbei: false, error: '' };
const state = (o) => ({
  schema: 1, luna: true, daemon_running: o.running !== false,
  always_ready: arOf(o.ar || 'off'), hi_lg: hlOf(o.hl || 'off', o.others),
  remote_off: { state: o.ro || 'normal', active: o.active === undefined ? true : o.active },
  mic: { state: o.mic || 'listening', kill_switch: !!o.kill },
  power_state: 'Active', last_change: null,
});
const text = (o) => `${o.headline} ${o.detail}`;

// Literal cells (the strings are the contract).
check('P1 off + normal', text(VP.outcome(state({ ar: 'off', ro: 'normal' }))) === P_DEEP);
check('P2 alwaysReady + normal', text(VP.outcome(state({ ar: 'alwaysReady', ro: 'normal' }))) === P_STBY);
check('P3 off + keep', text(VP.outcome(state({ ar: 'off', ro: 'keep_listening' }))) === P_KEEP);
check('P4 alwaysReady + keep', text(VP.outcome(state({ ar: 'alwaysReady', ro: 'keep_listening' }))) === P_KEEP);
check('allEnabled behaves as on (P2, P4)', text(VP.outcome(state({ ar: 'allEnabled', ro: 'normal' }))) === P_STBY
  && text(VP.outcome(state({ ar: 'allEnabled', ro: 'keep_listening' }))) === P_KEEP);
check('none behaves as off (P1)', text(VP.outcome(state({ ar: 'none', ro: 'normal' }))) === P_DEEP);
check('lifeOnScreen, an unread or unknown Always Ready: not known', ['lifeOnScreen', 'error', 'weird'].every((a) => text(VP.outcome(state({ ar: a }))) === P_X));
check('Keep listening unreadable: not known', text(VP.outcome(Object.assign(state({}), { remote_off: { state: 'maybe' } }))) === P_X);
check('kill switch: keep treated as normal, with the explanation',
  text(VP.outcome(state({ ar: 'off', ro: 'keep_listening', kill: true }))) === P_DEEP
  && VP.outcome(state({ ar: 'off', ro: 'keep_listening', kill: true })).extra === X_KILL
  && VP.outcome(state({ ar: 'alwaysReady', ro: 'keep_listening', kill: true })).cell === 'P2'
  && VP.outcome(state({ ar: 'off', ro: 'normal', kill: true })).extra === '');
check('power watch off: keep treated as normal, with the explanation',
  VP.outcome(state({ ar: 'off', ro: 'keep_listening', active: false })).cell === 'P1'
  && VP.outcome(state({ ar: 'off', ro: 'keep_listening', active: false })).extra === X_WATCH);
const listen = (o) => VP.outcome(state(o)).listening;
check('listening: ok', listen({ mic: 'listening' }) === L.ok);
check('listening: busy with Hi LG on is the LG line (first opener: LG)', listen({ mic: 'busy', hl: 'on' }) === L.hilg);
check('listening: busy with Hi LG off is another program', listen({ mic: 'busy', hl: 'off' }) === L.busy);
check('listening: muted, kill switch, stopped, closed, unknown', listen({ mic: 'muted' }) === L.muted && listen({ kill: true }) === L.kill
  && listen({ running: false }) === L.stopped && listen({ mic: 'closed' }) === L.closed && listen({ mic: 'unknown' }) === L.unknown);
check('listening: stopped beats kill, kill beats muted, muted beats busy',
  listen({ running: false, kill: true }) === L.stopped && listen({ kill: true, mic: 'muted' }) === L.kill && listen({ mic: 'muted', hl: 'on' }) === L.muted);
const notes = (o) => VP.outcome(state(o)).notes;
check('Hi LG on while the satellite listens: listening line plus the restart reminder',
  listen({ hl: 'on', mic: 'listening' }) === L.ok && notes({ hl: 'on', mic: 'listening' }).join('|') === N_LISTEN);
check('Hi LG on, microphone closed or unknown: generic note, no claim that LG has it',
  notes({ hl: 'on', mic: 'closed' }).join('|') === N_ON && notes({ hl: 'on', mic: 'unknown' }).join('|') === N_ON);
check('Hi LG on, busy: no reminder (the line already says it)', notes({ hl: 'on', mic: 'busy' }).length === 0);
check('other hands-free on: its own note', notes({ others: true }).join('|') === N_OTHERS
  && notes({ hl: 'on', mic: 'listening', others: true }).join('|') === N_LISTEN + '|' + N_OTHERS);
check('Hi LG is thinQ only: state on from Alexa alone is Hi LG off plus the note; dangbei counts as other',
  VP.rows(state({ hl: 'off', others: true }))[1].word === 'Off' && VP.rows(state({ hl: 'off', others: true }))[1].cmd === 'set hi-lg on'
  && VP.outcome(Object.assign(state({}), { hi_lg: { state: 'on', thinq: false, amazon: false, dangbei: true, error: '' } })).notes.join('|') === N_OTHERS
  && VP.outcome(Object.assign(state({}), { hi_lg: { state: 'on', thinq: true, amazon: false, dangbei: false, error: '' } })).notes.join('|') === N_LISTEN
  && VP.outcome(Object.assign(state({}), { hi_lg: { state: 'off', thinq: false, amazon: false, dangbei: false, error: '' } })).notes.length === 0);
check('unreadable Always Ready or Hi LG: the warning note', notes({ hl: 'error' }).join('|') === N_X && notes({ ar: 'error' }).join('|') === N_X
  && notes({ hl: 'unknown' }).join('|') === N_X && notes({}).length === 0);

// Every combination against an oracle written from the rules, not from the code.
function oracle(o) {
  const arK = { off: 'off', none: 'off', alwaysReady: 'on', allEnabled: 'on' }[o.ar] || 'x';
  let power;
  if (arK === 'x' || o.ro === 'maybe') power = P_X;
  else {
    const keep = o.ro === 'keep_listening' && !o.kill && o.active !== false;
    power = arK === 'off' ? (keep ? P_KEEP : P_DEEP) : (keep ? P_KEEP : P_STBY);
  }
  let line;
  if (o.running === false) line = L.stopped;
  else if (o.kill) line = L.kill;
  else if (o.mic === 'muted') line = L.muted;
  else if (o.mic === 'busy') line = o.hl === 'on' ? L.hilg : L.busy;
  else if (o.mic === 'listening') line = L.ok;
  else if (o.mic === 'closed') line = L.closed;
  else line = L.unknown;
  const n = [];
  if (o.hl === 'on' && o.mic === 'listening' && !o.kill && o.running !== false) n.push(N_LISTEN);
  else if (o.hl === 'on' && (o.mic === 'closed' || o.mic === 'unknown') && !o.kill && o.running !== false) n.push(N_ON);
  if (o.others) n.push(N_OTHERS);
  if (arK === 'x' || o.hl === 'error' || o.hl === 'unknown') n.push(N_X);
  return { power, line, n };
}
let combos = 0;
let bad = '';
for (const ar of ['off', 'none', 'alwaysReady', 'allEnabled', 'lifeOnScreen', 'error', 'weird'])
  for (const hl of ['off', 'on', 'unknown', 'error'])
    for (const others of [false, true])
      for (const ro of ['normal', 'keep_listening', 'maybe'])
        for (const active of [true, false])
          for (const mic of ['listening', 'busy', 'muted', 'closed', 'unknown'])
            for (const kill of [false, true])
              for (const running of [true, false]) {
                const o = { ar, hl, others, ro, active, mic, kill, running };
                const got = VP.outcome(state(o));
                const want = oracle(o);
                combos += 1;
                // A stopped daemon or a kill switch hides the Hi LG reminders in the oracle; the app shows them
                // only when the line says the microphone state is real, so compare notes ignoring those two cases.
                const gotNotes = got.notes.filter((x) => (x !== N_LISTEN && x !== N_ON) || (!kill && running));
                if (text(got) !== want.power || got.listening !== want.line || gotNotes.join('|') !== want.n.join('|')) {
                  if (!bad) bad = JSON.stringify(o);
                }
              }
check(`all ${combos} combinations match the oracle`, !bad, bad);
check('every string in the tables is plain ASCII and one line', [].concat(Object.values(VP.POWER).map((p) => p.join(' ')), Object.values(VP.LISTEN), Object.values(VP.NOTE),
  Object.values(VP.POWER_EXTRA)).every((s) => /^[\x20-\x7e]+$/.test(s)));

// Rows.
const rows = (o) => VP.rows(state(o));
check('rows: Always Ready off and alwaysReady are changeable, with confirm', rows({ ar: 'off' })[0].can && /Press OK again to confirm\.$/.test(rows({ ar: 'off' })[0].confirm)
  && rows({ ar: 'off' })[0].cmd === 'set always-ready on' && rows({ ar: 'alwaysReady' })[0].cmd === 'set always-ready off'
  && rows({ ar: 'off' })[0].word === 'Off' && rows({ ar: 'alwaysReady' })[0].word === 'On');
check('rows: none, lifeOnScreen, allEnabled show the value and cannot be changed', ['none', 'lifeOnScreen', 'allEnabled'].every((a) => {
  const r = rows({ ar: a })[0];
  return !r.can && r.cmd === '' && /change it in LG's menu/i.test(r.warn) && r.word.indexOf(a === 'allEnabled' ? 'wallpaper' : a) >= 0;
}));
check('rows: Hi LG toggles thinQ with a confirm; unknown and error are not changeable',
  rows({ hl: 'off' })[1].cmd === 'set hi-lg on' && rows({ hl: 'on' })[1].cmd === 'set hi-lg off' && !!rows({ hl: 'on' })[1].confirm
  && !rows({ hl: 'error' })[1].can && rows({ hl: 'error' })[1].word === 'Cannot read' && !rows({ hl: 'unknown' })[1].can);
check('rows: Keep listening on needs one press, off needs two', rows({ ro: 'keep_listening' })[2].cmd === 'set remote-off normal'
  && rows({ ro: 'keep_listening' })[2].confirm === '' && rows({ ro: 'normal' })[2].cmd === 'set remote-off keep' && !!rows({ ro: 'normal' })[2].confirm);
check('rows before the first read say Checking... and cannot be pressed', VP.rows(null).every((r) => r.word === 'Checking...' && !r.can));

// ---- Part 2: the screen ----------------------------------------------------
const execs = () => calls.filter((c) => c.uri.endsWith('/exec')).map((c) => c.p.command.replace(/^sh '[^']*ctl\.sh' /, '').replace(/ 2>&1$/, ''));
const CTL_ALLOWED = /^sh '\/media\/developer\/apps\/usr\/palm\/applications\/io\.github\.pando85\.voicesatellite\/payload\/ctl\.sh' (status --json|tv-settings --json|set always-ready (on|off)|set hi-lg (on|off)|set remote-off (normal|keep)) 2>&1$/;
const key = (k) => { let prevented = false; doc.keydown({ keyCode: k, preventDefault() { prevented = true; } }); return prevented; };
const click = (id) => byId[id].listeners.click();
const press = async (id) => { click(id); for (let i = 0; i < 4; i++) await tick(); };
const word = (r) => byId[`v-${r}-state`].textContent;
const fireTimers = (ms) => { for (const t of timers.filter((x) => x.live && x.ms === ms)) { t.live = false; t.f(); } };

(async () => {
  for (let i = 0; i < 4; i++) await tick();
  check('the Voice and power button is shown for an installed TV', !byId['b-voice'].hidden);

  fake.tv = state({ ar: 'off', hl: 'off', ro: 'normal', mic: 'listening' });
  calls.length = 0;
  click('b-voice');
  check('opening shows Checking... before the first read', byId.main.hidden && !byId['voice-view'].hidden
    && byId['v-headline'].textContent === 'Checking...' && word('ar') === 'Checking...' && doc.activeElement === byId['v-row-ar']);
  for (let i = 0; i < 4; i++) await tick();
  check('first read runs tv-settings --json and shows the sentence and the rows', execs().join() === 'tv-settings --json'
    && byId['v-headline'].textContent === 'Power button: the TV goes to deep standby.'
    && byId['v-detail'].textContent === 'The satellite cannot hear until the TV is turned on.'
    && byId['v-listening'].textContent === L.ok && word('ar') === 'Off' && word('hl') === 'Off' && word('keep') === 'Off');
  check('focus stays on the first row after the read', doc.activeElement === byId['v-row-ar']);
  check('the focused row shows its warning', /^Turn on Always Ready\?/.test(byId['v-warning'].textContent));
  check('rows are switches with aria-checked', byId['v-row-ar'].attrs.role === 'switch' && byId['v-row-ar'].attrs['aria-checked'] === 'false');

  // focus order and wrapping
  const order = ['v-row-ar', 'v-row-hl', 'v-row-keep', 'v-refresh', 'v-back'];
  let ok = true;
  for (let i = 1; i < order.length; i++) { key(40); ok = ok && doc.activeElement === byId[order[i]]; }
  key(39); ok = ok && doc.activeElement === byId[order[0]];
  key(38); ok = ok && doc.activeElement === byId[order[4]];
  key(37); ok = ok && doc.activeElement === byId[order[3]];
  check('focus order: three rows, Refresh, Back; Down/Right next, Up/Left previous, wrapping', ok);
  byId['v-row-ar'].focus();

  // two presses
  calls.length = 0;
  await press('v-row-ar');
  check('first OK arms the row, shows the confirm text, sends nothing', execs().length === 0 && word('ar') === 'Press OK again'
    && /Press OK again to confirm\.$/.test(byId['v-warning'].textContent) && /^Turn on Always Ready\?/.test(byId['v-warning'].textContent));
  key(40);
  check('a focus move disarms', word('ar') === 'Off' && execs().length === 0);
  byId['v-row-ar'].focus();
  await press('v-row-ar');
  key(65);
  check('any other key disarms', word('ar') === 'Off');
  await press('v-row-ar');
  fireTimers(5000);
  check('the 5 s timeout disarms', word('ar') === 'Off' && execs().length === 0);
  timers.length = 0;
  await press('v-row-ar');
  const staleTimer = timers[timers.length - 1];
  key(65); await press('v-row-ar'); // disarm and arm again; the first timer must not disarm the second arming
  staleTimer.live = false; staleTimer.f();
  check('a stale timeout does not disarm a later arming', word('ar') === 'Press OK again');
  key(65);

  // applied
  fake.setReply = (c) => c === 'set always-ready on' ? '{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"on","result":"applied","error":""}\nwebos-satellite: ok\n' : 'unexpected ' + c;
  fake.tv = state({ ar: 'alwaysReady', hl: 'off', ro: 'normal' });
  calls.length = 0;
  byId['v-row-ar'].focus();
  click('v-row-ar'); await tick();
  click('v-row-ar');
  check('while the command runs: Changing... and presses are ignored', word('ar') === 'Changing...' && byId['v-row-hl'].attrs['aria-disabled'] === 'true');
  click('v-row-hl'); click('v-row-keep');
  for (let i = 0; i < 4; i++) await tick();
  check('second OK sends exactly set always-ready on, once', execs().join() === 'set always-ready on');
  check('applied: result line and the new sentence', byId['v-message'].textContent === 'Always Ready is now On. Changed from Off.'
    && /ok/.test(byId['v-message'].className) && word('ar') === 'On' && byId['v-headline'].textContent === 'Power button: the TV goes to standby.'
    && byId['v-detail'].textContent === 'The satellite still hears you, but you will not hear its replies.');
  check('a re-read is scheduled 3 s after a change', timers.some((t) => t.live && t.ms === 3000));
  calls.length = 0;
  fireTimers(3000);
  for (let i = 0; i < 4; i++) await tick();
  check('the 3 s re-read runs tv-settings --json and keeps the result line', execs().join() === 'tv-settings --json'
    && byId['v-message'].textContent === 'Always Ready is now On. Changed from Off.');

  // LG reverts it: the set says applied (off), the 3 s re-read finds On again
  fake.setReply = () => '{"schema":1,"setting":"always-ready","requested":"off","before":"on","after":"off","result":"applied","error":""}\n';
  click('v-row-ar'); await tick(); click('v-row-ar');
  for (let i = 0; i < 4; i++) await tick();
  check('before the re-read the row shows the applied value', word('ar') === 'Off' && byId['v-message'].textContent === 'Always Ready is now Off. Changed from On.');
  fake.tv = state({ ar: 'alwaysReady', hl: 'off', ro: 'normal' });
  fireTimers(3000);
  for (let i = 0; i < 4; i++) await tick();
  check('a revert seen by the 3 s re-read is reported', byId['v-message'].textContent === 'LG changed Always Ready back to On.' && word('ar') === 'On');

  // Keep listening: two presses to turn on, one to turn off
  fake.tv = state({ ar: 'alwaysReady', ro: 'normal' });
  await press('v-refresh');
  key(40); key(40);
  check('focus is on the Keep listening row', doc.activeElement === byId['v-row-keep']);
  calls.length = 0;
  fake.setReply = () => '{"schema":1,"setting":"remote-off","requested":"keep","before":"normal","after":"keep_listening","result":"applied","error":"","daemon":"applied"}\n';
  fake.tv = state({ ar: 'alwaysReady', ro: 'keep_listening' });
  await press('v-row-keep');
  check('turning Keep listening on needs a confirm', execs().length === 0 && /^Turn on Keep listening\?/.test(byId['v-warning'].textContent));
  await press('v-row-keep');
  check('second press sends set remote-off keep', execs().join() === 'set remote-off keep' && word('keep') === 'On'
    && byId['v-message'].textContent === 'Keep listening is now On. Changed from Off.' && byId['v-headline'].textContent.indexOf('the screen goes dark') > 0);
  calls.length = 0;
  fake.setReply = () => '{"schema":1,"setting":"remote-off","requested":"normal","before":"keep_listening","after":"normal","result":"applied","error":"","daemon":"applied"}\n';
  fake.tv = state({ ar: 'alwaysReady', ro: 'normal' });
  await press('v-row-keep');
  check('turning Keep listening off is one press', execs().join() === 'set remote-off normal' && word('keep') === 'Off'
    && byId['v-message'].textContent === 'Done. The power button turns the TV off the normal way.');

  // failed, refused, mismatch, already, unparsable
  const trial = async (id, reply, wantCmd) => {
    fake.setReply = () => reply;
    calls.length = 0;
    byId[id].focus();
    click(id); await tick();
    if (id !== 'v-row-keep' || !/keep_listening/.test(JSON.stringify(fake.tv))) { click(id); }
    for (let i = 0; i < 4; i++) await tick();
    return execs().indexOf(wantCmd) >= 0;
  };
  fake.tv = state({ ar: 'off', hl: 'off', ro: 'normal' });
  await press('v-refresh');
  await trial('v-row-ar', '{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"off","result":"failed","error":"not exist"}\n', 'set always-ready on');
  check('failed: reason, old value and the menu path; state unchanged', byId['v-message'].textContent
    === 'Could not change Always Ready: not exist. It is still Off. You can also change it on the TV: Settings, General, Always Ready.'
    && /bad/.test(byId['v-message'].className) && word('ar') === 'Off');
  await trial('v-row-ar', '{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"alwaysReady","result":"mismatch","error":""}\n'.replace('"alwaysReady"', '"allEnabled"'), 'set always-ready on');
  check('mismatch: shows what the TV reports', byId['v-message'].textContent
    === 'The TV reports On, with wallpaper, not On. The change did not stick, or LG changed it back. It is On, with wallpaper now.'.replace('On, with wallpaper, not On', 'On, with wallpaper, not On')
    && /warn/.test(byId['v-message'].className) && word('ar') === 'On, with wallpaper');
  fake.tv = state({ ar: 'off', hl: 'off', ro: 'normal' });
  await press('v-refresh');
  await trial('v-row-hl', '{"schema":1,"setting":"hi-lg","requested":"on","before":"off","after":"off","result":"refused","error":"not root"}\n', 'set hi-lg on');
  check('refused: reason and the Hi LG menu path', /^Could not change Hi LG: not root\. You can also change it on the TV: Settings, General, AI Service/.test(byId['v-message'].textContent));
  await trial('v-row-hl', '{"schema":1,"setting":"hi-lg","requested":"on","before":"on","after":"on","result":"already","error":""}\n', 'set hi-lg on');
  check('already: nothing changed', byId['v-message'].textContent === 'Already On. Nothing changed.');
  fake.tv = state({ ar: 'off', hl: 'off', ro: 'normal' });
  await press('v-refresh');
  await trial('v-row-hl', 'garbage\n', 'set hi-lg on');
  check('an unreadable answer is not counted as success', /^Could not confirm the change of Hi LG/.test(byId['v-message'].textContent) && /bad/.test(byId['v-message'].className));

  // unreadable and unchangeable states
  fake.tv = Object.assign(state({ hl: 'error', ar: 'lifeOnScreen' }), {});
  await press('v-refresh');
  calls.length = 0;
  byId['v-row-ar'].focus(); await press('v-row-ar');
  byId['v-row-hl'].focus(); await press('v-row-hl');
  check('unknown Always Ready and unreadable Hi LG: pressing sends nothing, the warning names the menu', execs().length === 0 && word('ar') === 'Not known (lifeOnScreen)'
    && word('hl') === 'Cannot read' && /Hi LG could not be read/.test(byId['v-warning'].textContent) && /Settings, General, AI Service/.test(byId['v-warning'].textContent));
  byId['v-row-ar'].focus(); key(40); key(38);
  check('Always Ready lifeOnScreen: warning says to change it in LG\'s menu', /Change it in LG's menu \(Settings, General, Always Ready\)/.test(byId['v-warning'].textContent));
  fake.tv = state({ ar: 'allEnabled' });
  await press('v-refresh');
  check('allEnabled: shown as On, with wallpaper, not changeable', word('ar') === 'On, with wallpaper');
  fake.tv = state({ others: true });
  await press('v-refresh');
  check('the note about Alexa/other hands-free is shown', byId['v-notes'].textContent === N_OTHERS);

  // total read failure
  fake.tvFails = true;
  await press('v-refresh');
  check('a failed read shows Cannot read, the not-known sentence and the reason', word('ar') === 'Cannot read' && byId['v-headline'].textContent === 'Power button: not known.'
    && /Could not read the TV settings/.test(byId['v-message'].textContent));
  fake.tvFails = false;
  fake.tv = state({});
  await press('v-refresh');

  // periodic refresh and Back
  calls.length = 0;
  const tickFn = intervals[intervals.length - 1].f;
  tickFn();
  for (let i = 0; i < 3; i++) await tick();
  check('every 10 s the open screen reads tv-settings --json (and not the main status)', execs().join() === 'tv-settings --json');
  await press('v-row-ar'); // armed
  calls.length = 0;
  tickFn();
  for (let i = 0; i < 3; i++) await tick();
  check('no periodic read while a row is armed', execs().length === 0);
  key(65);
  key(27);
  for (let i = 0; i < 3; i++) await tick();
  check('Back (27) leaves the screen, resumes the main refresh and focuses the button', !byId.main.hidden && byId['voice-view'].hidden
    && execs().indexOf('status --json') >= 0 && doc.activeElement === byId['b-voice']);
  click('b-voice');
  for (let i = 0; i < 3; i++) await tick();
  check('reopening reads again', !byId['voice-view'].hidden);
  key(461);
  check('Back (461) works too', byId['voice-view'].hidden);
  click('b-voice'); key(8);
  check('Back (8) works too', byId['voice-view'].hidden);

  check('only whitelisted commands were sent in the whole run', calls.filter((c) => c.uri.endsWith('/exec')).every((c) => CTL_ALLOWED.test(c.p.command)));
  if (fails) { console.log(`${fails} check(s) failed`); process.exit(1); }
  console.log('all checks passed');
})();
