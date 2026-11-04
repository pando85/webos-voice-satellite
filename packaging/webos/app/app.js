/*
 * Voice Satellite status app. ES5 only (old webOS web runtimes).
 *
 * Talks to the satellite through the Homebrew Channel's root `exec` method
 * (luna://org.webosbrew.hbchannel.service/exec), running the package's own
 * payload/ctl.sh with a fixed argument list: no text typed or received is
 * ever put into a command. Design: docs/design/packaging.md.
 */
(function () {
  'use strict';

  var HBC_EXEC = 'luna://org.webosbrew.hbchannel.service/exec';
  // Documented LG API; the service name changed between webOS versions.
  var CONN_SERVICES = ['luna://com.webos.service.connectionmanager/getStatus',
    'luna://com.palm.connectionmanager/getStatus'];
  // The only argument lists the app sends to ctl.sh.
  var ALLOWED = ['status --json', 'install', 'start', 'stop', 'autostart on',
    'autostart off', 'mic off', 'mic on', 'pair', 'reset-pairing',
    'clear-crashloop', 'uninstall', 'purge'];

  var appDir = decodeURIComponent(window.location.pathname.replace(/\/[^\/]*$/, ''));
  var ctlPath = appDir + '/payload/ctl.sh';
  var pending = [];
  var busy = false;
  var autoUpdateTried = false;
  var tvAddress = '';
  var last = null;

  function $(id) { return document.getElementById(id); }

  function text(el, s) { el.textContent = s; }

  function message(s) { text($('message'), s || ''); }

  // One Luna call through the web runtime's PalmServiceBridge (what LG's
  // webOSTV.js does inside webOS.service.request). The bridge object is kept
  // referenced until its reply arrives.
  function luna(uri, params, done) {
    if (!window.PalmServiceBridge) {
      done({ returnValue: false, errorText: 'not running on a webOS TV (PalmServiceBridge missing)' });
      return;
    }
    var bridge = new window.PalmServiceBridge();
    pending.push(bridge);
    bridge.onservicecallback = function (msg) {
      var res;
      try { res = JSON.parse(msg); } catch (e) { res = { returnValue: false, errorText: String(msg) }; }
      var i = pending.indexOf(bridge);
      if (i >= 0) { pending.splice(i, 1); }
      done(res);
    };
    bridge.call(uri, JSON.stringify(params));
  }

  function ctl(args, done) {
    if (ALLOWED.indexOf(args) < 0 || !/^\/[A-Za-z0-9._\/+-]+$/.test(ctlPath)) {
      done({ ok: false, out: '', err: 'refused: ' + args });
      return;
    }
    luna(HBC_EXEC, { command: "sh '" + ctlPath + "' " + args + ' 2>&1' }, function (res) {
      var out = res.stdoutString || '';
      var ok = res.returnValue !== false && !res.errorCode && !res.errorText;
      done({ ok: ok, out: out, err: res.errorText || '' });
    });
  }

  function lastLine(s) {
    var lines = (s || '').replace(/\s+$/, '').split('\n');
    return lines[lines.length - 1];
  }

  function addFact(dl, name, value, cls) {
    var dt = document.createElement('dt');
    var dd = document.createElement('dd');
    text(dt, name);
    text(dd, value);
    if (cls) { dd.className = cls; }
    dl.appendChild(dt);
    dl.appendChild(dd);
  }

  function show(id, on) { $(id).hidden = !on; }

  var KEY_TEXT = {
    fixed: ['fixed key from etc/noise_psk', 'ok'],
    paired: ['paired with Home Assistant', 'ok'],
    unpaired: ['not paired: add it in Home Assistant while the pairing window is open', 'warn'],
    cleared: ['unpaired by Home Assistant (device removed there); open the pairing window to pair again', 'warn'],
    none: ['no key and pairing off: check the config', 'bad']
  };

  function render(s) {
    last = s;
    var facts = $('facts');
    while (facts.firstChild) { facts.removeChild(facts.firstChild); }
    var root = s.uid === 0;
    var headline;
    var detail = '';
    if (!root) {
      headline = 'The Homebrew Channel is not running as root';
      detail = 'This app needs a rooted TV with an elevated Homebrew Channel. Open the Homebrew Channel settings and check "Root status".';
    } else if (!s.installed) {
      headline = 'Not set up yet';
      detail = 'Set up installs the service under ' + s.prefix + ', starts it and opens a 10-minute pairing window for Home Assistant.';
    } else if (s.running) {
      headline = 'Running';
      if (!s.firmware_ok) { detail = 'The microphone stays off: this firmware is not validated.'; }
      else if (s.mic_off) { detail = 'The microphone is off (kill switch).'; }
      else { detail = 'Listening through Home Assistant.'; }
    } else {
      headline = 'Stopped';
      if (s.crashloop) { detail = 'The supervisor gave up after repeated crashes. See the log, then clear the marker.'; }
      else if (s.disabled) { detail = 'Starts are refused while ' + s.prefix + '/disable exists.'; }
    }
    text($('headline'), headline);
    text($('detail'), detail);

    addFact(facts, 'Package', s.package_version || '?');
    addFact(facts, 'Installed', s.installed ? (s.installed_version || '?') + (s.update_pending ? ' (update pending)' : '') : 'no',
      s.update_pending ? 'warn' : '');
    if (s.build) { addFact(facts, 'Build', s.build); }
    addFact(facts, 'Service', s.running ? 'running, pid ' + s.pid + (s.supervised ? ', supervised' : '') : 'stopped',
      s.running ? 'ok' : 'warn');
    addFact(facts, 'Start at boot', s.autostart ? 'on' : 'off');
    addFact(facts, 'Microphone', s.mic_off ? 'off (kill switch)' : 'on', s.mic_off ? 'warn' : '');
    addFact(facts, 'Firmware', s.firmware || '?', s.firmware_ok ? '' : 'warn');
    if (s.installed) {
      var k = KEY_TEXT[s.key] || [s.key, ''];
      addFact(facts, 'Home Assistant', k[0], k[1]);
      addFact(facts, 'Device name', s.name || '?');
      addFact(facts, 'Reply volume', s.reply_volume ? s.reply_volume + ' %'
        : (s.reply_volume_default ? s.reply_volume_default + ' % (default; set it in Home Assistant)' : 'default (set it in Home Assistant)'));
      // Short form of the risks in docs/design/always-on.md section 12.
      addFact(facts, 'Remote power key',
        s.remote_off === 'keep_listening'
          ? 'keeps listening: the TV stays on with the screen dark (uses more power than standby; OLED care at power-off may not run). Change it in Home Assistant.'
          : 'turns the TV off normally', s.remote_off === 'keep_listening' ? 'warn' : '');
    }
    addFact(facts, 'Address', (tvAddress || 'unknown') + ', port ' + (s.port || '6053'));
    addFact(facts, 'Kernel', s.machine || '?');

    text($('log'), (s.log || []).slice(-4).join('\n') || '(no log yet)');

    var inst = root && s.installed;
    show('b-install', root && !s.installed);
    show('b-update', inst && s.update_pending);
    show('b-start', inst && !s.running);
    show('b-stop', inst && s.running);
    show('b-boot-on', inst && !s.autostart);
    show('b-boot-off', root && s.autostart);
    show('b-mic-off', root && !s.mic_off);
    show('b-mic-on', root && s.mic_off);
    show('b-pair', inst && s.running && (s.key === 'unpaired' || s.key === 'cleared'));
    show('b-reset', inst && s.running && s.key === 'paired');
    show('b-crash', inst && s.crashloop);
    show('b-uninstall', inst);
    show('b-purge', root && (s.installed || s.installed_version !== '' || s.key !== 'none'));
    focusFirst();

    // An update installed through the Homebrew Channel or luna-send is applied
    // when the app is opened (also at the next boot, when start at boot is on).
    if (inst && s.update_pending && !autoUpdateTried) {
      autoUpdateTried = true;
      run('install', 'Applying update ' + (s.installed_version || '?') + ' -> ' + (s.package_version || '?') + '...');
    }
  }

  function refresh() {
    ctl('status --json', function (r) {
      var s = null;
      var line = lastLine(r.out);
      try { s = JSON.parse(line); } catch (e) { s = null; }
      if (!s) {
        text($('headline'), 'Cannot reach the Homebrew Channel');
        text($('detail'), r.err || line || 'No answer from luna://org.webosbrew.hbchannel.service/exec.');
        return;
      }
      render(s);
    });
  }

  function run(args, label, then) {
    if (busy) { return; }
    busy = true;
    message(label || ('Running ' + args + '...'));
    ctl(args, function (r) {
      busy = false;
      message(lastLine(r.out) || r.err || (r.ok ? 'done' : 'failed'));
      if (r.ok && then) {
        run(then, null, null);
        return;
      }
      refresh();
    });
  }

  function onButton(btn) {
    var args = btn.getAttribute('data-cmd');
    if (args === '') { message(''); refresh(); return; }
    if (btn.getAttribute('data-confirm') && !btn.armed) {
      btn.armed = true;
      btn.className = 'armed';
      btn.setAttribute('data-label', btn.textContent);
      text(btn, 'Press again to confirm');
      setTimeout(function () {
        btn.armed = false;
        btn.className = '';
        text(btn, btn.getAttribute('data-label'));
      }, 5000);
      return;
    }
    run(args, null, btn.getAttribute('data-then'));
  }

  function visibleButtons() {
    var all = $('actions').getElementsByTagName('button');
    var out = [];
    for (var i = 0; i < all.length; i++) { if (!all[i].hidden) { out.push(all[i]); } }
    return out;
  }

  function focusFirst() {
    var b = visibleButtons();
    if (b.length && (document.activeElement === document.body || document.activeElement.hidden)) { b[0].focus(); }
  }

  // Remote control: arrow keys move between buttons, OK (Enter) presses.
  document.addEventListener('keydown', function (ev) {
    var b = visibleButtons();
    var i = b.indexOf(document.activeElement);
    var k = ev.keyCode;
    if (k === 37 || k === 38) { i = i <= 0 ? b.length - 1 : i - 1; }
    else if (k === 39 || k === 40) { i = i < 0 || i >= b.length - 1 ? 0 : i + 1; }
    else { return; }
    if (b[i]) { b[i].focus(); }
    ev.preventDefault();
  });

  var buttons = $('actions').getElementsByTagName('button');
  for (var j = 0; j < buttons.length; j++) {
    buttons[j].addEventListener('click', (function (btn) { return function () { onButton(btn); }; }(buttons[j])));
    buttons[j].hidden = true;
  }
  show('b-refresh', true);

  function findAddress(idx) {
    if (idx >= CONN_SERVICES.length) { return; }
    luna(CONN_SERVICES[idx], {}, function (r) {
      if (r.returnValue === false) { findAddress(idx + 1); return; }
      var w = r.wired || {};
      var f = r.wifi || {};
      tvAddress = (w.state === 'connected' && w.ipAddress) || (f.state === 'connected' && f.ipAddress) || w.ipAddress || f.ipAddress || '';
      if (last) { render(last); }
    });
  }

  findAddress(0);
  refresh();
  setInterval(function () { if (!busy) { refresh(); } }, 10000);
}());
