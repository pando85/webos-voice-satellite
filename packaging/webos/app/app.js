/*
 * Voice Satellite status app. ES5 only (old webOS web runtimes), except the
 * vendored qrcodegen.js (Project Nayuki, MIT; ES2015, see
 * QRCODEGEN-LICENSE.txt), which draws the compatibility-report QR code
 * offline.
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
    'clear-crashloop', 'uninstall', 'purge', 'report --json', 'selfcheck', 'ack-firmware',
    // Voice and power screen: one read and six fixed changes, no typed text.
    'tv-settings --json', 'set always-ready on', 'set always-ready off', 'set hi-lg on', 'set hi-lg off',
    'set remote-off normal', 'set remote-off keep'];
  var QR_SIZE = 640;

  var appDir = decodeURIComponent(window.location.pathname.replace(/\/[^\/]*$/, ''));
  var ctlPath = appDir + '/payload/ctl.sh';
  var pending = [];
  var busy = false;
  var autoUpdateTried = false;
  var tvAddress = '';
  var last = null;
  var reportOpen = false;
  var voiceOpen = false;

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

  var COMPAT_CLASS = {
    'verified': 'ok',
    'series verified (firmware not yet reported)': '',
    'not verified': 'warn',
    'firmware changed': 'warn',
    'capture problem': 'bad',
    'unknown': 'warn',
    'checking': ''
  };

  // The one-paragraph banner under the headline: what the compatibility
  // status means and what to do. Returns [text, class] or null.
  function compatBanner(s) {
    var c = s.compat;
    if (!s.installed || !c) { return null; }
    if (c.capture_problem) {
      return ['The microphone check failed (' + (c.selfcheck || 'no details') + '). The microphone is closed and retried a few times. ' +
        'If LG hands-free voice is on, turn it off. Otherwise press Report compatibility so this TV and firmware can be looked at.', 'bad'];
    }
    if (c.firmware_changed) {
      return ['The TV firmware changed since the satellite last ran. The microphone keeps running and is being checked again. ' +
        'When it works for you, press Acknowledge. If it does not, press Report compatibility.', ''];
    }
    if (c.compat_status === 'not verified' || c.compat_status === 'unknown') {
      return ['This TV and firmware are not in the project\'s verified list yet. The microphone runs anyway and is checked. ' +
        'If it does not work, press Report compatibility.', ''];
    }
    if (c.compat_status === 'series verified (firmware not yet reported)') {
      return ['This TV series is verified; this firmware version has not been reported yet. If it works, a report helps everyone: press Report compatibility.', 'ok'];
    }
    return null;
  }

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
      if (!s.firmware_ok) { detail = 'The microphone stays off: strict firmware policy and this firmware is not in the list.'; }
      else if (s.compat && s.compat.capture_problem) { detail = 'The microphone check failed; the microphone is closed (see below).'; }
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
    var c = s.compat || null;
    if (c) {
      addFact(facts, 'Compatibility', c.compat_status || '?', COMPAT_CLASS[c.compat_status] || '');
      if (c.tv_model) { addFact(facts, 'TV model', c.tv_model); }
      addFact(facts, 'Microphone check', c.selfcheck || '?', c.capture_problem ? 'bad' : '');
    }
    if (s.installed) {
      var k = KEY_TEXT[s.key] || [s.key, ''];
      addFact(facts, 'Home Assistant', k[0], k[1]);
      addFact(facts, 'Device name', s.name || '?');
      addFact(facts, 'Reply volume', s.reply_volume ? s.reply_volume + ' %'
        : (s.reply_volume_default ? s.reply_volume_default + ' % (default; set it in Home Assistant)' : 'default (set it in Home Assistant)'));
      // Short form of the risks in docs/design/always-on.md section 12.
      addFact(facts, 'Remote power key',
        s.remote_off === 'keep_listening'
          ? 'keeps listening: the TV stays on with the screen dark (uses more power than standby; OLED care at power-off may not run). Change it in Voice and power or in Home Assistant.'
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
    show('b-voice', inst);
    show('b-report', inst);
    show('b-selfcheck', inst && s.running && !!s.compat);
    show('b-ack', inst && s.running && !!s.compat && !!s.compat.firmware_changed);
    var banner = compatBanner(s);
    show('compat', !!banner);
    if (banner) { text($('compat-text'), banner[0]); $('compat').className = 'panel banner ' + banner[1]; }
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

  // --- compatibility report screen -------------------------------------------

  function drawQr(url) {
    var canvas = $('qr');
    var ctx = canvas.getContext('2d');
    var qr = window.qrcodegen.QrCode.encodeText(url, window.qrcodegen.QrCode.Ecc.LOW);
    var border = 4;
    var cells = qr.size + border * 2;
    var scale = Math.max(1, Math.floor(QR_SIZE / cells));
    var px = cells * scale;
    canvas.width = px;
    canvas.height = px;
    canvas.style.width = px + 'px';
    canvas.style.height = px + 'px';
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, px, px);
    ctx.fillStyle = '#000000';
    for (var y = 0; y < qr.size; y++) {
      for (var x = 0; x < qr.size; x++) {
        if (qr.getModule(x, y)) { ctx.fillRect((x + border) * scale, (y + border) * scale, scale, scale); }
      }
    }
    return qr.size;
  }

  function openReport() {
    if (busy) { return; }
    busy = true;
    message('Building the report...');
    ctl('report --json', function (r) {
      busy = false;
      var rep = null;
      try { rep = JSON.parse(lastLine(r.out)); } catch (e) { rep = null; }
      if (!rep) {
        message(lastLine(r.out) || r.err || 'The report could not be built.');
        return;
      }
      message('');
      reportOpen = true;
      show('main', false);
      show('report-view', true);
      text($('report-error'), '');
      text($('report-title'), rep.title || '');
      text($('report-url'), rep.url || '');
      text($('report-text'), rep.full || rep.short || '');
      $('report-text').scrollTop = 0;
      if (rep.error || !rep.url) {
        text($('report-error'), 'The report was withheld: ' + (rep.error || 'no link') + '. See ctl.sh report over SSH.');
      } else {
        try { drawQr(rep.url); } catch (e) { text($('report-error'), 'The QR code could not be drawn (' + e + '). Type the address instead.'); }
      }
      $('r-back').focus();
    });
  }

  function closeReport() {
    reportOpen = false;
    show('report-view', false);
    show('main', true);
    focusFirst();
    refresh();
  }

  // --- Voice and power screen ------------------------------------------------
  // Design: docs/design/voice-and-power-screen.md and ADR 0008. The screen
  // reads `ctl.sh tv-settings --json` and changes one setting per press with
  // `ctl.sh set ...` (two presses for every LG change and for turning Keep
  // listening on, one to turn it off). The matrix below is the only place that
  // turns the facts into sentences; tests/app_matrix.js checks every cell.

  var AR_MENU = 'Settings, General, Always Ready';
  var HL_MENU = 'Settings, General, AI Service, Voice recognition settings, Hi LG';
  var ARM_MS = 5000;
  var REREAD_MS = 3000;

  // What the remote's power button does, by Always Ready x "Keep listening".
  // P1 off+normal, P2 on+normal, P3 off+keep, P4 on+keep, PX not known.
  var POWER = {
    P1: ['Power button: the TV goes to deep standby.', 'The satellite cannot hear until the TV is turned on.'],
    P2: ['Power button: the TV goes to standby.', 'The satellite still hears you, but you will not hear its replies.'],
    P3: ['Power button: the screen goes dark but the TV stays on.', 'The satellite hears you and replies. Press power again to turn the picture on.'],
    P4: ['Power button: the screen goes dark but the TV stays on.', 'The satellite hears you and replies. Press power again to turn the picture on.'],
    PX: ['Power button: not known.', 'The TV\'s settings could not be read. Press Refresh.']
  };
  var POWER_EXTRA = {
    kill: 'Keep listening does nothing while the microphone is off with the kill switch.',
    nowatch: 'Keep listening needs the power watch, which is off in the config.'
  };

  // The microphone line, first match wins (see outcome()).
  var LISTEN = {
    stopped: 'Listening now: no. The satellite is not running.',
    kill: 'Listening now: no. The microphone is off (kill switch).',
    muted: 'Listening now: no. The microphone is muted in Home Assistant.',
    hilg: 'Listening now: no. The satellite cannot hear: LG\'s voice assistant has the microphone. Turn Hi LG off to use the satellite.',
    busy: 'Listening now: no. Another program has the microphone.',
    ok: 'Listening now: yes.',
    closed: 'Listening now: no. The microphone is closed.',
    unknown: 'Listening now: not known.'
  };
  var NOTE = {
    hilgListening: 'Hi LG is on but this satellite has the microphone now; after a restart LG may take it. Turn Hi LG off to be sure.',
    hilgOn: 'Hi LG is on. If the satellite cannot hear you, turn it off.',
    others: 'Alexa/other hands-free is on: turn it off in LG\'s menu.',
    unreadable: 'Always Ready or Hi LG could not be read, so the sentence above may be wrong.'
  };

  var VROWS = [
    { id: 'ar', btn: 'v-row-ar', state: 'v-ar-state', label: 'Always Ready', menu: AR_MENU },
    { id: 'hl', btn: 'v-row-hl', state: 'v-hl-state', label: 'Hi LG', menu: HL_MENU },
    { id: 'keep', btn: 'v-row-keep', state: 'v-keep-state', label: 'Keep listening', menu: '' }
  ];
  var VFOCUS = ['v-row-ar', 'v-row-hl', 'v-row-keep', 'v-refresh', 'v-back'];
  var AR_KNOWN = ['off', 'alwaysReady', 'allEnabled', 'none', 'lifeOnScreen'];

  var vData = null;     // last tv-settings reply (null: not read or unreadable)
  var vLoaded = false;  // a read has finished at least once since the screen opened
  var vArmed = null;    // id of the row waiting for the second press
  var vArmToken = 0;
  var vPending = null;  // id of the row whose command runs
  var vReading = false;
  var vExpect = null;   // { id, kind } to compare with the re-read after a change
  var vMsg = '';
  var vMsgCls = '';

  function parseJsonLine(out) {
    var lines = (out || '').replace(/\s+$/, '').split('\n');
    for (var i = lines.length - 1; i >= 0; i--) {
      if (lines[i].charAt(0) === '{') {
        try { return JSON.parse(lines[i]); } catch (e) { return null; }
      }
    }
    return null;
  }

  // Always Ready as the TV reports it: kind is one of AR_KNOWN, 'other' or 'error'.
  function arInfo(ar) {
    ar = ar || {};
    if (ar.state === 'error' || (!ar.state && !ar.raw)) { return { kind: 'error', raw: '', error: ar.error || '' }; }
    var raw = ar.raw || '';
    if (!raw && ar.state === 'off') { raw = 'off'; }
    if (!raw && ar.state === 'on') { raw = ar.variant === 'wallpaper' ? 'allEnabled' : 'alwaysReady'; }
    return { kind: AR_KNOWN.indexOf(raw) >= 0 ? raw : 'other', raw: raw, error: '' };
  }

  // Hi LG is the thinQ provider only (EXP-011: setActivation has no provider
  // parameter). tv-settings reports state `on` when ANY provider is on, so the
  // per-provider flags decide: thinq true is Hi LG on; amazon or dangbei true
  // is the "other hands-free" note.
  function hlInfo(hl) {
    hl = hl || {};
    var kind;
    if (hl.state === 'error' || !hl.state) { kind = 'error'; }
    else if (hl.thinq === true) { kind = 'on'; }
    else if (hl.thinq === false) { kind = 'off'; }
    else { kind = hl.state === 'on' || hl.state === 'off' ? hl.state : 'unknown'; }
    return { kind: kind, others: hl.amazon === true || hl.dangbei === true, error: hl.error || '' };
  }

  function roInfo(ro) {
    ro = ro || {};
    var kind = ro.state === 'keep_listening' || ro.state === 'keep' ? 'keep' : (ro.state === 'normal' ? 'normal' : 'unknown');
    return { kind: kind, noWatch: ro.active === false };
  }

  // The sentence: { cell, headline, detail, extra, listenCell, listening, notes }.
  function outcome(s) {
    s = s || {};
    var ar = arInfo(s.always_ready);
    var hl = hlInfo(s.hi_lg);
    var ro = roInfo(s.remote_off);
    var mic = s.mic || {};
    var micState = ['listening', 'busy', 'muted', 'closed'].indexOf(mic.state) >= 0 ? mic.state : 'unknown';
    var kill = mic.kill_switch === true || mic.state === 'kill';
    var stopped = s.daemon_running === false;
    var hlOn = hl.kind === 'on';

    var arOn = ar.kind === 'alwaysReady' || ar.kind === 'allEnabled';
    var arOff = ar.kind === 'off' || ar.kind === 'none';
    var cell = 'PX';
    var extra = '';
    if ((arOn || arOff) && ro.kind !== 'unknown') {
      // The kill switch (and a missing power watch) stop the daemon acting on a power-off.
      var keep = ro.kind === 'keep' && !kill && !ro.noWatch;
      if (ro.kind === 'keep' && kill) { extra = POWER_EXTRA.kill; }
      else if (ro.kind === 'keep' && ro.noWatch) { extra = POWER_EXTRA.nowatch; }
      cell = arOff ? (keep ? 'P3' : 'P1') : (keep ? 'P4' : 'P2');
    }

    var listenCell;
    if (stopped) { listenCell = 'stopped'; }
    else if (kill) { listenCell = 'kill'; }
    else if (micState === 'muted') { listenCell = 'muted'; }
    else if (micState === 'busy') { listenCell = hlOn ? 'hilg' : 'busy'; }
    else if (micState === 'listening') { listenCell = 'ok'; }
    else if (micState === 'closed') { listenCell = 'closed'; }
    else { listenCell = 'unknown'; }

    var notes = [];
    if (hlOn && listenCell === 'ok') { notes.push(NOTE.hilgListening); }
    else if (hlOn && (listenCell === 'closed' || listenCell === 'unknown')) { notes.push(NOTE.hilgOn); }
    if (hl.others) { notes.push(NOTE.others); }
    if (!(arOn || arOff) || hl.kind === 'error' || hl.kind === 'unknown') { notes.push(NOTE.unreadable); }

    return { cell: cell, headline: POWER[cell][0], detail: POWER[cell][1], extra: extra,
      listenCell: listenCell, listening: LISTEN[listenCell], notes: notes };
  }

  function wordOf(id, v) {
    if (id === 'ar') {
      if (v === 'on' || v === 'alwaysReady') { return 'On'; }
      if (v === 'off') { return 'Off'; }
      if (v === 'allEnabled') { return 'On, with wallpaper'; }
      return v ? 'Not known (' + v + ')' : 'Not known';
    }
    if (v === 'on' || v === 'keep' || v === 'keep_listening') { return 'On'; }
    if (v === 'off' || v === 'normal') { return 'Off'; }
    return v ? 'Not known (' + v + ')' : 'Not known';
  }

  function confirmText(id, turningOn) {
    if (id === 'ar') {
      return turningOn
        ? 'Turn on Always Ready? The TV then uses more power in standby (not measured). With "Keep listening" off, the satellite hears you after the power button but its replies are silent.'
        : 'Turn off Always Ready? Without "Keep listening", the TV suspends after the power button and the satellite stops.';
    }
    if (id === 'hl') {
      return turningOn
        ? 'Turn on Hi LG? LG\'s assistant may take the microphone, so the satellite may not hear you. It did not while the satellite already had the microphone, but it can after a restart.'
        : 'Turn off Hi LG? The TV stops answering "Hi LG". The satellite can use the microphone.';
    }
    return turningOn
      ? 'Turn on Keep listening? After the power button the TV stays on with the screen dark. It uses more power than standby (not measured), LG\'s OLED care at power-off may not run, and the screen may flash briefly (not observed).'
      : 'Keep listening is on. Press OK to turn it off. The power button then turns the TV off the normal way.';
  }

  // The three rows: { word, cls, checked, can, cmd, confirm, warn }.
  function rowsModel(s) {
    var loaded = !!s;
    function wait() { return { word: 'Checking...', cls: 'muted', checked: 'false', can: false, cmd: '', confirm: '', warn: '' }; }
    if (!loaded) { return [wait(), wait(), wait()]; }
    var ar = arInfo(s.always_ready);
    var hl = hlInfo(s.hi_lg);
    var ro = roInfo(s.remote_off);
    var a;
    var h;
    var k;
    var menuNote = function (what, menu) { return what + ' Change it in LG\'s menu (' + menu + ').'; };

    if (ar.kind === 'error') {
      a = { word: 'Cannot read', cls: 'bad', checked: 'false', can: false, cmd: '', confirm: '',
        warn: menuNote('Always Ready could not be read' + (ar.error ? ': ' + ar.error + '.' : '.'), AR_MENU) };
    } else if (ar.kind === 'off') {
      a = { word: 'Off', cls: '', checked: 'false', can: true, cmd: 'set always-ready on', confirm: confirmText('ar', true) };
    } else if (ar.kind === 'alwaysReady') {
      a = { word: 'On', cls: '', checked: 'true', can: true, cmd: 'set always-ready off', confirm: confirmText('ar', false) };
    } else if (ar.kind === 'allEnabled') {
      a = { word: wordOf('ar', 'allEnabled'), cls: 'warn', checked: 'true', can: false, cmd: '', confirm: '',
        warn: menuNote('Always Ready is set to On, with wallpaper, which this app does not change.', AR_MENU) };
    } else {
      a = { word: wordOf('ar', ar.raw), cls: 'warn', checked: 'mixed', can: false, cmd: '', confirm: '',
        warn: menuNote('Always Ready is set to ' + (ar.raw || 'a value this app does not know') + ', which this app does not change.', AR_MENU) };
    }
    if (a.can) { a.warn = a.confirm; a.confirm = a.confirm + ' Press OK again to confirm.'; }

    if (hl.kind === 'on' || hl.kind === 'off') {
      var hon = hl.kind === 'on';
      h = { word: hon ? 'On' : 'Off', cls: hon ? 'warn' : '', checked: hon ? 'true' : 'false', can: true,
        cmd: hon ? 'set hi-lg off' : 'set hi-lg on', confirm: confirmText('hl', !hon) };
      h.warn = h.confirm;
      h.confirm = h.confirm + ' Press OK again to confirm.';
    } else {
      h = { word: hl.kind === 'error' ? 'Cannot read' : 'Not known', cls: hl.kind === 'error' ? 'bad' : 'warn', checked: 'false',
        can: false, cmd: '', confirm: '', warn: menuNote('Hi LG could not be read' + (hl.error ? ': ' + hl.error + '.' : '.'), HL_MENU) };
    }

    if (ro.kind === 'unknown') {
      k = { word: 'Cannot read', cls: 'bad', checked: 'false', can: false, cmd: '', confirm: '', warn: 'Keep listening could not be read.' };
    } else if (ro.kind === 'keep') {
      k = { word: 'On', cls: 'warn', checked: 'true', can: true, cmd: 'set remote-off normal', confirm: '', warn: confirmText('keep', false) };
    } else {
      k = { word: 'Off', cls: '', checked: 'false', can: true, cmd: 'set remote-off keep', confirm: confirmText('keep', true) };
      k.warn = k.confirm;
      k.confirm = k.confirm + ' Press OK again to confirm.';
    }
    return [a, h, k];
  }

  // One line for the result of a `set` (the JSON line of ctl.sh), as [text, class].
  function resultMessage(id, r, model) {
    var row = VROWS[id === 'ar' ? 0 : (id === 'hl' ? 1 : 2)];
    if (!r) { return ['Could not confirm the change of ' + row.label + '. Press Refresh to see what the TV reports.', 'bad']; }
    var fix = row.menu ? ' You can also change it on the TV: ' + row.menu + '.' : '';
    var why = r.error ? r.error : 'no reason given';
    if (r.result === 'applied') {
      if (id === 'keep' && wordOf(id, r.after) === 'Off') { return ['Done. The power button turns the TV off the normal way.', 'ok']; }
      var tail = r.daemon === 'pending' ? ' The satellite has not applied it yet.'
        : (r.daemon === 'not running' ? ' The satellite is not running; it applies this when it starts.' : '');
      return [row.label + ' is now ' + wordOf(id, r.after) + '. Changed from ' + wordOf(id, r.before) + '.' + tail, 'ok'];
    }
    if (r.result === 'already') { return ['Already ' + wordOf(id, r.after || r.before) + '. Nothing changed.', 'ok']; }
    if (r.result === 'mismatch') {
      return ['The TV reports ' + wordOf(id, r.after) + ', not ' + wordOf(id, r.requested) + '. The change did not stick, or LG changed it back. It is ' +
        wordOf(id, r.after) + ' now.', 'warn'];
    }
    if (r.result === 'refused') { return ['Could not change ' + row.label + ': ' + why + '.' + fix, 'bad']; }
    return ['Could not change ' + row.label + ': ' + why + '. It is still ' + (model && model.word ? model.word : wordOf(id, r.before)) + '.' + fix, 'bad'];
  }

  // The kind of one row in a tv-settings reply (to see whether LG reverted a change).
  function rowKind(id, d) {
    if (id === 'ar') { return arInfo(d.always_ready).kind; }
    if (id === 'hl') { return hlInfo(d.hi_lg).kind; }
    return roInfo(d.remote_off).kind;
  }

  function vModels() { return rowsModel(vLoaded ? (vData || {}) : null); }

  function vFocusedRow() {
    for (var i = 0; i < VROWS.length; i++) { if (document.activeElement === $(VROWS[i].btn)) { return i; } }
    return -1;
  }

  function renderVoice() {
    var m = vModels();
    var o = vLoaded ? outcome(vData || {}) : null;
    text($('v-headline'), o ? o.headline : 'Checking...');
    text($('v-detail'), o ? (o.detail + (o.extra ? ' ' + o.extra : '')) : '');
    text($('v-listening'), o ? o.listening : '');
    text($('v-notes'), o ? o.notes.join('\n') : '');
    for (var i = 0; i < VROWS.length; i++) {
      var r = VROWS[i];
      var b = $(r.btn);
      var armed = vArmed === r.id;
      var busyRow = vPending === r.id;
      var inert = !m[i].can || !!vPending;
      text($(r.state), busyRow ? 'Changing...' : (armed ? 'Press OK again' : m[i].word));
      $(r.state).className = 'v-state ' + (busyRow ? 'muted' : (armed ? 'armed' : m[i].cls));
      b.className = 'vrow' + (armed ? ' armed' : '') + (inert ? ' disabled' : '');
      b.setAttribute('aria-checked', m[i].checked);
      b.setAttribute('aria-disabled', inert ? 'true' : 'false');
    }
    var f = vFocusedRow();
    var warn = '';
    if (vArmed) { warn = m[vArmed === 'ar' ? 0 : (vArmed === 'hl' ? 1 : 2)].confirm; }
    else if (f >= 0) { warn = m[f].warn; }
    text($('v-warning'), warn);
    text($('v-message'), vMsg);
    $('v-message').className = 'message ' + vMsgCls;
  }

  function disarm() {
    if (!vArmed) { return; }
    vArmed = null;
    vArmToken++;
    renderVoice();
  }

  function readVoice(after) {
    if (vReading || vPending) { return; }
    vReading = true;
    ctl('tv-settings --json', function (r) {
      vReading = false;
      if (!voiceOpen) { return; }
      var d = parseJsonLine(r.out);
      vLoaded = true;
      if (d && typeof d === 'object') {
        vData = d;
      } else {
        vData = null;
        vMsg = 'Could not read the TV settings: ' + (lastLine(r.out) || r.err || 'no answer') + '.';
        vMsgCls = 'bad';
      }
      if (after) { after(d); }
      renderVoice();
    });
  }

  function applyLocal(id, after) {
    if (!vData) { return; }
    if (id === 'ar') {
      var raw = after === 'on' ? 'alwaysReady' : after;
      vData.always_ready = { state: raw === 'off' ? 'off' : (raw === 'alwaysReady' || raw === 'allEnabled' ? 'on' : 'unknown'), raw: raw, error: '' };
    } else if (id === 'hl') {
      var prev = vData.hi_lg || {};
      var on = after === 'on';
      vData.hi_lg = { state: on || prev.amazon === true || prev.dangbei === true ? 'on' : 'off', thinq: on, amazon: prev.amazon, dangbei: prev.dangbei, error: '' };
    } else {
      var ro = vData.remote_off || {};
      vData.remote_off = { state: wordOf('keep', after) === 'On' ? 'keep_listening' : 'normal', active: ro.active };
    }
  }

  function doChange(id, model) {
    vArmed = null;
    vArmToken++;
    vPending = id;
    vMsg = '';
    vMsgCls = '';
    renderVoice();
    ctl(model.cmd, function (r) {
      vPending = null;
      if (!voiceOpen) { return; }
      var res = parseJsonLine(r.out);
      var m = resultMessage(id, res, model);
      vMsg = m[0];
      vMsgCls = m[1];
      vExpect = null;
      if (res && res.after && (res.result === 'applied' || res.result === 'already' || res.result === 'mismatch')) {
        applyLocal(id, res.after);
        if (vData) { vExpect = { id: id, kind: rowKind(id, vData) }; }
      }
      renderVoice();
      // One more read a little later catches LG (or another writer) changing it back.
      setTimeout(function () {
        if (!voiceOpen || vPending || vArmed) { return; }
        readVoice(function (d) {
          if (!d || !vExpect || vExpect.id !== id) { return; }
          var now = rowKind(id, d);
          if (now !== vExpect.kind) {
            var w = vModels()[id === 'ar' ? 0 : (id === 'hl' ? 1 : 2)].word;
            vMsg = id === 'keep' ? 'Keep listening was changed to ' + w + ' by someone else.' : 'LG changed ' + VROWS[id === 'ar' ? 0 : 1].label + ' back to ' + w + '.';
            vMsgCls = 'warn';
          }
          vExpect = null;
        });
      }, REREAD_MS);
    });
  }

  function pressRow(id) {
    if (vPending) { return; }
    var m = vModels()[id === 'ar' ? 0 : (id === 'hl' ? 1 : 2)];
    if (!m.can) { renderVoice(); return; }
    if (m.confirm && vArmed !== id) {
      vArmed = id;
      vArmToken++;
      var token = vArmToken;
      vMsg = '';
      renderVoice();
      setTimeout(function () { if (vArmToken === token && vArmed === id) { disarm(); } }, ARM_MS);
      return;
    }
    doChange(id, m);
  }

  function openVoice() {
    if (voiceOpen) { return; }
    voiceOpen = true;
    vData = null;
    vLoaded = false;
    vArmed = null;
    vPending = null;
    vMsg = '';
    vMsgCls = '';
    message('');
    show('main', false);
    show('voice-view', true);
    renderVoice();
    $('v-row-ar').focus();
    renderVoice();
    readVoice(null);
  }

  function closeVoice() {
    voiceOpen = false;
    vArmed = null;
    vArmToken++;
    show('voice-view', false);
    show('main', true);
    if (!$('b-voice').hidden) { $('b-voice').focus(); } else { focusFirst(); }
    refresh();
  }

  function voiceKey(ev) {
    var k = ev.keyCode;
    if (k === 27 || k === 8 || k === 461) { closeVoice(); ev.preventDefault(); return; }
    if (k === 13) { return; } // OK: the button's click does the rest
    if (k === 37 || k === 38 || k === 39 || k === 40) {
      disarm();
      var i = -1;
      for (var j = 0; j < VFOCUS.length; j++) { if (document.activeElement === $(VFOCUS[j])) { i = j; } }
      if (k === 37 || k === 38) { i = i <= 0 ? VFOCUS.length - 1 : i - 1; }
      else { i = i < 0 || i >= VFOCUS.length - 1 ? 0 : i + 1; }
      $(VFOCUS[i]).focus();
      renderVoice();
      ev.preventDefault();
      return;
    }
    disarm();
  }

  function initVoice() {
    show('voice-view', false);
    VROWS.forEach(function (r) {
      $(r.btn).addEventListener('click', function () { pressRow(r.id); });
    });
    $('v-refresh').addEventListener('click', function () {
      disarm();
      if (vPending) { return; }
      vMsg = '';
      vMsgCls = '';
      readVoice(null);
    });
    $('v-back').addEventListener('click', closeVoice);
  }

  window.voicePower = { outcome: outcome, rows: rowsModel, resultMessage: resultMessage, POWER: POWER, LISTEN: LISTEN, NOTE: NOTE, POWER_EXTRA: POWER_EXTRA };

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
    if (args === 'report --json') { openReport(); return; }
    if (args === 'voice') { openVoice(); return; }
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
    if (reportOpen) {
      var box = $('report-text');
      var kc = ev.keyCode;
      if (kc === 38) { box.scrollTop = Math.max(0, box.scrollTop - 80); }
      else if (kc === 40) { box.scrollTop += 80; }
      else if (kc === 27 || kc === 8 || kc === 461) { closeReport(); }
      else { return; }
      ev.preventDefault();
      return;
    }
    if (voiceOpen) { voiceKey(ev); return; }
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
  show('report-view', false);
  $('r-back').addEventListener('click', closeReport);
  initVoice();

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
  setInterval(function () {
    if (voiceOpen) { if (!vPending && !vArmed) { readVoice(null); } return; }
    if (!busy && !reportOpen) { refresh(); }
  }, 10000);
}());
