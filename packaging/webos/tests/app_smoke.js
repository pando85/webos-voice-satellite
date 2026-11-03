// Host smoke test of the status app (packaging/webos/app/app.js) with a
// minimal fake DOM and a fake PalmServiceBridge. Checks that the app sends
// only whitelisted ctl.sh commands through the Homebrew Channel's exec,
// renders a status, shows the right buttons, applies a pending update once,
// and needs a second press for destructive actions.
// usage: node packaging/webos/tests/app_smoke.js
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');

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
  };
}
const byId = {};
for (const m of html.matchAll(/<(\w+) id="([^"]+)"([^>]*)>([^<]*)/g)) {
  const e = el(m[2], m[1]);
  for (const a of m[3].matchAll(/([\w-]+)="([^"]*)"/g)) e.attrs[a[1]] = a[2];
  e.textContent = m[4];
  byId[m[2]] = e;
}
byId.actions.children = Object.values(byId).filter((e) => e.tagName === 'button');
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
      const out = / status --json /.test(p.command) ? JSON.stringify(status) + '\n' : 'webos-satellite: done\n';
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
vm.runInNewContext(fs.readFileSync(path.join(APP, 'app.js'), 'utf8'), {
  window, document: doc, JSON, setTimeout: (f) => timers.push(f), setInterval: () => 0,
  decodeURIComponent, String,
});

let fails = 0;
function check(name, ok) { console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}`); if (!ok) fails += 1; }
const tick = () => new Promise((r) => setImmediate(r));
const visible = () => byId.actions.children.filter((b) => !b.hidden).map((b) => b.id).sort();
const execs = () => calls.filter((c) => c.uri.endsWith('/exec')).map((c) => c.p.command);
const ALLOWED = /^sh '\/media\/developer\/apps\/usr\/palm\/applications\/io\.github\.pando85\.voicesatellite\/payload\/ctl\.sh' (status --json|install|start|stop|autostart on|autostart off|mic off|mic on|pair|reset-pairing|clear-crashloop|uninstall|purge) 2>&1$/;

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
