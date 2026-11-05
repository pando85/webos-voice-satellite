// Host test of the phone page's pure logic (assets/phone.js) against the real
// shared model (packaging/webos/app/voicepower.js). No DOM, no network.
// Run: node crates/setup-server/tests/phone_logic.js
'use strict';
const assert = require('assert');
const fs = require('fs');
const path = require('path');

const root = path.join(__dirname, '..', '..', '..');
const vp = require(path.join(root, 'packaging/webos/app/voicepower.js'));
const phone = require(path.join(__dirname, '..', 'assets', 'phone.js'));
const html = fs.readFileSync(path.join(__dirname, '..', 'assets', 'phone.html'), 'utf8');
const js = fs.readFileSync(path.join(__dirname, '..', 'assets', 'phone.js'), 'utf8');

function state(over) {
  const s = {
    schema: 1, age_s: 1.2,
    device: { installed: true, installed_version: '0.4.0', running: true, key: 'unpaired', port: 6053,
      reply_volume: 40, reply_volume_default: 40, pairing_window_s: 600, pairing: { open: false, ends_in_s: null } },
    tv: { luna: true, always_ready: { state: 'off', raw: 'off', error: '' },
      hi_lg: { state: 'off', thinq: false, amazon: false, dangbei: false, error: '' },
      remote_off: { state: 'normal', active: true }, mic: { state: 'listening', kill_switch: false },
      power_state: 'Active', last_change: null },
    errors: { device: '', tv: '' }, pending: null
  };
  return Object.assign(s, over || {});
}

// ---- clock and volume ----
assert.strictEqual(phone.clock(0), '0:00');
assert.strictEqual(phone.clock(65), '1:05');
assert.strictEqual(phone.clock(599.9), '9:59');
assert.strictEqual(phone.clock(-5), '0:00');
assert.strictEqual(phone.volumeText(40), '40 % is -30 dB.');
assert.strictEqual(phone.volumeText(100), '100 % is 0 dB.');
assert.match(phone.volumeText(0), /^Silent/);

// ---- the view: unpaired TV, everything readable ----
let v = phone.buildView(vp, state(), false);
assert.strictEqual(v.cards.listen.value, 'Yes');
assert.strictEqual(v.cards.listen.tone, 'ok');
assert.strictEqual(v.cards.ha.value, 'Not paired');
assert.strictEqual(v.cards.power.value, vp.POWER.P1.short);
assert.strictEqual(v.out.head, vp.POWER.P1.headline);
assert.ok(v.ha.show && v.ha.showPair && v.ha.canPair);
assert.strictEqual(v.ha.port, '6053');
assert.strictEqual(v.rows.length, 3);
assert.deepStrictEqual(v.rows.map((r) => [r.action, r.value, r.can]),
  [['always-ready', 'on', true], ['hi-lg', 'on', true], ['remote-off', 'keep', true]]);
assert.strictEqual(v.rows[2].sub.indexOf('A satellite setting'), -1);
assert.deepStrictEqual([v.mic.checked, v.mic.can], [true, true]);
assert.deepStrictEqual([v.volume.value, v.volume.can], [40, true]);
assert.strictEqual(v.warn, '');

// busy disables every control
v = phone.buildView(vp, state(), true);
assert.ok(v.rows.every((r) => !r.can) && !v.mic.can && !v.volume.can && !v.ha.canPair);

// paired: the add box and pairing go away
v = phone.buildView(vp, state({ device: Object.assign(state().device, { key: 'paired' }) }), false);
assert.ok(!v.ha.show && !v.ha.showPair);
assert.strictEqual(v.cards.ha.value, 'Paired');
v = phone.buildView(vp, state({ device: Object.assign(state().device, { key: 'cleared' }) }), false);
assert.ok(v.ha.show && v.ha.showPair);
// pairing window open: the countdown is offered, no second window
v = phone.buildView(vp, state({ device: Object.assign(state().device, { pairing: { open: true, ends_in_s: 321 } }) }), false);
assert.ok(v.ha.show && !v.ha.showPair && v.ha.pairLeftS === 321);
assert.strictEqual(v.cards.ha.value, 'Waiting for pairing');
// daemon stopped: no pairing button action, listening says so
v = phone.buildView(vp, state({ device: Object.assign(state().device, { running: false }) }), false);
assert.ok(v.ha.showPair && !v.ha.canPair);
assert.strictEqual(v.cards.listen.value, 'No, stopped');

