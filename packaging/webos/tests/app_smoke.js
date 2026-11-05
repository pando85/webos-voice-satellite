// Host smoke test of the TV app (packaging/webos/app/app.js) with a minimal
// fake DOM and a fake PalmServiceBridge (tests/app_fake.js). Checks that the
// app sends only whitelisted ctl.sh commands through the Homebrew Channel's
// exec; Home (cards, notice, tiles), focus by direction, Back to Home; the
// Home Assistant and Diagnostics screens and More actions; a pending update
// applied once; two presses for destructive actions; the compatibility
// notices; the report QR code (the module matrix drawn on the canvas must
// equal the encoder's own and have a valid QR structure, and, when zbarimg is
// installed, decode to exactly the report URL); and the phone setup
// (docs/design/phone-setup.md, ADR 0009): the QR from the app's address and
// the session path, the poll from every screen while a session lives (2 s, 1 s while a
// request is pending), the 45 s hint, the confirmation overlay (OK allows, Back refuses,
// 60 s timeout refuses, unknown requests refused without asking, the overlay
// wins over an armed button), the microphone off that runs without the
// overlay, the result lines, Done ending the session, and ended sessions.
// usage: node packaging/webos/tests/app_smoke.js
'use strict';
const fs = require('fs');
const path = require('path');
const os = require('os');
const cp = require('child_process');
const { load, checker, APP } = require('./app_fake.js');

const { check, done } = checker();
const VP = require('../app/voicepower.js');

const REPORT_URL = 'https://github.com/pando85/webos-voice-satellite/issues/new?template=compatibility-report.yml&title='
  + encodeURIComponent('[Compat] OLED42C34LA fw 03.30.74: works') + '&report=' + encodeURIComponent(
    'webos-satellite compatibility report v1\nresult: works\napp: version=0.1.1, arch=armv7\ntv: model=OLED42C34LA, webos=8.3.0, fw=03.30.74\n'
    + 'checks: C1=pass, C2=pass, C3=pass, C4=pass, C5=pass, C6=pass, C7=pass, C8=skipped\nlevels: window=10s, rms=-67.6, peak=-53.6, clipped=0\n'.repeat(5));
const REPORT = { schema: 1, verdict: 'works', title: '[Compat] OLED42C34LA fw 03.30.74: works', short: 'short text', full: 'FULL REPORT\nline two', url: REPORT_URL };
const TOKEN_PATH = '/s/q3V2kR9xYb7Lm2Pn5Wc8Df1Gh4Jk6Mn0Qs3Tv5Xz7Ab';
let status = { uid: 0, installed: false, installed_version: '', package_version: '0.0.2', update_pending: false,
  running: false, pid: null, supervised: false, autostart: false, disabled: false, crashloop: false,
  mic_off: false, firmware: 'firmware 03.30.74', firmware_ok: false, key: 'none', name: '', port: '6053',
  prefix: '/var/lib/webos-satellite', machine: 'aarch64', build: '', log: [] };
const tv = { schema: 1, luna: true, daemon_running: true, always_ready: { state: 'off', raw: 'off', error: '' },
  hi_lg: { state: 'off', thinq: false, amazon: false, dangbei: false, error: '' }, remote_off: { state: 'normal', active: true },
  mic: { state: 'listening', kill_switch: false }, power_state: 'Active', last_change: null };

// The fake setup server behind `ctl.sh setup ...`.
const NONE = { schema: 1, state: 'none', reason: '', port: null, expires_in_s: null, peer: null, peer_other_subnet: null, pending: null, last_result: null };
const srv = { s: Object.assign({}, NONE), startFails: '' };
const answers = [];
function exec(args) {
  if (args === 'status --json') return JSON.stringify(status) + '\n';
  if (args === 'tv-settings --json') return JSON.stringify(tv) + '\n';
  if (args === 'report --json') return JSON.stringify(REPORT) + '\n';
  if (args === 'setup start') {
    if (srv.startFails) return 'ctl.sh: ' + srv.startFails + '\n';
    srv.s = Object.assign({}, NONE, { state: 'waiting', port: 49731, path: TOKEN_PATH, expires_in_s: 600 });
    return JSON.stringify(srv.s) + '\n';
  }
  if (args === 'setup status --json') return JSON.stringify(srv.s) + '\n';
  if (args === 'setup stop') { srv.s = Object.assign({}, NONE); return 'ctl.sh: stopping\n' + JSON.stringify(srv.s) + '\n'; }
  let m = /^setup answer ([0-9a-f]{8}) (allow|refuse)$/.exec(args);
  if (m) { answers.push(m[1] + ' ' + m[2]); return 'ctl.sh: answer recorded\n'; }
  return 'webos-satellite: done\n';
}
const T = load({ exec });
const { byId, doc, settle, advance, step, execs, key, click, ok, calls } = T;
const ALLOWED = /^sh '\/media\/developer\/apps\/usr\/palm\/applications\/io\.github\.pando85\.voicesatellite\/payload\/ctl\.sh' (status --json|install|start|stop|autostart on|autostart off|mic off|mic on|pair|reset-pairing|clear-crashloop|uninstall|purge|report --json|selfcheck|ack-firmware|tv-settings --json|setup start|setup status --json|setup stop|setup answer [0-9a-f]{8} (allow|refuse)) 2>&1$/;
const shown = (ids) => ids.filter((id) => !byId[id].hidden).sort().join();
const polls = () => execs().filter((c) => c === 'setup status --json').length;
// The 10 s refresh (the status read) without waiting for it.
const refresh = async () => { const t = T.timers.filter((x) => x.live && x.interval && x.ms === 10000)[0]; advance(t.at - T.clock.now); await settle(); };

