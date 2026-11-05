// Host test of the shared model (packaging/webos/app/voicepower.js) and of
// the "Voice and power" screen of the TV app (app.js; design:
// docs/design/voice-and-power-screen.md, ADR 0008). Part 1 checks the outcome
// matrix (the sentence for the remote's power button and the microphone line)
// against literal strings for every cell and against an independently
// written oracle for every combination of Always Ready, Hi LG, Keep
// listening, microphone state, kill switch and daemon state; the short card
// wording next to the full wording; and the phone-request texts. Part 2
// drives the screen with a fake DOM and a fake Homebrew Channel: focus by
// direction, Back to Home, the two-press rule (and one press to turn Keep
// listening off), arming timeout and disarm keys, pending rows, the result
// lines (applied, already, failed, refused, mismatch), the delayed re-read
// that catches a revert, and that only whitelisted commands are sent.
// usage: node packaging/webos/tests/app_matrix.js
'use strict';
const path = require('path');
const { load, checker } = require('./app_fake.js');

const VP = require(path.join(__dirname, '..', 'app', 'voicepower.js'));
const { check, done } = checker();

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
const tableStrings = [].concat(
  ...Object.values(VP.POWER).map((p) => [p.headline, p.detail, p.short]), ...Object.values(VP.LISTEN).map((l) => [l.full, l.short, l.detail]),
  Object.values(VP.NOTE), Object.values(VP.POWER_EXTRA), ...Object.values(VP.KEY).map((k) => [k.short, k.full]),
  ...Object.values(VP.COMPAT).map((c) => [c.tag, c.short, c.full].filter(Boolean)), Object.values(VP.ENDED),
  ...Object.values(VP.REQUEST).map((a) => [].concat(...Object.values(a).map((e) => [e.ask, e.detail, e.done]))));
check('every string in the tables is plain ASCII and one line', tableStrings.every((s) => /^[\x20-\x7e]+$/.test(s)), tableStrings.find((s) => !/^[\x20-\x7e]+$/.test(s)));

// Short and full wording live in one entry: each cell has both, the short form is shorter.
check('POWER: every cell has headline, detail, a shorter card form and a tone', Object.values(VP.POWER).every((p) => p.headline && p.detail
  && p.short && p.short.length < p.headline.length && ['ok', 'warn', 'bad', 'idle'].includes(p.tone)));
check('LISTEN: every cell has the full line, a shorter card form, a card detail and a tone', Object.keys(VP.LISTEN).length === 8
  && Object.values(VP.LISTEN).every((l) => /^Listening now: /.test(l.full) && l.short && l.short.length < l.full.length && ['ok', 'warn', 'bad', 'idle'].includes(l.tone)));
check('outcome carries the card forms of its cells', (() => {
  const o = VP.outcome(state({ ar: 'off', ro: 'keep_listening', mic: 'muted' }));
  return o.powerShort === VP.POWER.P3.short && o.powerTone === 'ok' && o.listenShort === 'No, muted' && o.listenTone === 'warn' && o.listenDetail === 'Muted in Home Assistant.';
})());
check('Home Assistant card: key words, the pairing window wins, not installed', VP.haSummary({ installed: true, key: 'paired' }).short === 'Paired'
  && VP.haSummary({ installed: true, key: 'unpaired' }).tone === 'warn' && VP.haSummary({ installed: true, key: 'cleared' }).short === 'Unpaired by Home Assistant'
  && VP.haSummary({ installed: true, key: 'unpaired', pairing: { open: true, ends_in_s: 300 } }).key === 'pairing'
  && VP.haSummary({ installed: true, key: 'unpaired', pairing: { open: true, ends_in_s: 300 } }).pairingLeftS === 300
  && VP.haSummary({ installed: true, key: 'unpaired', pairing: { open: false, ends_in_s: null } }).key === 'unpaired'
  && VP.haSummary({ installed: false }).short === 'Not set up' && VP.haSummary({ installed: true, key: 'weird' }).key === 'none');
