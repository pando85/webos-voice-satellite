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
// that catches a revert, and that only whitelisted commands are sent. Part 3
// checks the Sound screen (echo cancellation and music, config keys whose
// change restarts the daemon): its model, two presses in both directions,
// on -> off -> on for both, the pending row, the result lines (restarted,
// not running, already, failed with the previous setting back), an older
// status without the fields, and that the phone has no request for them.
// usage: node packaging/webos/tests/app_matrix.js
'use strict';
const path = require('path');
const { load, checker } = require('./app_fake.js');

const VP = require(path.join(__dirname, '..', 'app', 'voicepower.js'));
const { check, done } = checker();

// ---- Part 1: the matrix ----------------------------------------------------
const P_DEEP = 'Power button: the TV goes to deep standby. The satellite cannot hear until the TV is turned on.';
const P_STBY = 'Power button: the TV goes to standby. The satellite still hears you, but you will not hear its replies.';
const P_KEEP = 'Power button: the TV turns itself back on with the screen dark. About 5 to 6 seconds after the key. The satellite hears you and replies. Press power again to turn the picture on.';
const P_KEEP_AR_OFF = 'Power button: the TV turns itself back on with the screen dark. About 5 to 6 seconds after the key. The satellite hears you and replies; press power again to turn the picture on. The TV may sometimes fall asleep instead, and the satellite then stops until the TV is turned on. Turn Always Ready on to avoid that.';
const HINT_TEXT = 'Works best with Always Ready on. Turn it on?';
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
check('P3 off + keep: may fall asleep, recommends Always Ready', text(VP.outcome(state({ ar: 'off', ro: 'keep_listening' }))) === P_KEEP_AR_OFF);
check('P4 alwaysReady + keep', text(VP.outcome(state({ ar: 'alwaysReady', ro: 'keep_listening' }))) === P_KEEP);
check('allEnabled behaves as on (P2, P4)', text(VP.outcome(state({ ar: 'allEnabled', ro: 'normal' }))) === P_STBY
  && text(VP.outcome(state({ ar: 'allEnabled', ro: 'keep_listening' }))) === P_KEEP);
check('P3 and P4 differ; only P3 mentions falling asleep', VP.POWER.P3.detail !== VP.POWER.P4.detail && /fall asleep/.test(VP.POWER.P3.detail)
  && !/fall asleep/.test(VP.POWER.P4.detail) && VP.POWER.P3.tone === 'warn' && VP.POWER.P4.tone === 'ok');
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
    power = arK === 'off' ? (keep ? P_KEEP_AR_OFF : P_DEEP) : (keep ? P_KEEP : P_STBY);
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
  return o.powerShort === VP.POWER.P3.short && o.powerTone === 'warn' && o.listenShort === 'No, muted' && o.listenTone === 'warn' && o.listenDetail === 'Muted in Home Assistant.';
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
// Keep listening wording and the Always Ready hint.
const hint = (o) => VP.hint(state(o));
const keepOn = rows({ ar: 'off', ro: 'normal' })[2].confirm;
check('Keep listening confirm: what happens, and "works best with Always Ready on" only while Always Ready is not on',
  /^Turn on Keep listening\? About 5 to 6 seconds after the power button the satellite turns the TV back on with the screen dark\./.test(keepOn)
  && /You can talk to it and hear replies; press power again for the picture\./.test(keepOn) && / Works best with Always Ready on\. Press OK again to confirm\.$/.test(keepOn)
  && !/Works best/.test(rows({ ar: 'alwaysReady', ro: 'normal' })[2].confirm) && !/Works best/.test(rows({ ar: 'allEnabled', ro: 'normal' })[2].confirm)
  && /Works best with Always Ready on\.$/.test(VP.requestText('remote-off', 'keep').detail));