// kill switch: microphone switch off, Keep listening noted
let s = state();
s.tv.mic = { state: 'closed', kill_switch: true };
s.tv.remote_off = { state: 'keep_listening', active: true };
v = phone.buildView(vp, s, false);
assert.strictEqual(v.mic.checked, false);
assert.match(v.mic.sub, /Off on this TV/);
assert.strictEqual(v.cards.listen.value, 'No, microphone off');
assert.deepStrictEqual([v.rows[2].action, v.rows[2].value], ['remote-off', 'normal']);

// Keep listening on while Always Ready is off: the phone offers the same
// Always Ready request as the switch; no hint otherwise
s = state();
v = phone.buildView(vp, s, false);
assert.strictEqual(v.hint, null);
s.tv.remote_off = { state: 'keep_listening', active: true };
v = phone.buildView(vp, s, false);
assert.deepStrictEqual(v.hint, { text: 'Works best with Always Ready on. Turn it on?', label: 'Turn on Always Ready',
  action: 'always-ready', value: 'on', can: true });
assert.strictEqual(phone.askText(vp, v.hint.action, v.hint.value), 'Turn Always Ready on');
assert.strictEqual(phone.buildView(vp, s, true).hint.can, false);
s.tv.always_ready = { state: 'on', raw: 'alwaysReady', error: '' };
assert.strictEqual(phone.buildView(vp, s, false).hint, null);
s.tv.always_ready = { state: 'on', raw: 'lifeOnScreen', error: '' };
assert.strictEqual(phone.buildView(vp, s, false).hint, null);
s = state({ tv: null, errors: { device: '', tv: 'timeout' } });
assert.strictEqual(phone.buildView(vp, s, false).hint, null);

// Always Ready in an unknown mode cannot be switched from the phone
s = state();
s.tv.always_ready = { state: 'on', raw: 'lifeOnScreen', error: '' };
v = phone.buildView(vp, s, false);
assert.strictEqual(v.rows[0].can, false);
assert.ok(v.rows[0].note.length > 0);

// the TV settings could not be read: nothing is guessed
s = state({ tv: null, errors: { device: '', tv: 'timeout after 25 s' } });
v = phone.buildView(vp, s, false);
assert.strictEqual(v.cards.power.value, vp.POWER.PX.short);
assert.ok(v.rows.every((r) => !r.can));
assert.strictEqual(v.mic.can, false);
assert.match(v.warn, /could not be read: timeout/);
// nothing at all yet
v = phone.buildView(vp, state({ tv: null, device: null }), false);
assert.strictEqual(v.cards.listen.value, 'Checking...');
assert.strictEqual(v.volume.value, null);
assert.ok(!v.ha.show);
// a stale mirror is said
v = phone.buildView(vp, state({ age_s: 30 }), false);
assert.match(v.warn, /30 s old/);

// ---- requests and results ----
assert.strictEqual(phone.askText(vp, 'hi-lg', 'on'), 'Turn Hi LG on');
assert.strictEqual(phone.askText(vp, 'pair', null), 'Open the pairing window for 10 minutes');
assert.strictEqual(phone.askText(vp, 'reply-volume', '30'), 'Set the reply volume to 30 %');
assert.strictEqual(phone.askText(vp, 'nope', 'x'), '');