check('Home Assistant Home card: connected, disconnected, unpaired, unknown from ha.state; short and full wording; pairing and not installed win', (() => {
  const c = (ha, x) => VP.haCard(Object.assign({ installed: true, key: 'paired', ha: ha }, x));
  const words = Object.keys(VP.HA_CONN);
  return words.join() === 'connected,disconnected,unpaired,unknown'
    && words.every((w) => VP.HA_CONN[w].short && VP.HA_CONN[w].full && VP.HA_CONN[w].short.length < VP.HA_CONN[w].full.length && ['ok', 'warn', 'idle'].includes(VP.HA_CONN[w].tone))
    && c({ state: 'connected', since_s: 5 }).short === 'Connected' && c({ state: 'connected', since_s: 5 }).tone === 'ok'
    && c({ state: 'disconnected', since_s: 5 }).short === 'Disconnected' && c({ state: 'disconnected', since_s: 5 }).tone === 'warn'
    && c({ state: 'unpaired', since_s: null }).short === 'Not paired'
    && c({ state: 'unknown', since_s: null }).short === 'Not known' && c(undefined).key === 'unknown' && c(null).key === 'unknown'
    && c({ state: 'weird' }).key === 'unknown' && c('connected').key === 'unknown'
    && c({ state: 'connected' }, { pairing: { open: true, ends_in_s: 120 } }).key === 'pairing'
    && c({ state: 'connected' }, { pairing: { open: true, ends_in_s: 120 } }).pairingLeftS === 120
    && VP.haCard({ installed: false, ha: { state: 'connected' } }).short === 'Not set up'
    && VP.haCard({}).key === 'notinstalled';
})());
check('firmware card: capture beats changed beats the status word; the reason goes into the full text', (() => {
  const c = (x) => VP.compat({ installed: true, compat: Object.assign({ compat_status: 'not verified', selfcheck: 'passed' }, x) });
  return c({ capture_problem: true, firmware_changed: true, selfcheck: 'failed: no audio' }).name === 'capture'
    && /\(failed: no audio\)/.test(c({ capture_problem: true, selfcheck: 'failed: no audio' }).full)
    && c({ firmware_changed: true }).name === 'changed' && c({ compat_status: 'verified' }).short === ''
    && c({ compat_status: 'series verified (firmware not yet reported)' }).tag === 'Series verified' && c({}).tag === 'Not verified'
    && VP.compat({ installed: true }) === null;
})());

// Phone requests: texts keyed by action and value, nothing else accepted.
const rq = VP.requestText;
check('request texts: every action and value of the design table', rq('always-ready', 'on').ask === 'turn Always Ready on'
  && rq('always-ready', 'off').ask === 'turn Always Ready off' && rq('hi-lg', 'on').ask === 'turn Hi LG on' && rq('hi-lg', 'off').ask === 'turn Hi LG off'
  && rq('remote-off', 'keep').ask === 'turn Keep listening on' && rq('remote-off', 'normal').ask === 'turn Keep listening off'
  && rq('mic', 'on').ask === 'turn the microphone on' && rq('mic', 'off').ask === 'turn the microphone off'
  && rq('pair', '').ask === 'open the pairing window for 10 minutes' && rq('pair', null).ask === rq('pair', '').ask
  && rq('reply-volume', '35').ask === 'set the reply volume to 35 %');
check('request details reuse the Voice and power confirm texts (without the question)', rq('always-ready', 'on').detail === VP.confirmText('ar', true).replace(/^[^?]*\?\s*/, '')
  && rq('hi-lg', 'off').detail === 'The TV stops answering "Hi LG". The satellite can use the microphone.'
  && rq('remote-off', 'normal').detail === 'The power button then turns the TV off the normal way.'
  && rq('reply-volume', '0').detail === 'Replies and timer alarms will be silent.');