check('Always Ready confirm texts explain the light standby and the 7 second margin', /light standby/.test(rows({ ar: 'off' })[0].warn)
  && /you hear no replies/.test(rows({ ar: 'off' })[0].warn) && /within about 7 seconds/.test(rows({ ar: 'alwaysReady' })[0].warn)
  && /sometimes fall asleep/.test(rows({ ar: 'alwaysReady' })[0].warn));
check('hint: Keep listening on and in effect with Always Ready off', JSON.stringify(hint({ ar: 'off', ro: 'keep_listening' }))
  === JSON.stringify({ text: HINT_TEXT, label: 'Turn on Always Ready', action: 'always-ready', value: 'on', cmd: 'set always-ready on' }));
check('hint: none when Always Ready is on, Keep listening is off, unknown, killed, or not watching',
  hint({ ar: 'alwaysReady', ro: 'keep_listening' }) === null && hint({ ar: 'off', ro: 'normal' }) === null
  && hint({ ar: 'off', ro: 'weird' }) === null && hint({ ar: 'off', ro: 'keep_listening', kill: true }) === null
  && hint({ ar: 'off', ro: 'keep_listening', active: false }) === null);
check('hint: none when Always Ready cannot be changed (none, lifeOnScreen, allEnabled, unreadable) or nothing was read',
  ['none', 'lifeOnScreen', 'allEnabled', 'error', 'weird'].every((a) => hint({ ar: a, ro: 'keep_listening' }) === null) && VP.hint(null) === null && VP.hint({}) === null);
check('rows before the first read say Checking... and cannot be pressed', VP.rows(null).every((r) => r.word === 'Checking...' && !r.can));

// ---- Part 2: the screen ----------------------------------------------------
const STATUS = { uid: 0, installed: true, installed_version: '0.4.0', package_version: '0.4.0', update_pending: false, running: true,
  pid: 42, supervised: true, autostart: true, disabled: false, crashloop: false, mic_off: false, firmware: 'firmware 03.30.74',
  firmware_ok: true, key: 'paired', name: 'webos-satellite-test', port: '6053', prefix: '/var/lib/webos-satellite',
  machine: 'aarch64', build: '', log: [], remote_off: 'normal', aec_enabled: true, music_enabled: false,
  mic_button: 'off', mic_button_active: false, mic_button_error: null, lg_voice_key: 'lg', overlay_enabled: true, overlay_app: true };