(async () => {
  await settle();
  // ---- not installed ----
  check('starting: one setup status read; no session, so no further polls', polls() === 1 && !T.timers.some((t) => t.live && !t.interval && t.ms === 1000));
  check('not installed: Home says so and offers Set up', !byId.home.hidden && !byId['h-notice'].hidden
    && /^Not set up yet/.test(byId['h-notice-text'].textContent) && byId['h-notice-btn'].textContent === 'Set up' && !byId['h-notice-btn'].hidden
    && byId['h-ha-value'].textContent === 'Not set up' && byId['h-voice'].hidden && !byId['h-mic'].hidden);
  check('address from the connection manager on the Home Assistant tile', byId['h-ha-sub'].textContent === '192.0.2.10, port 6053');
  check('Home: the focus starts on a tile; no Back hint on Home', doc.activeElement === byId['h-mic'] && byId['key-back-box'].hidden);
  key(38);
  check('Up from a tile reaches the notice button', doc.activeElement === byId['h-notice-btn']);
  ok();
  await settle();
  const ex = execs();
  check('Set up runs install then start', ex.indexOf('install') >= 0 && ex.indexOf('start') > ex.indexOf('install'));

  // ---- installed, running, unpaired, update pending ----
  status = Object.assign({}, status, { installed: true, installed_version: '0.0.1', update_pending: true, running: true,
    pid: 42, supervised: true, key: 'unpaired', firmware_ok: true, name: 'webos-satellite-abc123',
    remote_off: 'keep_listening', reply_volume: '40' });
  calls.length = 0;
  await refresh();
  check('pending update applied once on open', execs().filter((c) => c === 'install').length === 1);
  await refresh();
  check('a second refresh does not apply it again', execs().filter((c) => c === 'install').length === 1);
  status = Object.assign({}, status, { update_pending: false, installed_version: '0.0.2' });
  await refresh();
  check('Home cards: listening, Home Assistant, power button', byId['h-listen-value'].textContent === 'Yes'
    && byId['h-ha-value'].textContent === 'Not known' && byId['h-power-value'].textContent === 'The TV goes to deep standby.'
    && /tone-ok/.test(byId['h-listen-card'].className) && /live/.test(byId['h-listen-dot'].className));
  const haCard = async (ha, extra) => {
    status = Object.assign({}, status, { ha: ha, pairing: undefined }, extra || {});
    await refresh();
    return byId['h-ha-value'].textContent + '|' + /tone-(\w+)/.exec(byId['h-ha-card'].className)[1] + '|' + byId['h-ha-detail'].textContent;
  };
  check('Home Assistant card: no ha object (older daemon) is Not known', (await haCard(undefined)) === 'Not known|idle|' + VP.HA_CONN.unknown.full);
  check('Home Assistant card: connected', (await haCard({ state: 'connected', since_s: 30 })) === 'Connected|ok|' + VP.HA_CONN.connected.full);
  check('Home Assistant card: disconnected', (await haCard({ state: 'disconnected', since_s: 30 })) === 'Disconnected|warn|' + VP.HA_CONN.disconnected.full);
  check('Home Assistant card: unpaired', (await haCard({ state: 'unpaired', since_s: 30 })) === 'Not paired|warn|' + VP.HA_CONN.unpaired.full);
  check('Home Assistant card: an open pairing window wins and counts down', /^Waiting for pairing\|accent\|Add it in Home Assistant\. \d+:\d\d left\.$/.test(
    await haCard({ state: 'unpaired', since_s: 30 }, { pairing: { open: true, ends_in_s: 300 } })));
  check('Home Assistant card: back to the connection when the window closes', (await haCard({ state: 'connected', since_s: 3 }, { pairing: { open: false, ends_in_s: null } })) === 'Connected|ok|' + VP.HA_CONN.connected.full);
  status = Object.assign({}, status, { ha: undefined, pairing: undefined });
  await refresh();
  check('four tiles', ['h-mic', 'h-voice', 'h-ha', 'h-diag'].every((id) => !byId[id].hidden) && byId['h-mic-pill'].textContent === 'On');
  byId['h-mic'].focus();
  key(39); const r1 = doc.activeElement.id; key(39); const r2 = doc.activeElement.id; key(39); const r3 = doc.activeElement.id; key(39); const r4 = doc.activeElement.id;
  key(37);
  check('Right moves along the tiles and stops at the last; Left goes back', [r1, r2, r3, r4].join() === 'h-voice,h-ha,h-diag,h-diag' && doc.activeElement === byId['h-ha']);
  calls.length = 0;
  click('h-mic');
  await settle();
  check('Microphone tile: one press runs mic off and says so', execs()[0] === 'mic off' && byId['h-msg'].textContent === 'Microphone off. The satellite cannot hear.');

  // ---- Home Assistant screen ----
  click('h-ha');
  await settle();
  check('Home Assistant: not paired, address and port, Start pairing and Set up with your phone', !byId.ha.hidden && byId['ha-value'].textContent === 'Not paired'
    && byId['ha-addr'].textContent === '192.0.2.10' && byId['ha-port'].textContent === '6053'
    && shown(['a-pair', 'a-phone', 'a-reset', 'a-endphone']) === 'a-pair,a-phone' && doc.activeElement === byId['a-pair']
    && byId.crumb.textContent === '/ Home Assistant');
  calls.length = 0;
  ok();
  await settle();
  check('Start pairing runs ctl.sh pair', execs()[0] === 'pair' && /Pairing is open for 10 minutes/.test(byId['ha-msg'].textContent));
  status = Object.assign({}, status, { pairing: { open: true, ends_in_s: 590 } });
  await step(10000);
  check('an open pairing window: countdown, no second Start pairing', !byId['ha-count'].hidden && /^9:[45]\d$/.test(byId['ha-count-time'].textContent)
    && byId['a-pair'].hidden && byId['ha-value'].textContent === 'Waiting for pairing');
  status = Object.assign({}, status, { key: 'paired', pairing: { open: false, ends_in_s: null } });
  await step(10000);
  check('paired: Reset pairing offered, two presses', !byId['a-reset'].hidden && byId['a-pair'].hidden);
  calls.length = 0;
  byId['a-reset'].focus();
  ok();
  check('Reset pairing needs a second press', execs().length === 0 && byId['a-reset-t'].textContent === 'Press OK again' && /armed/.test(byId['a-reset'].className));
  advance(5000);
  check('the arming ends after 5 s', byId['a-reset-t'].textContent === 'Reset pairing');
  key(461);
  await settle();
  check('Back returns to Home with the focus on the tile', !byId.home.hidden && byId.ha.hidden && doc.activeElement === byId['h-ha']);

  // ---- Diagnostics, More actions, compatibility ----
  const compat = (o) => Object.assign({ schema: 1, compat_status: 'not verified', selfcheck: 'passed (peak -54 dBFS)', tv_model: 'OLED42C34LA',
    tv_firmware: '03.30.99', firmware_verified: false, firmware_changed: false, capture_problem: false, policy: 'warn', report_status: 'idle' }, o);
  status = Object.assign({}, status, { firmware_ok: true, compat: compat({}) });
  await refresh();
  check('not verified: a short notice on Home, no button', !byId['h-notice'].hidden && /not verified yet/.test(byId['h-notice-text'].textContent)
    && byId['h-notice-btn'].hidden && byId['h-diag-sub'].textContent === 'Firmware: not verified');
  click('h-diag');
  await settle();
  check('Diagnostics: facts, tag, the full text and the actions', !byId.diag.hidden && byId['d-fw'].textContent === 'firmware 03.30.74'
    && byId['d-fw-tag'].textContent === 'Not verified' && byId['d-check-tag'].textContent === 'Passed' && /runs anyway/.test(byId['d-compat'].textContent)
    && /webos-satellite-abc123, reply volume 40 %/.test(byId['d-device'].textContent) && /^OLED42C34LA, aarch64$/.test(byId['d-tv'].textContent)
    && shown(['d-check', 'd-report', 'd-ack', 'd-more']) === 'd-check,d-more,d-report' && /lead/.test(byId['d-report'].className));
  check('no "microphone stays off" text for an unverified firmware', !/stays off/.test(byId['h-notice-text'].textContent));
  key(40);
  check('Down moves from Re-run to Report', doc.activeElement === byId['d-report']);
  status = Object.assign({}, status, { compat: compat({ compat_status: 'firmware changed', firmware_changed: true }) });
  await step(10000);
  check('firmware changed: Acknowledge offered', !byId['d-ack'].hidden && byId['d-fw-tag'].textContent === 'Firmware changed');
  calls.length = 0;
  click('d-ack');
  await settle();
  check('Acknowledge runs ctl.sh ack-firmware', execs().indexOf('ack-firmware') >= 0);
  status = Object.assign({}, status, { compat: compat({ compat_status: 'capture problem', capture_problem: true, selfcheck: 'failed: no audio frames (will retry)' }) });
  await step(10000);
  check('capture problem: the reason on Diagnostics, Failed tag', /microphone check failed \(failed: no audio frames/.test(byId['d-compat'].textContent)
    && byId['d-check-tag'].textContent === 'Failed' && /bad/.test(byId['d-check-tag'].className));
  calls.length = 0;
  click('d-check');
  await settle();
  check('Re-run runs ctl.sh selfcheck', execs().indexOf('selfcheck') >= 0);

  click('d-more');
  await settle();
  check('More actions: stop, start at boot on, remove (twice), purge', !byId.more.hidden
    && shown(['x-stop', 'x-start', 'x-boot-on', 'x-boot-off', 'x-crash', 'x-uninstall', 'x-purge']) === 'x-boot-on,x-purge,x-stop,x-uninstall');
  calls.length = 0;
  click('x-purge');
  await settle();
  check('purge needs a second press', execs().length === 0 && byId['x-purge-t'].textContent === 'Press OK again');
  click('x-purge');
  await settle();
  check('second press runs purge', execs().indexOf('purge') >= 0);

  // ---- report ----
  status = Object.assign({}, status, { compat: compat({ compat_status: 'verified', firmware_verified: true }) });
  click('h-diag');
  await settle();
  calls.length = 0;
  click('d-report');
  await settle();
  check('Report compatibility runs ctl.sh report --json', execs().indexOf('report --json') >= 0);
  check('report screen shows the URL, title and text', !byId.report.hidden && byId.diag.hidden
    && byId['r-url'].textContent === REPORT_URL && byId['r-text'].textContent.startsWith('FULL REPORT')
    && byId['r-title'].textContent.includes('OLED42C34LA') && doc.activeElement === byId['r-back']);
  check('the URL fits one QR code and the report URL is under 1500 characters', REPORT_URL.length < 1500);
  const matrix = (canvas, border) => {
    const ops = canvas.ops.slice(canvas.ops.map((o) => o.fs).lastIndexOf('#ffffff'));
    const full = ops[0];
    const dark = ops.slice(1).filter((o) => o.fs === '#000000');
    const scale = dark[0].w;
    const size = full.w / scale - 2 * border;
    const drawn = Array.from({ length: size }, () => new Array(size).fill(false));
    for (const o of dark) drawn[o.y / scale - border][o.x / scale - border] = true;
    return { drawn, size, scale, full };
  };
  const sameAs = (m, text, ecc) => {
    const ref = T.window.qrcodegen.QrCode.encodeText(text, ecc);
    if (ref.size !== m.size) return false;
    for (let y = 0; y < m.size; y++) for (let x = 0; x < m.size; x++) if (m.drawn[y][x] !== ref.getModule(x, y)) return false;
    return true;
  };
  const rq = matrix(byId['r-qr'], 2);
  check(`report QR drawn on the canvas equals the encoder's matrix (version ${(rq.size - 17) / 4}, scale ${rq.scale} px)`,
    sameAs(rq, REPORT_URL, T.window.qrcodegen.QrCode.Ecc.LOW) && rq.full.w === rq.full.h && rq.full.w <= 552);
  const finder = (d, ox, oy) => {
    for (let dy = 0; dy < 7; dy++) for (let dx = 0; dx < 7; dx++) {
      const edge = Math.max(Math.abs(dx - 3), Math.abs(dy - 3));
      if (d[oy + dy][ox + dx] !== (edge !== 2)) return false;
    }
    return true;
  };
  const structure = (m) => (m.size - 17) % 4 === 0 && finder(m.drawn, 0, 0) && finder(m.drawn, m.size - 7, 0) && finder(m.drawn, 0, m.size - 7)
    && Array.from({ length: m.size - 16 }, (_, i) => i + 8).every((i) => m.drawn[6][i] === (i % 2 === 0) && m.drawn[i][6] === (i % 2 === 0));
  check('report QR structure: three finder patterns and the timing patterns', structure(rq));
  const zbar = (m) => {
    try {
      const px = 4;
      const dim = (m.size + 8) * px;
      const img = Buffer.alloc(dim * dim, 255);
      for (let y = 0; y < m.size; y++) for (let x = 0; x < m.size; x++) if (m.drawn[y][x])
        for (let dy = 0; dy < px; dy++) for (let dx = 0; dx < px; dx++) img[((y + 4) * px + dy) * dim + (x + 4) * px + dx] = 0;
      const f = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'qr-')), 'qr.pgm');
      fs.writeFileSync(f, Buffer.concat([Buffer.from(`P5\n${dim} ${dim}\n255\n`), img]));
      return cp.execFileSync('zbarimg', ['--raw', '-q', f], { encoding: 'utf8', timeout: 20000 }).replace(/\n$/, '');
    } catch (e) { return null; }
  };
  const z1 = zbar(rq);
  if (z1 === null) console.log('SKIP  independent decode (zbarimg not installed or failed to read the image)');
  else check('independent decoder (zbarimg) reads exactly the report URL from the drawn QR', z1 === REPORT_URL);
  byId['r-text'].scrollTop = 0;
  key(40);
  check('down key scrolls the report text', byId['r-text'].scrollTop > 0);
  click('r-back');
  check('Done returns to Diagnostics', !byId.diag.hidden && byId.report.hidden && doc.activeElement === byId['d-report']);
  key(27);
  await settle();
  check('Back returns to Home', !byId.home.hidden && byId.diag.hidden);

  // ---- phone setup: start and the QR ----
  status = Object.assign({}, status, { compat: compat({ compat_status: 'verified', firmware_verified: true }) });
  click('h-ha');
  await settle();
  check('Set up with your phone is offered (lead while paired)', !byId['a-phone'].hidden && /lead/.test(byId['a-phone'].className) && byId['a-endphone'].hidden);
  calls.length = 0;
  click('a-phone');
  check('the phone screen opens at once and says it is starting', !byId.setup.hidden && byId['q-noqr'].textContent === 'Starting...');
  await settle();
  const setupUrl = 'http://192.0.2.10:49731' + TOKEN_PATH;
  check('setup start: QR shown with the URL from the app\'s address and the session path', execs()[0] === 'setup start'
    && byId['q-url'].textContent === setupUrl && byId['q-noqr'].hidden && !byId['q-count'].hidden && byId['q-time'].textContent === '10:00'
    && doc.activeElement === byId['q-done'] && byId['chip-phone-text'].textContent === 'Phone code open' && !byId['chip-phone'].hidden);
  const sq = matrix(byId['q-qr'], 2);
  check('setup QR on the canvas equals the encoder\'s matrix of the URL (medium error correction)', sameAs(sq, setupUrl, T.window.qrcodegen.QrCode.Ecc.MEDIUM)
    && structure(sq) && sq.scale >= 8);
  const z2 = zbar(sq);
  if (z2 === null) console.log('SKIP  independent decode of the setup QR (zbarimg not installed)');
  else check('independent decoder (zbarimg) reads exactly the setup URL', z2 === setupUrl);
  check('the token path is never in a command line', calls.every((c) => !JSON.stringify(c.p).includes(TOKEN_PATH.slice(3))));
  calls.length = 0;
  await step(6000);
  check('poll: setup status --json every 2 s while waiting (EXP-012)', polls() === 3);
  key(461);
  await settle();
  calls.length = 0;
  await step(4000);
  check('the poll goes on from Home (it is the heartbeat), the chip shows the session', polls() === 2 && !byId['chip-phone'].hidden);
  click('h-ha');
  await settle();
  check('Home Assistant offers the code again and End phone session', byId['a-phone-t'].textContent === 'Show the phone code' && !byId['a-endphone'].hidden);
  calls.length = 0;
  click('a-phone');
  await settle();
  check('showing the code again does not start a second server', execs().indexOf('setup start') < 0 && !byId.setup.hidden);
  check('no hint before 45 s', byId['q-hint'].hidden);
  await step(40000, 5000);
  check('after 45 s with no phone: the network hint with the address and port', !byId['q-hint'].hidden
    && /same network as this TV.*This TV: 192\.0\.2\.10 \(port 49731\)\./.test(byId['q-hint'].textContent));

  // ---- a phone connects ----
  srv.s = Object.assign({}, srv.s, { state: 'active', path: undefined, peer: '192.0.2.77', expires_in_s: 520 });
  delete srv.s.path;
  await step(2000);
  check('active: QR marked used, the phone named, the chip says connected', /used/.test(byId['q-box'].className) && !byId['q-used'].hidden
    && byId['q-conn'].textContent === 'Phone 192.0.2.77 connected. Session ends in 8:40.' && byId['chip-phone-text'].textContent === 'Phone connected'
    && byId['q-hint'].hidden && byId['q-other'].hidden && byId['q-url'].textContent === '');
  srv.s.peer_other_subnet = true;
  await step(2000);
  check('a phone from another network is pointed out', !byId['q-other'].hidden && /different network \(192\.0\.2\.77\)/.test(byId['q-other'].textContent));
  srv.s.peer_other_subnet = false;

  // ---- the confirmation overlay ----
  const pend = (id, action, value, o) => Object.assign({ id, action, value, peer: '192.0.2.77', state: 'pending', confirm: true, timeout_in_s: 60 }, o || {});
  key(461); await settle();
  click('h-voice'); await settle();
  byId['v-row-hl'].focus();
  click('v-row-hl');
  check('(a Voice and power row is armed)', byId['v-hl-state'].textContent === 'Press OK again');
  srv.s.pending = pend('a1b2c3d4', 'hi-lg', 'off');
  await step(2000);
  check('a phone request shows the overlay over any screen, in the app\'s own words', !byId.overlay.hidden
    && byId['m-from'].textContent === 'Phone 192.0.2.77 wants to:' && byId['m-what'].textContent === 'turn Hi LG off'
    && byId['m-detail'].textContent === 'The TV stops answering "Hi LG". The satellite can use the microphone.'
    && doc.activeElement === byId['m-allow'] && byId['key-ok'].textContent === 'Allow' && byId['key-back'].textContent === 'Refuse');
  calls.length = 0;
  await step(4000, 1000);
  check('while a request is pending the poll runs every second', polls() === 4 && !byId.overlay.hidden);
  check('the overlay wins: the armed row is disarmed', byId['v-hl-state'].textContent !== 'Press OK again' && byId['v-hl-confirm'].hidden);
  key(40); key(38);
  check('arrows stay inside the overlay', doc.activeElement === byId['m-allow']);
  key(39);
  check('Right moves to Refuse, Left back to Allow', doc.activeElement === byId['m-refuse'] && (key(37), doc.activeElement === byId['m-allow']));
  click('v-row-hl');
  check('a row cannot be pressed under the overlay', byId['v-hl-state'].textContent !== 'Press OK again');
  calls.length = 0;
  ok();
  await settle();
  check('OK allows: setup answer ID allow, the overlay closes, the focus returns', answers.join() === 'a1b2c3d4 allow' && byId.overlay.hidden
    && doc.activeElement === byId['v-row-hl'] && execs().indexOf('setup answer a1b2c3d4 allow') >= 0);
  await step(2000);
  check('the same pending request does not reopen the overlay', byId.overlay.hidden && answers.length === 1);
  srv.s.pending = Object.assign({}, srv.s.pending, { state: 'running' });
  await step(2000);
  check('running: no overlay, a changing line', byId.overlay.hidden && byId['h-msg'].textContent === 'From the phone: turn Hi LG off. Changing...');
  srv.s.pending = null;
  srv.s.last_result = { id: 'a1b2c3d4', action: 'hi-lg', value: 'off', verdict: 'allowed', result: 'ok', error: '', peer: '192.0.2.77' };
  calls.length = 0;
  await step(2000);
  check('the result line of an allowed request, and the screens read again', byId['v-message'].textContent === 'From the phone: Hi LG is off.'
    && byId['h-msg'].textContent === 'From the phone: Hi LG is off.' && execs().indexOf('tv-settings --json') >= 0 && execs().indexOf('status --json') < 0);
  await step(2000);
  check('a result is shown once (compared by id)', byId['h-msg'].textContent === 'From the phone: Hi LG is off.');

  // Back refuses
  key(461); await settle();
  srv.s.pending = pend('0badf00d', 'always-ready', 'on');
  await step(2000);
  check('second request: overlay', !byId.overlay.hidden && byId['m-what'].textContent === 'turn Always Ready on');
  key(461);
  await settle();
  check('Back refuses (and does not leave the screen)', answers[answers.length - 1] === '0badf00d refuse' && byId.overlay.hidden && !byId.home.hidden);
  srv.s.pending = null;
  srv.s.last_result = { id: '0badf00d', action: 'always-ready', value: 'on', verdict: 'refused', result: '', error: '', peer: '192.0.2.77' };
  await step(2000);
  check('refused result line', byId['h-msg'].textContent === 'Refused the phone: turn Always Ready on. Nothing changed.');

  // the 60 s timeout refuses (the server counts down too)
  click('h-ha'); await settle();
  byId['a-reset'].focus(); ok();
  check('(Reset pairing is armed)', byId['a-reset-t'].textContent === 'Press OK again');
  srv.s.pending = pend('c0ffee00', 'reply-volume', '0');
  await step(2000);
  check('volume request: overlay with the silent warning; the armed button is disarmed', !byId.overlay.hidden
    && byId['m-what'].textContent === 'set the reply volume to 0 %' && byId['m-detail'].textContent === 'Replies and timer alarms will be silent.'
    && byId['a-reset-t'].textContent === 'Reset pairing' && /^(1:00|0:59)$/.test(byId['m-time'].textContent));
  let t = 0;
  for (; t < 55; t++) { srv.s.pending.timeout_in_s = 58 - t; await step(1000); }
  check('the overlay counts down and is still open before 60 s', !byId.overlay.hidden && /^0:0[1-4]$/.test(byId['m-time'].textContent) && answers.indexOf('c0ffee00 refuse') < 0);
  await step(5000, 1000);
  check('after 60 s with no answer the app refuses once and says so', answers.filter((a) => a === 'c0ffee00 refuse').length === 1 && byId.overlay.hidden
    && byId['ha-msg'].textContent === 'No answer on the TV: the phone\'s request was refused.' && doc.activeElement === byId['a-reset']);
  srv.s.pending = null;
  srv.s.last_result = { id: 'c0ffee00', action: 'reply-volume', value: '0', verdict: 'expired', result: '', error: '', peer: '192.0.2.77' };
  await step(2000);

  // requests the app cannot put into words are refused without asking
  for (const [id, p] of [['11111111', { action: 'name', value: 'x' }], ['22222222', { action: 'hi-lg', value: 'maybe' }], ['33333333', { peer: 'evil.example' }],
    ['44444444', { action: 'reply-volume', value: '101' }]]) {
    srv.s.pending = pend(id, 'hi-lg', 'on', p);
    await step(2000);
    check(`unknown request (${JSON.stringify(p)}) refused without an overlay`, byId.overlay.hidden && answers.indexOf(id + ' refuse') >= 0
      && answers.filter((a) => a.startsWith(id)).length === 1);
  }
  const before = answers.length;
  srv.s.pending = pend('A1B2C3D4', 'hi-lg', 'on');
  await step(2000);
  srv.s.pending = pend("1234567'", 'hi-lg', 'on');
  await step(2000);
  check('a malformed id is neither shown nor answered', byId.overlay.hidden && answers.length === before);
  srv.s.pending = pend('55555555', 'hi-lg', 'on', { confirm: false, state: 'pending' });
  await step(2000);
  check('a request without confirm is not shown', byId.overlay.hidden && answers.length === before);

  // microphone off from the phone: runs without the TV
  srv.s.pending = { id: '66666666', action: 'mic', value: 'off', peer: '192.0.2.77', state: 'running', confirm: false, timeout_in_s: null };
  await step(2000);
  check('mic off from the phone: no overlay, no answer', byId.overlay.hidden && answers.length === before);
  srv.s.pending = null;
  srv.s.last_result = { id: '66666666', action: 'mic', value: 'off', verdict: 'auto', result: 'ok', error: '', peer: '192.0.2.77' };
  status = Object.assign({}, status, { mic_off: true });
  calls.length = 0;
  await step(2000);
  check('mic off from the phone: a short notice; the microphone shows off without an extra status read', byId['ha-msg'].textContent === 'Microphone turned off from the phone.'
    && byId['h-mic-pill'].textContent === 'Off' && execs().indexOf('status --json') < 0);

  // ---- Done ends the session ----
  click('a-phone');
  await settle();
  calls.length = 0;
  click('q-done');
  await settle();
  check('Done runs setup stop and returns to Home Assistant', execs()[0] === 'setup stop' && !byId.ha.hidden && byId['a-endphone'].hidden
    && byId['chip-phone'].hidden && byId['ha-msg'].textContent === 'The phone session ended.');
  calls.length = 0;
  await step(5000);
  check('no polling after the session ended', polls() === 0);

  // ---- a session that ends by itself ----
  click('a-phone');
  await settle();
  check('a new session starts from the button', !byId.setup.hidden && byId['q-url'].textContent === setupUrl);
  srv.s = Object.assign({}, NONE, { state: 'ended', reason: 'idle' });
  await step(2000);
  check('ended by the server: the reason, Start again, no QR', byId['q-state'].textContent === 'The phone session ended: the phone was idle for 3 minutes.'
    && !byId['q-again'].hidden && byId['q-urlbox'].hidden && byId['q-noqr'].textContent === 'No code.' && byId['chip-phone'].hidden);
  calls.length = 0;
  await step(3000);
  check('no polling after an end', polls() === 0);
  calls.length = 0;
  click('q-done');
  await settle();
  check('Done on an ended session needs no command', execs().length === 0 && !byId.ha.hidden);

  // ---- start fails; no address ----
  srv.startFails = 'cannot create /tmp/webos-satellite-setup';
  click('a-phone');
  await settle();
  check('a failed start says why and offers Start again', /^The phone setup could not start: ctl\.sh: cannot create/.test(byId['q-state'].textContent)
    && !byId['q-again'].hidden && byId['q-urlbox'].hidden);
  srv.startFails = '';
  click('q-done');
  await settle();
  // An ipk built without the server: the same refusal, shown as a plain sentence, no raw error, no Start again.
  srv.startFails = 'the setup server is missing: /var/lib/webos-satellite/bin/webos-satellite-setup (reinstall the package)';
  click('a-phone');
  await settle();
  check('a missing setup server says "Phone setup is not available in this version", not the raw error', byId['q-state'].textContent === 'Phone setup is not available in this version.'
    && !/ctl\.sh|webos-satellite-setup|reinstall/.test(byId['q-state'].textContent) && /bad-line/.test(byId['q-state'].className) && byId['q-again'].hidden
    && byId['q-urlbox'].hidden && !T.timers.some((t) => t.live && t.ms === 2000));
  click('q-done');
  await settle();
  check('Home Assistant screen: the phone action says it is not available, Home Assistant stays usable', byId['a-phone-sub'].textContent === 'Not available in this version'
    && !byId['a-phone'].hidden && byId['chip-phone'].hidden);
  srv.startFails = '';
  click('a-phone');
  await settle();
  check('the next press tries again (the package may have been reinstalled) and works', !byId.setup.hidden && byId['q-url'].textContent === setupUrl
    && byId['a-phone-sub'].textContent !== 'Not available in this version');
  click('q-done');
  await settle();
  srv.s = Object.assign({}, NONE);

  // ---- not root ----
  status = Object.assign({}, status, { uid: 1000 });
  key(461);
  await settle();
  check('not root: explains and hides the microphone and Voice and power tiles', /not running as root/.test(byId['h-notice-text'].textContent)
    && byId['h-mic'].hidden && byId['h-voice'].hidden);

  // ---- the whitelist ----
  check('every exec command is whitelisted', calls.length >= 0 && T.calls.filter((c) => c.uri.endsWith('/exec')).every((c) => ALLOWED.test(c.p.command)));
  check('the app refuses anything else, including a malformed answer', ['setup answer a1b2c3d4 allow', 'setup answer 0badf00d refuse', 'setup start'].every(T.app.allowed)
    && !['setup answer A1B2C3D4 allow', 'setup answer a1b2c3d4 yes', 'setup answer a1b2c3d4 allow; reboot', 'setup answer a1b2c3d allow', 'setup start now',
      'setup status', 'set reply-volume 40', 'reboot', ''].some(T.app.allowed));
  check('only the HBC exec and connection manager are called', T.calls.every((c) =>
    c.uri === 'luna://org.webosbrew.hbchannel.service/exec' || /connectionmanager\/getStatus$/.test(c.uri)));
  check('the poll intervals are constants: 2 s, and 1 s while a request is pending', T.app.SETUP_POLL_MS === 2000 && T.app.SETUP_POLL_FAST_MS === 1000 && T.app.REQUEST_MS === 60000);

  // ---- ES5 (no syntax newer than the TV's engine is known to run) ----
  const es5 = ['app.js', 'voicepower.js'].map((f) => {
    const code = fs.readFileSync(path.join(APP, f), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '')
      .replace(/'(?:[^'\\\n]|\\.)*'/g, "''").replace(/"(?:[^"\\\n]|\\.)*"/g, '""');
    return /=>|`|\blet\s|\bconst\s|\bclass\s|\.\.\.|\basync\b|\bawait\b/.test(code) ? f : '';
  }).filter(Boolean);
  check('app.js and voicepower.js use ES5 syntax only (no arrows, let, const, classes, templates, spread)', es5.length === 0, es5.join());
  check('index.html loads the shared model before the app', /<script src="voicepower\.js"><\/script>\s*<script src="app\.js">/.test(fs.readFileSync(path.join(APP, 'index.html'), 'utf8')));
  done();
})();