check('only the microphone off skips the TV confirmation', rq('mic', 'off').confirm === false && rq('mic', 'on').confirm === true
  && ['always-ready on', 'always-ready off', 'hi-lg on', 'hi-lg off', 'remote-off keep', 'remote-off normal', 'pair ', 'reply-volume 50']
    .every((x) => rq(x.split(' ')[0], x.split(' ')[1]).confirm === true));
check('unknown actions and values are refused (null)', [['always-ready', 'allEnabled'], ['hi-lg', 'ON'], ['remote-off', 'on'], ['mic', 'mute'], ['pair', 'x'],
  ['name', 'x'], ['reply-volume', '101'], ['reply-volume', '05'], ['reply-volume', '-1'], ['reply-volume', '5 '], ['reply-volume', 5], ['toString', ''],
  ['__proto__', ''], ['always-ready', 'toString'], [null, 'on'], ['hi-lg', {}]].every(([a, v]) => rq(a, v) === null));
check('IPv4 literals only', ['192.168.1.23', '10.0.0.1', '0.0.0.0'].every(VP.isIPv4)
  && !['192.168.1', '192.168.1.256', '192.168.01.2', 'fe80::1', 'tv.local', '1.2.3.4 ', '', null].some(VP.isIPv4));
const pr = (o) => VP.phoneResult(Object.assign({ id: 'a1b2c3d4', action: 'hi-lg', value: 'off', error: '', peer: '192.0.2.77' }, o));
check('phone result lines from verdict and result', pr({ verdict: 'allowed', result: 'ok' })[0] === 'From the phone: Hi LG is off.'
  && pr({ verdict: 'allowed', result: 'ok' })[1] === 'ok'
  && pr({ verdict: 'allowed', result: 'failed', error: 'timeout' })[0] === 'Could not turn Hi LG off: timeout.'
  && pr({ verdict: 'refused', result: '' })[0] === 'Refused the phone: turn Hi LG off. Nothing changed.'
  && pr({ verdict: 'expired', result: '' })[0] === 'No answer on the TV in time: turn Hi LG off. Nothing changed.'
  && pr({ verdict: 'aborted', result: '' })[1] === 'warn'
  && pr({ verdict: 'auto', result: 'ok', action: 'mic', value: 'off' })[0] === 'Microphone turned off from the phone.'
  && pr({ verdict: 'allowed', result: 'ok', action: 'evil', value: 'x' })[0] === 'The phone asked for something this app does not know. Nothing changed.'
  && VP.phoneResult(null) === null);
check('ended reasons have their own line; unknown reasons a neutral one', VP.endedText('timeout') === 'The phone session ended after 10 minutes.'
  && VP.endedText('app-gone') !== VP.endedText('done') && VP.endedText('whatever') === 'The phone session ended.' && VP.endedText('toString') === 'The phone session ended.');

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
const STATUS = { uid: 0, installed: true, installed_version: '0.4.0', package_version: '0.4.0', update_pending: false, running: true,
  pid: 42, supervised: true, autostart: true, disabled: false, crashloop: false, mic_off: false, firmware: 'firmware 03.30.74',
  firmware_ok: true, key: 'paired', name: 'webos-satellite-test', port: '6053', prefix: '/var/lib/webos-satellite',
  machine: 'aarch64', build: '', log: [], remote_off: 'normal' };