// What the fake ctl.sh answers: `tv` is the tv-settings reply, `setReply(cmd)` the reply of a set command.
const fake = { tv: state({}), setReply: null, tvFails: false };
const T = load({
  exec(args) {
    if (args === 'status --json') return JSON.stringify(STATUS) + '\n';
    if (args === 'tv-settings --json') return fake.tvFails ? 'ctl.sh: broken\n' : JSON.stringify(fake.tv) + '\n';
    if (args === 'setup status --json') return '{"schema":1,"state":"none","reason":"","port":null,"expires_in_s":null,"peer":null,"peer_other_subnet":null,"pending":null,"last_result":null}\n';
    if (/^config (aec|music|mic-button|overlay) /.test(args)) return fake.soundReply(args);
    if (/^set /.test(args)) return fake.setReply(args);
    return 'webos-satellite: done\n';
  },
});
const { byId, doc, settle, advance, execs, key, click, calls } = T;
const indexHtml = require('fs').readFileSync(require('path').join(__dirname, '..', 'app', 'index.html'), 'utf8');
check('index.html row sub-lines are the voicepower.js ones', VP.ROWS.every((r) => indexHtml.indexOf('<span class="vr-sub">' + r.sub + '</span>') > 0));
const CTL_ALLOWED = /^sh '\/media\/developer\/apps\/usr\/palm\/applications\/io\.github\.pando85\.voicesatellite\/payload\/ctl\.sh' (status --json|tv-settings --json|setup status --json|set always-ready (on|off)|set hi-lg (on|off)|set remote-off (normal|keep)|config (aec|music|mic-button|overlay) (on|off)) 2>&1$/;
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
    && byId['v-message'].textContent === 'Keep listening is now On. Changed from Off.' && byId['v-headline'].textContent.indexOf('with the screen dark') > 0);
  calls.length = 0;
  fake.setReply = () => '{"schema":1,"setting":"remote-off","requested":"normal","before":"keep_listening","after":"normal","result":"applied","error":"","daemon":"applied"}\n';
  fake.tv = state({ ar: 'alwaysReady', ro: 'normal' });
  await press('v-row-keep');
  check('turning Keep listening off is one press', sets().join() === 'set remote-off normal' && word('keep') === 'Off'
    && byId['v-message'].textContent === 'Done. The power button turns the TV off the normal way.');

  // The hint: Keep listening on and Always Ready off
  fake.tv = state({ ar: 'off', ro: 'normal' });
  await press('v-refresh');
  check('hint hidden while Keep listening is off', byId['v-hint'].hidden && byId['v-hint-btn'].hidden);
  fake.tv = state({ ar: 'alwaysReady', ro: 'keep_listening' });
  await press('v-refresh');
  check('hint hidden while Always Ready is on', byId['v-hint'].hidden && byId['v-hint-btn'].hidden);
  fake.tv = state({ ar: 'lifeOnScreen', ro: 'keep_listening' });
  await press('v-refresh');
  check('hint hidden while Always Ready cannot be changed', byId['v-hint'].hidden && byId['v-hint-btn'].hidden);
  fake.tv = state({ ar: 'off', ro: 'keep_listening' });
  await press('v-refresh');
  check('hint shown: text and a focusable action', !byId['v-hint'].hidden && !byId['v-hint-btn'].hidden
    && byId['v-hint-text'].textContent === HINT_TEXT && byId['v-hint-btn'].textContent === 'Turn on Always Ready'
    && /fall asleep/.test(byId['v-detail'].textContent));
  byId['v-row-ar'].focus();
  key(39);
  check('Right from the rows reaches the hint action', doc.activeElement === byId['v-hint-btn']);
  key(37);
  check('Left from the hint action returns to a row', ['v-row-ar', 'v-row-hl', 'v-row-keep'].indexOf(doc.activeElement.id) >= 0);
  byId['v-hint-btn'].focus();
  calls.length = 0;
  click('v-hint-btn');
  await settle();
  check('the hint arms the Always Ready change (two-press): nothing sent, confirm shown, focus on that row', sets().length === 0
    && doc.activeElement === byId['v-row-ar'] && word('ar') === 'Press OK again' && /^Turn on Always Ready\?.* Press OK again to confirm\.$/.test(byId['v-ar-confirm-text'].textContent));
  key(40);
  check('a focus move disarms it as usual', word('ar') === 'Off' && sets().length === 0);
  byId['v-hint-btn'].focus();
  click('v-hint-btn');
  fake.setReply = (c) => c === 'set always-ready on' ? '{"schema":1,"setting":"always-ready","requested":"on","before":"off","after":"on","result":"applied","error":""}\n' : 'unexpected ' + c;
  fake.tv = state({ ar: 'alwaysReady', ro: 'keep_listening' });
  await press('v-row-ar');
  check('second OK sends set always-ready on and the hint goes away', sets().join() === 'set always-ready on'
    && byId['v-hint'].hidden && byId['v-hint-btn'].hidden && word('ar') === 'On' && byId['v-headline'].textContent === VP.POWER.P4.headline);
  fake.tv = state({ ar: 'off', ro: 'keep_listening' });
  await press('v-refresh');
  byId['v-hint-btn'].focus();
  fake.tv = state({ ar: 'alwaysReady', ro: 'keep_listening' });
  await press('v-refresh');
  check('a hint that disappears under the focus hands the focus to a row', doc.activeElement !== byId['v-hint-btn'] && ['v-row-ar', 'v-row-hl', 'v-row-keep', 'v-refresh'].indexOf(doc.activeElement.id) >= 0);
  fake.tv = state({ ar: 'alwaysReady', ro: 'normal' });
  await press('v-refresh');

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

  // ---- Part 3: the Sound screen ----------------------------------------------
  const SR = VP.soundRows;
  check('sound rows before a status say Checking... and cannot be pressed', SR(null).every((r) => r.word === 'Checking...' && !r.can));
  check('sound rows from an older status without the fields: Not known, not pressable, says to update', SR({ installed: true }).every((r) => r.word === 'Not known' && !r.can && /Apply the update/.test(r.warn)));
  const sr = SR({ aec_enabled: true, music_enabled: false, mic_button: 'off', mic_button_active: false, overlay_enabled: true });
  check('sound rows: echo cancellation On turns off, music Off turns on, both with two presses',
    sr[0].word === 'On' && sr[0].checked === 'true' && sr[0].cmd === 'config aec off' && sr[1].word === 'Off' && sr[1].checked === 'false' && sr[1].cmd === 'config music on'
    && sr.every((r) => r.can && r.confirm === r.warn + VP.CONFIRM_TAIL && /restarts the voice service/.test(r.warn)));
  check('sound confirm texts: plain words, CPU for echo cancellation, the encrypted-only fetch and Experimental for music',
    /^Turn off echo cancellation\?/.test(sr[0].warn) && /little more CPU/.test(VP.soundConfirm('aec', true))
    && /^Turn on music\?/.test(sr[1].warn) && /encrypted connection, never from the TV itself/.test(sr[1].warn) && /Experimental/.test(sr[1].warn)
    && /only replies, announcements and timer alarms/.test(VP.soundConfirm('music', false)));
  check('sound rows: the on-screen assistant is On (the default) and turns off with config overlay off',
    sr[3].word === 'On' && sr[3].checked === 'true' && sr[3].cmd === 'config overlay off' && sr[3].can
    && /^Turn off the on-screen assistant\?/.test(sr[3].warn) && /restarts the voice service/.test(sr[3].warn));
  const ovOff = SR({ aec_enabled: true, music_enabled: false, mic_button: 'off', mic_button_active: false, overlay_enabled: false })[3];
  check('on-screen assistant off: Off, turns on; the confirm says it never takes the remote and plays no sound',
    ovOff.word === 'Off' && ovOff.cmd === 'config overlay on' && /never takes the remote and plays no sound/.test(ovOff.warn));
  check('on-screen assistant from an older status without the field: Not known, not pressable',
    SR({ aec_enabled: true, music_enabled: false, mic_button: 'off', mic_button_active: false })[3].word === 'Not known'
    && !SR({ aec_enabled: true, music_enabled: false, mic_button: 'off', mic_button_active: false })[3].can);
  check('sound result: the on-screen assistant', VP.soundResult('overlay', { schema: 1, setting: 'overlay', requested: 'off', before: 'on', after: 'off', result: 'applied', daemon: 'restarted', error: '' })[0]
    === 'On-screen assistant is now Off. The voice service restarted.' && VP.soundIndex('overlay') === 3 && VP.soundIndex('aec') === 0);
  check('sound rows: the mic button row is Off and turns on (config mic-button on, two presses)',
    sr[2].word === 'Off' && sr[2].checked === 'false' && sr[2].cmd === 'config mic-button on' && sr[2].can);
  const micOn = SR({ aec_enabled: true, music_enabled: false, mic_button: 'ptt', mic_button_active: true, lg_voice_key: 'ours (configd)' })[2];
  check('mic button running: On, turning it off gives the button back to LG', micOn.word === 'On' && micOn.checked === 'true'
    && micOn.cmd === 'config mic-button off' && /^Give the mic button back to LG\?/.test(micOn.warn));
  const micFail = SR({ aec_enabled: true, music_enabled: false, mic_button: 'ptt', mic_button_active: false, mic_button_error: 'the read-back says "launch", not ignore', lg_voice_key: 'lg' })[2];
  check('mic button configured but not running (takeover failed): Off (LG kept it), the reason, OK twice retries',
    micFail.word === 'Off (LG kept it)' && micFail.cls === 'warn' && micFail.checked === 'false' && micFail.cmd === 'config mic-button on'
    && /could not be taken from LG/.test(micFail.warn) && /the read-back says "launch", not ignore/.test(micFail.warn) && /^Turn on the remote mic button\?/.test(micFail.confirm));
  const micStopped = SR({ aec_enabled: true, music_enabled: false, mic_button: 'ptt', mic_button_active: null })[2];
  check('mic button configured, satellite not running: shown as configured (On)', micStopped.word === 'On');
  check('mic button confirm: LG\'s assistant stops opening from the button, its toast, given back at restart',
    /LG\'s voice assistant no longer opens from that button/.test(VP.soundConfirm('mic', true)) && /not supported/.test(VP.soundConfirm('mic', true))
    && /every TV restart/.test(VP.soundConfirm('mic', true)));
  check('sound summary: the mic button only when it runs', VP.soundSummary({ aec_enabled: true, music_enabled: false, mic_button: 'ptt', mic_button_active: true }) === 'Echo cancellation on, music off, mic button on'
    && VP.soundSummary({ aec_enabled: true, music_enabled: false, mic_button: 'ptt', mic_button_active: false }) === 'Echo cancellation on, music off');
  const stA = { mic_button: 'off', mic_button_active: false };
  VP.soundApply(stA, 'mic', 'on');
  check('a mic-button result updates the row until the next status read', stA.mic_button === 'ptt' && stA.mic_button_active === null);
  check('sound result: mic button on says the takeover is checked next', /whether that worked/.test(VP.soundResult('mic', { schema: 1, setting: 'mic-button', requested: 'on', before: 'off', after: 'on', result: 'applied', daemon: 'restarted', error: '' })[0]));
  check('sound summary for the Home tile', VP.soundSummary({ aec_enabled: false, music_enabled: true }) === 'Echo cancellation off, music on'
    && VP.soundSummary({}) === 'Echo cancellation, music' && VP.soundSummary(null) === 'Echo cancellation, music');
  const res = (o) => Object.assign({ schema: 1, setting: 'aec', requested: 'off', before: 'on', after: 'off', result: 'applied', daemon: 'restarted', error: '' }, o);
  check('sound result: applied and restarted', JSON.stringify(VP.soundResult('aec', res({}))) === JSON.stringify(['Echo cancellation is now Off. The voice service restarted.', 'ok']));
  check('sound result: applied while stopped', VP.soundResult('music', res({ setting: 'music', requested: 'on', before: 'off', after: 'on', daemon: 'not running' }))[0]
    === 'Music is now On. The satellite is not running; it uses this when it starts.');
  check('sound result: already', VP.soundResult('aec', res({ result: 'already', before: 'off', daemon: 'none' }))[0] === 'Already Off. Nothing changed.');
  check('sound result: failed with the previous setting back (restarted) and with the daemon down',
    JSON.stringify(VP.soundResult('music', res({ setting: 'music', result: 'failed', after: 'off', before: 'off', error: 'the satellite did not start with the new setting; the previous setting is back' })))
      === JSON.stringify(['Could not change Music: the satellite did not start with the new setting; the previous setting is back.', 'bad'])
    && / The satellite is not running: see Diagnostics\.$/.test(VP.soundResult('music', res({ setting: 'music', result: 'failed', daemon: 'failed', error: 'x' }))[0]));
  const cd = (o) => res(Object.assign({ result: 'refused', after: 'on', before: 'on', daemon: 'none', error: 'the last change was 5 s ago; try again in 25 s', reason: 'cooldown', retry_in_s: 25 }, o));
  check('sound result: refused by the cooldown, with and without retry_in_s (warn, nothing changed)',
    JSON.stringify(VP.soundResult('aec', cd({}))) === JSON.stringify(['Echo cancellation was changed moments ago. Try again in 25 s. Nothing changed.', 'warn'])
    && JSON.stringify(VP.soundResult('music', cd({ setting: 'music', retry_in_s: null }))) === JSON.stringify(['Music was changed moments ago. Try again in 30 s. Nothing changed.', 'warn']));
  check('sound result: refused while busy, for each kind (the reason text from ctl.sh)',
    [['a voice request is in progress'], ['music is loaded (playing or paused)'], ['a timer alarm is active']].every(([e]) =>
      JSON.stringify(VP.soundResult('aec', cd({ reason: 'busy', error: e, retry_in_s: null }))) === JSON.stringify(['Not now: ' + e + '. Nothing changed.', 'warn'])));
  check('sound result: a refusal without a reason (an older ctl.sh) keeps the generic text',
    JSON.stringify(VP.soundResult('aec', res({ result: 'refused', error: 'no config file', daemon: 'none' }))) === JSON.stringify(['Could not change Echo cancellation: no config file.', 'bad']));
  check('sound result: no or another reply', VP.soundResult('aec', null)[1] === 'bad' && /Could not confirm the change of Echo cancellation/.test(VP.soundResult('aec', res({ setting: 'music' }))[0]));
  check('the phone has no request for these two settings (a restart ends its session)', VP.requestText('aec', 'off') === null && VP.requestText('music', 'on') === null
    && !('aec' in VP.REQUEST) && !('music' in VP.REQUEST));
  check('index.html Sound sub-lines and the side note are the voicepower.js ones', VP.SOUND_ROWS.every((r) => indexHtml.indexOf('<span class="vr-sub">' + r.sub + '</span>') > 0)
    && indexHtml.indexOf('>' + VP.SOUND_NOTE.head + '<') > 0 && indexHtml.indexOf('>' + VP.SOUND_NOTE.detail + '<') > 0);

  // The fake ctl.sh for `config aec|music on|off`: what it answers and whether it changes the status.
  const sound = { mode: 'applied' };
  fake.soundReply = (args) => {
    const m = /^config (aec|music|mic-button|overlay) (on|off)$/.exec(args);
    if (m[1] === 'mic-button') {
      // The daemon takes LG's key at its restart: sound.mic says how that goes.
      const before = STATUS.mic_button === 'ptt' && STATUS.mic_button_active ? 'on' : 'off';
      STATUS.mic_button = m[2] === 'on' ? 'ptt' : 'off';
      STATUS.mic_button_active = m[2] === 'on' && sound.mic !== 'fail';
      STATUS.mic_button_error = m[2] === 'on' && sound.mic === 'fail' ? 'setConfigs failed: denied; LG\'s voice key was restored' : null;
      STATUS.lg_voice_key = STATUS.mic_button_active ? 'ours (configd)' : 'lg';
      return 'webos-satellite: started pid 44\n' + JSON.stringify(res({ setting: 'mic-button', requested: m[2], before, after: m[2] })) + '\n';
    }
    const k = m[1] + '_enabled';
    const before = STATUS[k] ? 'on' : 'off';
    if (sound.mode === 'rollback') {
      return 'webos-satellite: stopping pid 42\n' + JSON.stringify(res({ setting: m[1], requested: m[2], before, after: before, result: 'failed', daemon: 'restarted',
        error: 'the satellite did not start with the new setting; the previous setting is back' })) + '\n';
    }
    if (before === m[2]) return JSON.stringify(res({ setting: m[1], requested: m[2], before, after: before, result: 'already', daemon: 'none' })) + '\n';
    STATUS[k] = m[2] === 'on';
    return 'webos-satellite: stopping pid 42\nwebos-satellite: started pid 43\n' + JSON.stringify(res({ setting: m[1], requested: m[2], before, after: m[2] })) + '\n';
  };
  const sword = (r) => byId[`s-${r}-state`].textContent;
  const sR = (i) => VP.soundRows(STATUS)[i];
  check('the Sound tile is shown with the current state', !byId['h-sound'].hidden && byId['h-sound-sub'].textContent === 'Echo cancellation on, music off');
  calls.length = 0;
  click('h-sound');
  await settle();
  check('Sound opens: crumb, focus on the first row, rows from the status, its warning shown, a status read',
    !byId.sound.hidden && byId.home.hidden && byId.crumb.textContent === '/ Sound' && doc.activeElement === byId['s-row-aec']
    && sword('aec') === 'On' && sword('music') === 'Off' && byId['s-aec-sw'].className === 'sw on' && byId['s-music-sw'].className === 'sw'
    && byId['s-warning'].textContent === sr[0].warn && execs().indexOf('status --json') >= 0);
  key(40);
  const downOk = doc.activeElement === byId['s-row-music'] && byId['s-warning'].textContent === sr[1].warn;
  key(40);
  const toMic = doc.activeElement === byId['s-row-mic'];
  key(40);
  const toOverlay = doc.activeElement === byId['s-row-overlay'] && byId['s-warning'].textContent === sr[3].warn;
  key(40);
  const noWrap = doc.activeElement === byId['s-row-overlay'];
  key(38);
  key(38);
  key(38);
  check('Sound focus: Down to Music (its text on the right), to the mic button, to the on-screen assistant, no wrap, Up back',
    downOk && toMic && toOverlay && noWrap && doc.activeElement === byId['s-row-aec']);
  calls.length = 0;
  await press('s-row-aec');
  check('first OK arms the row, shows the confirm text in it, sends nothing', execs().length === 0 && sword('aec') === 'Press OK again'
    && !byId['s-aec-confirm'].hidden && byId['s-aec-confirm-text'].textContent === sr[0].confirm && byId['s-warning'].textContent === '');
  nextTick().f();
  await settle();
  check('no periodic status read while a Sound row is armed', execs().length === 0);
  advance(5000);
  check('the arming ends after 5 s', sword('aec') === 'On' && byId['s-aec-confirm'].hidden);
  await press('s-row-aec');
  key(40);
  check('a focus move disarms', sword('aec') === 'On' && byId['s-aec-confirm'].hidden && execs().length === 0);
  key(38);
  await press('s-row-aec');
  click('s-row-aec');
  check('the second OK sends config aec off; the row says Restarting... and both rows are inert', sword('aec') === 'Restarting...'
    && /is-disabled/.test(byId['s-row-aec'].className) && /is-disabled/.test(byId['s-row-music'].className) && /restarts/.test(byId['s-msg'].textContent));
  click('s-row-music');
  await settle();
  check('aec off: applied, Off, the result line, nothing else sent meanwhile', execs().filter((c) => /^config /.test(c)).join() === 'config aec off'
    && sword('aec') === 'Off' && byId['s-msg'].textContent === 'Echo cancellation is now Off. The voice service restarted.' && /ok/.test(byId['s-msg'].className));
  await press('s-row-aec'); await press('s-row-aec');
  check('aec on again: two presses send config aec on, On', execs().filter((c) => /^config /.test(c)).join() === 'config aec off,config aec on' && sword('aec') === 'On'
    && byId['s-msg'].textContent === 'Echo cancellation is now On. The voice service restarted.');
  byId['s-row-music'].focus();
  await press('s-row-music');
  check('music on is armed with its own confirm text', byId['s-music-confirm-text'].textContent === sr[1].confirm);
  await press('s-row-music');
  check('music on: applied, On', sword('music') === 'On' && byId['s-msg'].textContent === 'Music is now On. The voice service restarted.');
  await press('s-row-music'); await press('s-row-music');
  check('music off: applied, Off; on, off, on in order', sword('music') === 'Off'
    && execs().filter((c) => /^config /.test(c)).join() === 'config aec off,config aec on,config music on,config music off');
  sound.mode = 'rollback';
  await press('s-row-music'); await press('s-row-music');
  check('a failed change with the previous setting back: still Off, the reason in red', sword('music') === 'Off'
    && byId['s-msg'].textContent === 'Could not change Music: the satellite did not start with the new setting; the previous setting is back.' && /bad/.test(byId['s-msg'].className));
  sound.mode = 'applied';
  // The mic button: on (LG's key taken), off (given back), and a failed takeover.
  byId['s-row-mic'].focus();
  await press('s-row-mic');
  check('mic button on is armed with its own confirm text', byId['s-mic-confirm-text'].textContent === sr[2].confirm);
  await press('s-row-mic');
  await settle();
  check('mic button on: config mic-button on sent, the result says it is checked, then On from the status',
    execs().filter((c) => /^config mic-button/.test(c)).join() === 'config mic-button on' && /whether that worked/.test(byId['s-msg'].textContent) && sword('mic') === 'On');
  await press('s-row-mic'); await press('s-row-mic');
  await settle();
  check('mic button off: gives the button back, Off', sword('mic') === 'Off' && execs().filter((c) => /^config mic-button/.test(c)).join() === 'config mic-button on,config mic-button off');
  sound.mic = 'fail';
  await press('s-row-mic'); await press('s-row-mic');
  await settle();
  check('a failed takeover: Off (LG kept it) with the reason, OK twice tries again',
    sword('mic') === 'Off (LG kept it)' && /setConfigs failed: denied/.test(byId['s-warning'].textContent + byId['s-msg'].textContent + sR(2).warn) && sR(2).cmd === 'config mic-button on');
  sound.mic = 'ok';
  await press('s-row-mic'); await press('s-row-mic');
  await settle();
  check('the retry works: On', sword('mic') === 'On');
  await press('s-row-mic'); await press('s-row-mic');
  await settle();
  // The on-screen assistant: off and on again, two presses each.
  byId['s-row-overlay'].focus();
  await press('s-row-overlay');
  check('on-screen assistant off is armed with its own confirm text', byId['s-overlay-confirm-text'].textContent === sr[3].confirm && execs().filter((c) => /^config overlay/.test(c)).length === 0);
  await press('s-row-overlay');
  await settle();
  check('on-screen assistant off: config overlay off sent, Off, the result line', execs().filter((c) => /^config overlay/.test(c)).join() === 'config overlay off'
    && sword('overlay') === 'Off' && byId['s-msg'].textContent === 'On-screen assistant is now Off. The voice service restarted.');
  await press('s-row-overlay'); await press('s-row-overlay');
  await settle();
  check('on-screen assistant on again: On', sword('overlay') === 'On' && execs().filter((c) => /^config overlay/.test(c)).join() === 'config overlay off,config overlay on');
  byId['s-row-aec'].focus();
  key(461);
  await settle();
  check('Back returns to Home with the focus on the Sound tile, the tile shows the state', !byId.home.hidden && byId.sound.hidden
    && doc.activeElement === byId['h-sound'] && byId['h-sound-sub'].textContent === 'Echo cancellation on, music off');
  delete STATUS.aec_enabled;
  delete STATUS.music_enabled;
  delete STATUS.mic_button;
  click('h-sound');
  await settle();
  calls.length = 0;
  await press('s-row-aec'); await press('s-row-aec');
  check('an older status without the fields: Not known, nothing is sent', sword('aec') === 'Not known' && sword('music') === 'Not known' && sword('mic') === 'Not known'
    && execs().filter((c) => /^config /.test(c)).length === 0 && /Apply the update/.test(byId['s-warning'].textContent));
  key(461);
  await settle();
  STATUS.aec_enabled = true;
  STATUS.music_enabled = false;
  STATUS.mic_button = 'off';

  check('only whitelisted commands were sent in the whole run', calls.filter((c) => c.uri.endsWith('/exec')).every((c) => CTL_ALLOWED.test(c.p.command)));
  done();
})();