const req = { id: 'a1b2c3d4', action: 'hi-lg', value: 'off' };
let r = phone.resultLine(vp, req, { state: 'done', result: { result: 'applied', error: '' } });
assert.deepStrictEqual(r, ['From the phone: Hi LG is off.', 'ok']);
r = phone.resultLine(vp, req, { state: 'done', result: { result: 'failed', error: 'timeout' } });
assert.strictEqual(r[1], 'bad');
assert.match(r[0], /timeout/);
r = phone.resultLine(vp, req, { state: 'done', result: { result: 'mismatch', error: '' } });
assert.strictEqual(r[1], 'warn');
r = phone.resultLine(vp, req, { state: 'refused', result: null });
assert.deepStrictEqual(r, ['Refused on the TV: turn Hi LG off. Nothing changed.', 'warn']);
r = phone.resultLine(vp, req, { state: 'expired', result: null });
assert.match(r[0], /^No answer on the TV in time/);
r = phone.resultLine(vp, req, { state: 'aborted', result: null });
assert.match(r[0], /session ended first/);
// the microphone turned off runs without the OK: verdict auto
const mic = { id: 'a1b2c3d4', action: 'mic', value: 'off' };
assert.strictEqual(phone.lastResult(vp, mic, { state: 'done', result: { result: 'ok', error: '' } }).verdict, 'auto');
r = phone.resultLine(vp, mic, { state: 'done', result: { result: 'ok', error: '' } });
assert.deepStrictEqual(r, ['Microphone turned off from the phone.', 'warn']);
assert.strictEqual(phone.lastResult(vp, { id: 'x', action: 'mic', value: 'on' }, { state: 'done', result: { result: 'ok', error: '' } }).verdict, 'allowed');
// pair has no value
r = phone.resultLine(vp, { id: 'x', action: 'pair' }, { state: 'done', result: { result: 'ok', error: '' } });
assert.deepStrictEqual(r, ['From the phone: The pairing window is open.', 'ok']);
// volume
r = phone.resultLine(vp, { id: 'x', action: 'reply-volume', value: '35' }, { state: 'done', result: { result: 'applied', error: '' } });
assert.deepStrictEqual(r, ['From the phone: The reply volume is 35 %.', 'ok']);

// ---- API errors ----
assert.match(phone.errorLine(503, { error: 'app_gone', message: 'ignored' }), /TV app is not answering/);
assert.match(phone.errorLine(429, null), /Wait a moment/);
assert.match(phone.errorLine(409, null), /Another request/);
assert.strictEqual(phone.errorLine(422, { error: 'not_applicable', message: 'The pairing window is only for an unpaired TV.' }), 'The pairing window is only for an unpaired TV.');
assert.match(phone.errorLine(500, null), /\(500\)/);
assert.deepStrictEqual([401, 403, 410, 200, 404, 503, 0].map(phone.sessionGone), [true, true, true, false, false, false, false]);
assert.strictEqual(vp.endedText('timeout'), 'The phone session ended after 10 minutes.');

// ---- the page is CSP-clean and every id the script reads exists ----
assert.ok(!/\son[a-z]+\s*=/i.test(html), 'inline event handler');
assert.ok(!/style\s*=|<style|<script>(?!<)/i.test(html), 'inline style or script');
assert.ok(!/https?:\/\//.test(html), 'remote URL in the page');
assert.ok(!/(innerHTML|outerHTML|insertAdjacentHTML|document\.write|eval\(|new Function|setAttribute\('style')/.test(js), 'unsafe DOM API');
assert.ok(!/(localStorage|sessionStorage|document\.cookie)/.test(js), 'storage');
const ids = new Set([...html.matchAll(/\sid="([^"]+)"/g)].map((m) => m[1]));
for (const m of js.matchAll(/\$\('([^']+)'\)/g)) { assert.ok(ids.has(m[1]), 'no #' + m[1] + ' in phone.html'); }
for (const m of html.matchAll(/(?:for|aria-labelledby)="([^"]+)"/g)) { assert.ok(ids.has(m[1]), 'dangling ' + m[1]); }
// the only absolute URL is the My Home Assistant redirect
assert.deepStrictEqual([...js.matchAll(/https?:\/\/[^'"\s]+/g)].map((m) => m[0]).filter((u) => !/^https?:\/\/my\.home-assistant\.io\//.test(u)), []);
assert.strictEqual(phone.MY_HA, 'https://my.home-assistant.io/redirect/config_flow_start/?domain=esphome');
// the page only calls the server's documented API
const paths = new Set([...js.matchAll(/'(\/api\/[a-z/]*)/g)].map((m) => m[1]));
assert.deepStrictEqual([...paths].sort(), ['/api/actions', '/api/actions/', '/api/session', '/api/session/end', '/api/state']);

console.log('phone_logic: ok');