// What the fake ctl.sh answers: `tv` is the tv-settings reply, `setReply(cmd)` the reply of a set command.
const fake = { tv: state({}), setReply: null, tvFails: false };
const T = load({
  exec(args) {
    if (args === 'status --json') return JSON.stringify(STATUS) + '\n';
    if (args === 'tv-settings --json') return fake.tvFails ? 'ctl.sh: broken\n' : JSON.stringify(fake.tv) + '\n';
    if (args === 'setup status --json') return '{"schema":1,"state":"none","reason":"","port":null,"expires_in_s":null,"peer":null,"peer_other_subnet":null,"pending":null,"last_result":null}\n';
    if (/^set /.test(args)) return fake.setReply(args);
    return 'webos-satellite: done\n';
  },
});
const { byId, doc, settle, advance, execs, key, click, calls } = T;
const CTL_ALLOWED = /^sh '\/media\/developer\/apps\/usr\/palm\/applications\/io\.github\.pando85\.voicesatellite\/payload\/ctl\.sh' (status --json|tv-settings --json|setup status --json|set always-ready (on|off)|set hi-lg (on|off)|set remote-off (normal|keep)) 2>&1$/;
const press = async (id) => { click(id); await settle(); };
const word = (r) => byId[`v-${r}-state`].textContent;
const sets = () => execs().filter((c) => c !== 'tv-settings --json' && c !== 'status --json');
const nextTick = () => T.timers.filter((t) => t.live && t.interval && t.ms === 10000)[0];

