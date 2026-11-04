// Host smoke test of the status app (packaging/webos/app/app.js) with a
// minimal fake DOM and a fake PalmServiceBridge. Checks that the app sends
// only whitelisted ctl.sh commands through the Homebrew Channel's exec,
// renders a status, shows the right buttons, applies a pending update once,
// needs a second press for destructive actions, shows the compatibility
// banner and its buttons, and draws the report QR code: the module matrix
// drawn on the canvas must equal the encoder's own and have a valid QR
// structure, and, when zbarimg is installed, decode to exactly the report URL.
// usage: node packaging/webos/tests/app_smoke.js
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const os = require('os');
const cp = require('child_process');

const APP = path.join(__dirname, '..', 'app');
const html = fs.readFileSync(path.join(APP, 'index.html'), 'utf8');

function el(id, tag) {
  return {
    id, tagName: tag, hidden: false, className: '', textContent: '', children: [], attrs: {},
    listeners: {},
    get firstChild() { return this.children[0] || null; },
    appendChild(c) { this.children.push(c); return c; },
    removeChild(c) { this.children.splice(this.children.indexOf(c), 1); },
    getAttribute(n) { return n in this.attrs ? this.attrs[n] : null; },
    setAttribute(n, v) { this.attrs[n] = String(v); },
    addEventListener(t, f) { this.listeners[t] = f; },
    focus() { doc.activeElement = this; },
    getElementsByTagName() { return this.children; },
    scrollTop: 0,
    style: {},
    getContext() {
      const ops = this.ops;
      return { set fillStyle(v) { this.fs = v; }, fillRect(x, y, w, h) { ops.push({ x, y, w, h, fs: this.fs }); } };
    },
    ops: [],
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
  body: el('body', 'body'),
  activeElement: null,
  keydown: null,
  getElementById: (id) => byId[id],
  createElement: (t) => el('', t),
  addEventListener(t, f) { this.keydown = f; },
};
doc.activeElement = doc.body;

const calls = [];
const REPORT_URL = 'https://github.com/pando85/webos-voice-satellite/issues/new?template=compatibility-report.yml&title='
  + encodeURIComponent('[Compat] OLED42C34LA fw 03.30.74: works') + '&report=' + encodeURIComponent(
    'webos-satellite compatibility report v1\nresult: works\napp: version=0.1.1, arch=armv7\ntv: model=OLED42C34LA, webos=8.3.0, fw=03.30.74\n'
    + 'checks: C1=pass, C2=pass, C3=pass, C4=pass, C5=pass, C6=pass, C7=pass, C8=skipped\nlevels: window=10s, rms=-67.6, peak=-53.6, clipped=0\n'.repeat(5));
const REPORT = { schema: 1, verdict: 'works', title: '[Compat] OLED42C34LA fw 03.30.74: works', short: 'short text', full: 'FULL REPORT\nline two', url: REPORT_URL };
let status = { uid: 0, installed: false, installed_version: '', package_version: '0.0.2', update_pending: false,
  running: false, pid: null, supervised: false, autostart: false, disabled: false, crashloop: false,
  mic_off: false, firmware: 'firmware 03.30.74', firmware_ok: false, key: 'none', name: '', port: '6053',
  prefix: '/var/lib/webos-satellite', machine: 'aarch64', build: '', log: [] };
class Bridge {
  call(uri, params) {
    const p = JSON.parse(params);
    calls.push({ uri, p });
    let reply;
    if (uri.endsWith('/exec')) {
      const out = / status --json /.test(p.command) ? JSON.stringify(status) + '\n'
        : / report --json /.test(p.command) ? JSON.stringify(REPORT) + '\n' : 'webos-satellite: done\n';
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
const timers = [];
// The vendored QR generator declares a global `qrcodegen`; share it with the app.
const qrCtx = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(APP, 'qrcodegen.js'), 'utf8'), qrCtx);
const qrcodegen = vm.runInContext('qrcodegen', qrCtx);
window.qrcodegen = qrcodegen;
vm.runInNewContext(fs.readFileSync(path.join(APP, 'app.js'), 'utf8'), {
  window, document: doc, JSON, setTimeout: (f) => timers.push(f), setInterval: () => 0,
  decodeURIComponent, String,
});

let fails = 0;
function check(name, ok) { console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}`); if (!ok) fails += 1; }
const tick = () => new Promise((r) => setImmediate(r));
const visible = () => byId.actions.children.filter((b) => !b.hidden).map((b) => b.id).sort();
const execs = () => calls.filter((c) => c.uri.endsWith('/exec')).map((c) => c.p.command);
const ALLOWED = /^sh '\/media\/developer\/apps\/usr\/palm\/applications\/io\.github\.pando85\.voicesatellite\/payload\/ctl\.sh' (status --json|install|start|stop|autostart on|autostart off|mic off|mic on|pair|reset-pairing|clear-crashloop|uninstall|purge|report --json|selfcheck|ack-firmware) 2>&1$/;

(async () => {
  await tick(); await tick();
  check('not installed: headline and Set up button', byId.headline.textContent === 'Not set up yet'
    && visible().join() === ['b-install', 'b-mic-off', 'b-refresh'].sort().join());
  check('address from the connection manager', JSON.stringify(byId.facts.children.map((c) => c.textContent)).includes('192.0.2.10, port 6053'));

  byId['b-install'].listeners.click();
  await tick(); await tick(); await tick(); await tick();
  const ex = execs();
  check('Set up runs install then start', ex.some((c) => / install 2>&1$/.test(c)) && ex.some((c) => / start 2>&1$/.test(c))
    && ex.findIndex((c) => / install /.test(c)) < ex.findIndex((c) => / start /.test(c)));

  status = Object.assign({}, status, { installed: true, installed_version: '0.0.1', update_pending: true, running: true,
    pid: 42, supervised: true, key: 'unpaired', firmware_ok: true, name: 'webos-satellite-abc123',
    remote_off: 'keep_listening', reply_volume: '40' });
  calls.length = 0;
  byId['b-refresh'].listeners.click();
  for (let i = 0; i < 6; i++) await tick();
  check('pending update applied once on open', execs().filter((c) => / install /.test(c)).length === 1);
  const facts = byId.facts.children.map((c) => c.textContent).join('|');
  check('reply volume and remote power key shown', facts.includes('Reply volume|40 %')
    && /Remote power key\|keeps listening: the TV stays on/.test(facts));
  check('running and unpaired: pairing, stop, boot, uninstall buttons', ['b-pair', 'b-stop', 'b-boot-on', 'b-uninstall', 'b-purge']
    .every((id) => !byId[id].hidden) && byId['b-start'].hidden && byId['b-reset'].hidden);

  calls.length = 0;
  byId['b-purge'].listeners.click();
  await tick();
  check('purge needs a second press', execs().length === 0 && byId['b-purge'].textContent === 'Press again to confirm');
  byId['b-purge'].listeners.click();
  await tick(); await tick();
  check('second press runs purge', execs().some((c) => / purge 2>&1$/.test(c)));

  // --- compatibility banner, buttons and the report screen ---
  const compat = (o) => Object.assign({ schema: 1, compat_status: 'not verified', selfcheck: 'passed (peak -54 dBFS)', tv_model: 'OLED42C34LA',
    tv_firmware: '03.30.99', firmware_verified: false, firmware_changed: false, capture_problem: false, policy: 'warn', report_status: 'idle' }, o);
  const refresh = async () => { byId['b-refresh'].listeners.click(); for (let i = 0; i < 6; i++) await tick(); };
  const facts2 = () => byId.facts.children.map((c) => c.textContent).join('|');
  status = Object.assign({}, status, { update_pending: false, installed_version: '0.0.2', firmware_ok: true, compat: compat({}) });
  await refresh();
  check('not verified: banner says the mic runs anyway and offers the report', !byId.compat.hidden
    && /not in the project's verified list/.test(byId['compat-text'].textContent) && /runs anyway/.test(byId['compat-text'].textContent)
    && !byId['b-report'].hidden && !byId['b-selfcheck'].hidden && byId['b-ack'].hidden);
  check('compatibility facts shown', /Compatibility\|not verified\|TV model\|OLED42C34LA\|Microphone check\|passed/.test(facts2()));
  check('no "microphone stays off" text for an unverified firmware', !/stays off/.test(byId.detail.textContent));

  status = Object.assign({}, status, { compat: compat({ compat_status: 'firmware changed', firmware_changed: true }) });
  await refresh();
  check('firmware changed: banner and Acknowledge button', /firmware changed/.test(byId['compat-text'].textContent) && !byId['b-ack'].hidden);
  calls.length = 0;
  byId['b-ack'].listeners.click();
  await tick(); await tick();
  check('Acknowledge runs ctl.sh ack-firmware', execs().some((c) => / ack-firmware 2>&1$/.test(c)));

  status = Object.assign({}, status, { compat: compat({ compat_status: 'capture problem', capture_problem: true, selfcheck: 'failed: no audio frames (will retry)' }) });
  await refresh();
  check('capture problem: banner with the reason, re-run and report buttons', /microphone check failed \(failed: no audio frames/.test(byId['compat-text'].textContent)
    && byId.compat.className.includes('bad') && !byId['b-selfcheck'].hidden && !byId['b-report'].hidden);
  calls.length = 0;
  byId['b-selfcheck'].listeners.click();
  await tick(); await tick();
  check('Re-run runs ctl.sh selfcheck', execs().some((c) => / selfcheck 2>&1$/.test(c)));

  status = Object.assign({}, status, { compat: compat({ compat_status: 'verified', firmware_verified: true }) });
  await refresh();
  check('verified: no banner', byId.compat.hidden);

  calls.length = 0;
  byId['b-report'].listeners.click();
  await tick(); await tick(); await tick();
  check('Report compatibility runs ctl.sh report --json', execs().some((c) => / report --json 2>&1$/.test(c)));
  check('report screen replaces the main screen and shows the URL, title and text', byId.main.hidden && !byId['report-view'].hidden
    && byId['report-url'].textContent === REPORT_URL && byId['report-text'].textContent.startsWith('FULL REPORT')
    && byId['report-title'].textContent.includes('OLED42C34LA') && doc.activeElement === byId['r-back']);
  check('the URL fits one QR code and the report URL is under 1500 characters', REPORT_URL.length < 1500);

  // The matrix the app drew on the canvas.
  const ops = byId.qr.ops;
  const full = ops[0];
  const dark = ops.slice(1).filter((o) => o.fs === '#000000');
  const scale = dark[0].w;
  const cells = full.w / scale;
  const size = cells - 8;
  const drawn = Array.from({ length: size }, () => new Array(size).fill(false));
  for (const o of dark) drawn[o.y / scale - 4][o.x / scale - 4] = true;
  const ref = qrcodegen.QrCode.encodeText(REPORT_URL, qrcodegen.QrCode.Ecc.LOW);
  let same = ref.size === size;
  for (let y = 0; same && y < size; y++) for (let x = 0; x < size; x++) if (drawn[y][x] !== ref.getModule(x, y)) { same = false; break; }
  check(`QR drawn on the canvas equals the encoder's matrix (version ${(size - 17) / 4}, ${size}x${size}, scale ${scale} px)`, same && Number.isInteger(cells) && full.w === full.h && full.w <= 640);
  const finder = (ox, oy) => { // 7x7 finder: ring, light ring, 3x3 core
    for (let dy = 0; dy < 7; dy++) for (let dx = 0; dx < 7; dx++) {
      const edge = Math.max(Math.abs(dx - 3), Math.abs(dy - 3));
      if (drawn[oy + dy][ox + dx] !== (edge !== 2)) return false;
    }
    return true;
  };
  check('QR structure: three finder patterns and the timing patterns', (size - 17) % 4 === 0 && finder(0, 0) && finder(size - 7, 0) && finder(0, size - 7)
    && Array.from({ length: size - 16 }, (_, i) => i + 8).every((i) => drawn[6][i] === (i % 2 === 0) && drawn[i][6] === (i % 2 === 0)));
  let zbar = '';
  try {
    const px = 4;
    const dim = (size + 8) * px;
    const img = Buffer.alloc(dim * dim, 255);
    for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) if (drawn[y][x])
      for (let dy = 0; dy < px; dy++) for (let dx = 0; dx < px; dx++) img[((y + 4) * px + dy) * dim + (x + 4) * px + dx] = 0;
    const f = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'qr-')), 'qr.pgm');
    fs.writeFileSync(f, Buffer.concat([Buffer.from(`P5\n${dim} ${dim}\n255\n`), img]));
    zbar = cp.execFileSync('zbarimg', ['--raw', '-q', f], { encoding: 'utf8', timeout: 20000 }).replace(/\n$/, '');
  } catch (e) { zbar = null; }
  if (zbar === null) console.log('SKIP  independent decode (zbarimg not installed or failed to read the image)');
  else check('independent decoder (zbarimg) reads exactly the report URL from the drawn QR', zbar === REPORT_URL);

  byId['report-text'].scrollTop = 0;
  doc.keydown({ keyCode: 40, preventDefault() {} });
  check('down key scrolls the report text', byId['report-text'].scrollTop > 0);
  byId['r-back'].listeners.click();
  await tick();
  check('Back returns to the main screen', !byId.main.hidden && byId['report-view'].hidden);

  status = Object.assign({}, status, { uid: 1000 });
  byId['b-refresh'].listeners.click();
  for (let i = 0; i < 4; i++) await tick();
  check('not root: explains and hides actions', /not running as root/.test(byId.headline.textContent)
    && visible().join() === 'b-refresh');

  check('every exec command is whitelisted', calls.filter((c) => c.uri.endsWith('/exec')).every((c) => ALLOWED.test(c.p.command))
    && execs().length > 0);
  check('only the HBC exec and connection manager are called', calls.every((c) =>
    c.uri === 'luna://org.webosbrew.hbchannel.service/exec' || /connectionmanager\/getStatus$/.test(c.uri)));
  if (fails) { console.log(`${fails} check(s) failed`); process.exit(1); }
  console.log('all checks passed');
})();