(async () => {
  await settle();
  check('the Voice and power tile is shown for an installed TV', !byId['h-voice'].hidden);

  fake.tv = state({ ar: 'off', hl: 'off', ro: 'normal', mic: 'listening' });
  calls.length = 0;
  click('h-voice');
  check('opening shows Checking... before the first read', byId.home.hidden && !byId.voice.hidden
    && byId['v-headline'].textContent === 'Checking...' && word('ar') === 'Checking...' && doc.activeElement === byId['v-row-ar']);
  await settle();
  check('first read runs tv-settings --json and shows the sentence and the rows', execs().join() === 'tv-settings --json'
    && byId['v-headline'].textContent === 'Power button: the TV goes to deep standby.'
    && byId['v-detail'].textContent === 'The satellite cannot hear until the TV is turned on.'
    && byId['v-listening'].textContent === L.ok && word('ar') === 'Off' && word('hl') === 'Off' && word('keep') === 'Off');
  check('the crumb names the screen and the hint bar says Back goes Home', byId.crumb.textContent === '/ Voice and power'
    && !byId['key-back-box'].hidden && byId['key-back'].textContent === 'Home');
  check('focus stays on the first row after the read', doc.activeElement === byId['v-row-ar']);
  check('the focused row shows its warning', /^Turn on Always Ready\?/.test(byId['v-warning'].textContent));
  check('rows are switches with aria-checked and a switch drawing', byId['v-row-ar'].attrs.role === 'switch' && byId['v-row-ar'].attrs['aria-checked'] === 'false'
    && byId['v-ar-sw'].className === 'sw');

  // focus by direction: down through the rows to Refresh, no wrapping, sideways goes nowhere
  const order = ['v-row-ar', 'v-row-hl', 'v-row-keep', 'v-refresh'];
  let ok = true;
  for (let i = 1; i < order.length; i++) { key(40); ok = ok && doc.activeElement === byId[order[i]]; }
  key(40); ok = ok && doc.activeElement === byId['v-refresh'];
  for (let i = order.length - 2; i >= 0; i--) { key(38); ok = ok && doc.activeElement === byId[order[i]]; }
  key(38); ok = ok && doc.activeElement === byId['v-row-ar'];
  key(39); ok = ok && doc.activeElement === byId['v-row-ar'];
  key(37); ok = ok && doc.activeElement === byId['v-row-ar'];
  check('focus by direction: Down and Up move through the rows and Refresh, without wrapping; Left and Right stay', ok);

  // two presses
  calls.length = 0;
  await press('v-row-ar');
  check('first OK arms the row, shows the confirm text in the row, sends nothing', execs().length === 0 && word('ar') === 'Press OK again'
    && !byId['v-ar-confirm'].hidden && /^Turn on Always Ready\?.* Press OK again to confirm\.$/.test(byId['v-ar-confirm-text'].textContent)
    && byId['v-warning'].textContent === '');
  key(40);
  check('a focus move disarms', word('ar') === 'Off' && byId['v-ar-confirm'].hidden && execs().length === 0);
  byId['v-row-ar'].focus();
  await press('v-row-ar');
  key(65);
  check('any other key disarms', word('ar') === 'Off');
  await press('v-row-ar');
  advance(5000);
  check('the 5 s timeout disarms', word('ar') === 'Off' && sets().length === 0);
  await press('v-row-ar');
  key(65);
  advance(1000);
  await press('v-row-ar'); // disarmed and armed again; the first timer must not disarm the second arming
  advance(4500);
  check('a stale timeout does not disarm a later arming', word('ar') === 'Press OK again');
  key(65);

  // applied
  fake.setReply = (c) => c === 'set always-ready on' ? '{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"on","result":"applied","error":""}\nwebos-satellite: ok\n' : 'unexpected ' + c;
  fake.tv = state({ ar: 'alwaysReady', hl: 'off', ro: 'normal' });
  await settle();
  calls.length = 0;
  byId['v-row-ar'].focus();
  click('v-row-ar'); await settle(1);
  click('v-row-ar');
  check('while the command runs: Changing... and presses are ignored', word('ar') === 'Changing...' && byId['v-row-hl'].attrs['aria-disabled'] === 'true');
  click('v-row-hl'); click('v-row-keep');
  await settle();
  check('second OK sends exactly set always-ready on, once', sets().join() === 'set always-ready on');
  check('applied: result line and the new sentence', byId['v-message'].textContent === 'Always Ready is now On. Changed from Off.'
    && /ok/.test(byId['v-message'].className) && word('ar') === 'On' && byId['v-headline'].textContent === 'Power button: the TV goes to standby.'
    && byId['v-detail'].textContent === 'The satellite still hears you, but you will not hear its replies.' && byId['v-ar-sw'].className === 'sw on');
  calls.length = 0;
  advance(3000);
  await settle();
  check('the 3 s re-read runs tv-settings --json and keeps the result line', execs().includes('tv-settings --json')
    && byId['v-message'].textContent === 'Always Ready is now On. Changed from Off.');

  // LG reverts it: the set says applied (off), the 3 s re-read finds On again
  fake.setReply = () => '{"schema":1,"setting":"always-ready","requested":"off","before":"on","after":"off","result":"applied","error":""}\n';
  click('v-row-ar'); await settle(1); click('v-row-ar');
  await settle();
  check('before the re-read the row shows the applied value', word('ar') === 'Off' && byId['v-message'].textContent === 'Always Ready is now Off. Changed from On.');
  fake.tv = state({ ar: 'alwaysReady', hl: 'off', ro: 'normal' });
  advance(3000);
  await settle();
  check('a revert seen by the 3 s re-read is reported', byId['v-message'].textContent === 'LG changed Always Ready back to On.' && word('ar') === 'On');

  // Keep listening: two presses to turn on, one to turn off
  fake.tv = state({ ar: 'alwaysReady', ro: 'normal' });
  await press('v-refresh');
  byId['v-row-ar'].focus();
  key(40); key(40);
  check('focus is on the Keep listening row', doc.activeElement === byId['v-row-keep']);
  calls.length = 0;
  fake.setReply = () => '{"schema":1,"setting":"remote-off","requested":"keep","before":"normal","after":"keep_listening","result":"applied","error":"","daemon":"applied"}\n';
  fake.tv = state({ ar: 'alwaysReady', ro: 'keep_listening' });
  await press('v-row-keep');
  check('turning Keep listening on needs a confirm', sets().length === 0 && /^Turn on Keep listening\?/.test(byId['v-keep-confirm-text'].textContent));
  await press('v-row-keep');
  check('second press sends set remote-off keep', sets().join() === 'set remote-off keep' && word('keep') === 'On'
    && byId['v-message'].textContent === 'Keep listening is now On. Changed from Off.' && byId['v-headline'].textContent.indexOf('the screen goes dark') > 0);
  calls.length = 0;
  fake.setReply = () => '{"schema":1,"setting":"remote-off","requested":"normal","before":"keep_listening","after":"normal","result":"applied","error":"","daemon":"applied"}\n';
  fake.tv = state({ ar: 'alwaysReady', ro: 'normal' });
  await press('v-row-keep');
  check('turning Keep listening off is one press', sets().join() === 'set remote-off normal' && word('keep') === 'Off'
    && byId['v-message'].textContent === 'Done. The power button turns the TV off the normal way.');

  // failed, refused, mismatch, already, unparsable
  const trial = async (id, reply, wantCmd) => {
    fake.setReply = () => reply;
    calls.length = 0;
    byId[id].focus();
    click(id); await settle(1);
    if (id !== 'v-row-keep' || !/keep_listening/.test(JSON.stringify(fake.tv))) { click(id); }
    await settle();
    return execs().indexOf(wantCmd) >= 0;
  };
  fake.tv = state({ ar: 'off', hl: 'off', ro: 'normal' });
  await press('v-refresh');
  await trial('v-row-ar', '{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"off","result":"failed","error":"not exist"}\n', 'set always-ready on');
  check('failed: reason, old value and the menu path; state unchanged', byId['v-message'].textContent
    === 'Could not change Always Ready: not exist. It is still Off. You can also change it on the TV: Settings, General, Always Ready.'
    && /bad/.test(byId['v-message'].className) && word('ar') === 'Off');
  await trial('v-row-ar', '{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"allEnabled","result":"mismatch","error":""}\n', 'set always-ready on');
  check('mismatch: shows what the TV reports', byId['v-message'].textContent
    === 'The TV reports On, with wallpaper, not On. The change did not stick, or LG changed it back. It is On, with wallpaper now.'
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
  fake.tv = state({ hl: 'error', ar: 'lifeOnScreen' });
  await press('v-refresh');
  calls.length = 0;
  byId['v-row-ar'].focus(); await press('v-row-ar');
  byId['v-row-hl'].focus(); await press('v-row-hl');
  check('unknown Always Ready and unreadable Hi LG: pressing sends nothing, the warning names the menu', sets().length === 0 && word('ar') === 'Not known (lifeOnScreen)'
    && word('hl') === 'Cannot read' && /Hi LG could not be read/.test(byId['v-warning'].textContent) && /Settings, General, AI Service/.test(byId['v-warning'].textContent)
    && /na/.test(byId['v-hl-sw'].className) && /mixed/.test(byId['v-ar-sw'].className));
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
  advance(nextTick().at - T.clock.now);
  await settle();
  check('every 10 s the open screen reads tv-settings --json (and not the main status)', execs().join() === 'tv-settings --json');
  byId['v-row-ar'].focus();
  await press('v-row-ar'); // armed
  calls.length = 0;
  nextTick().f();
  await settle();
  check('no periodic read while a row is armed', execs().length === 0);
  key(65);
  key(27);
  await settle();
  check('Back (27) returns to Home, resumes the status refresh and focuses the tile', !byId.home.hidden && byId.voice.hidden
    && execs().indexOf('status --json') >= 0 && doc.activeElement === byId['h-voice'] && byId['key-back-box'].hidden);
  click('h-voice');
  await settle();
  check('reopening reads again', !byId.voice.hidden && execs().filter((c) => c === 'tv-settings --json').length >= 1);
  key(461);
  check('Back (461) works too', byId.voice.hidden && !byId.home.hidden);
  click('h-voice'); key(8);
  check('Back (8) works too', byId.voice.hidden);
  await settle();
  check('Home cards use the same model: listening and power-button short forms', byId['h-listen-value'].textContent === 'Yes'
    && byId['h-power-value'].textContent === VP.POWER.P1.short && byId['chip-text'].textContent === 'Listening');

  check('only whitelisted commands were sent in the whole run', calls.filter((c) => c.uri.endsWith('/exec')).every((c) => CTL_ALLOWED.test(c.p.command)));
  done();
})();
