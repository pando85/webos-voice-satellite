/*
 * Voice Satellite TV app. ES5 only (the TV's web engine is not verified for
 * newer syntax), except the vendored qrcodegen.js (Project Nayuki, MIT;
 * ES2015, see QRCODEGEN-LICENSE.txt), which draws the QR codes offline.
 * The sentences and the phone-request texts live in voicepower.js
 * (window.voicePower), shared with the phone setup page.
 *
 * Talks to the satellite through the Homebrew Channel's root `exec` method
 * (luna://org.webosbrew.hbchannel.service/exec), running the package's own
 * payload/ctl.sh with a fixed argument list: no text typed or received is
 * ever put into a command, except the 8-hex-digit id of a phone request,
 * checked by a regular expression (docs/design/phone-setup.md, ADR 0009).
 *
 * Screens (design "Calm dark", 2026-11-05): Home, Voice and power, Home
 * Assistant (with the phone setup QR), Diagnostics (with Report
 * compatibility and More actions). Back returns to the parent screen, to Home from the others, and leaves the app from Home. The arrow
 * keys move the focus to the nearest element in that direction.
 */
(function () {
  'use strict';

  var VP = window.voicePower;
  var HBC_EXEC = 'luna://org.webosbrew.hbchannel.service/exec';
  // Documented LG API; the service name changed between webOS versions.
  var CONN_SERVICES = ['luna://com.webos.service.connectionmanager/getStatus',
    'luna://com.palm.connectionmanager/getStatus'];
  // The only argument lists the app sends to ctl.sh (plus ANSWER_RE).
  var ALLOWED = ['status --json', 'install', 'start', 'stop', 'autostart on',
    'autostart off', 'mic off', 'mic on', 'pair', 'reset-pairing',
    'clear-crashloop', 'uninstall', 'purge', 'report --json', 'selfcheck', 'ack-firmware',
    // Voice and power screen: one read and six fixed changes, no typed text.
    'tv-settings --json', 'set always-ready on', 'set always-ready off', 'set hi-lg on', 'set hi-lg off',
    'set remote-off normal', 'set remote-off keep',
    // Phone setup (ADR 0009): start, the poll (also the heartbeat), stop.
    'setup start', 'setup status --json', 'setup stop'];
  var ANSWER_RE = /^setup answer [0-9a-f]{8} (allow|refuse)$/;
  var ID_RE = /^[0-9a-f]{8}$/;

  var QR_SIZE = 552;
  var ARM_MS = 5000;           // a two-press action stays armed this long
  var REREAD_MS = 3000;        // re-read after a Voice and power change
  var REFRESH_MS = 10000;      // status refresh (paused as described in tick())
  var HOME_TV_EVERY = 3;       // Home reads tv-settings every 3rd refresh (30 s)
  // Phone session poll, also the heartbeat (design 4.4, EXP-012 P7 on c3-a:
  // about 80 ms and 77 ms of CPU per exec call): every 2 s while a session
  // is waiting or active, every 1 s while a request is pending or its result
  // has not been shown yet.
  var SETUP_POLL_MS = 2000;
  var SETUP_POLL_FAST_MS = 1000;
  var SETUP_HINT_MS = 45000;   // "nothing has opened the link" hint (design 5.3)
  var REQUEST_MS = 60000;      // a phone request is refused after this (design 7)
  // OK is ignored for this long after the overlay opens: a press (or a held
  // key) meant for the screen below must not allow a phone's request.
  var ALLOW_GUARD_MS = 700;
  var CLOCK_MS = 1000;         // countdown texts

  var appDir = decodeURIComponent(window.location.pathname.replace(/\/[^\/]*$/, ''));
  var ctlPath = appDir + '/payload/ctl.sh';
  var bridges = [];
  var busy = false;            // an action from run() is in flight
  var autoUpdateTried = false;
  var tvAddress = '';
  var st = null;               // last `status --json`
  var tvData = null;           // last `tv-settings --json` (Home cards and Voice and power)
  var tvReading = false;
  var refreshTicks = 0;
  var screen = 'home';         // home | voice | ha | setup | diag | report | more
  var msg = { text: '', cls: '' };
  var armedBtn = null;         // id of an armed two-press button outside Voice and power
  var armToken = 0;
  var pairEnds = 0;            // local end time of the pairing window, 0 when not known

  function $(id) { return document.getElementById(id); }
  function text(el, s) { el.textContent = s; }
  function show(id, on) { $(id).hidden = !on; }
  function now() { return Date.now(); }
  function fmt(ms) {
    var s = Math.max(0, Math.ceil(ms / 1000));
    var r = s % 60;
    return Math.floor(s / 60) + ':' + (r < 10 ? '0' : '') + r;
  }

  // ---- Luna and ctl.sh --------------------------------------------------------

  // One Luna call through the web runtime's PalmServiceBridge (what LG's
  // webOSTV.js does inside webOS.service.request). The bridge object is kept
  // referenced until its reply arrives.
  function luna(uri, params, done) {
    if (!window.PalmServiceBridge) {
      done({ returnValue: false, errorText: 'not running on a webOS TV (PalmServiceBridge missing)' });
      return;
    }
    var bridge = new window.PalmServiceBridge();
    bridges.push(bridge);
    bridge.onservicecallback = function (m) {
      var res;
      try { res = JSON.parse(m); } catch (e) { res = { returnValue: false, errorText: String(m) }; }
      var i = bridges.indexOf(bridge);
      if (i >= 0) { bridges.splice(i, 1); }
      done(res);
    };
    bridge.call(uri, JSON.stringify(params));
  }

  function allowed(args) { return ALLOWED.indexOf(args) >= 0 || ANSWER_RE.test(args); }

  function ctl(args, done) {
    if (!allowed(args) || !/^\/[A-Za-z0-9._\/+-]+$/.test(ctlPath)) {
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

  // The last line of the output that is a JSON object (notes go to stderr,
  // which the exec merges into the output).
  function parseJsonLine(out) {
    var lines = (out || '').replace(/\s+$/, '').split('\n');
    for (var i = lines.length - 1; i >= 0; i--) {
      if (lines[i].charAt(0) === '{') {
        try { return JSON.parse(lines[i]); } catch (e) { return null; }
      }
    }
    return null;
  }

  // ---- the message line ---------------------------------------------------------

  var MSG_IDS = ['h-msg', 'ha-msg', 'd-msg', 'x-msg'];
  function setMsg(t, cls) {
    msg = { text: t || '', cls: cls || '' };
    for (var i = 0; i < MSG_IDS.length; i++) {
      text($(MSG_IDS[i]), msg.text);
      $(MSG_IDS[i]).className = 'msg ' + msg.cls;
    }
  }

  // ---- focus: nearest element in the pressed direction ---------------------------

  var FOCUS = {
    home: ['h-notice-btn', 'h-mic', 'h-voice', 'h-ha', 'h-diag'],
    voice: ['v-row-ar', 'v-row-hl', 'v-row-keep', 'v-refresh', 'v-hint-btn'],
    ha: ['a-pair', 'a-phone', 'a-reset', 'a-endphone'],
    setup: ['q-done', 'q-again'],
    diag: ['d-check', 'd-report', 'd-ack', 'd-more'],
    report: ['r-back'],
    more: ['x-stop', 'x-start', 'x-boot-on', 'x-boot-off', 'x-crash', 'x-uninstall', 'x-purge'],
    overlay: ['m-allow', 'm-refuse']
  };

  function layer() { return overlayOpen() ? 'overlay' : screen; }

  function focusables() {
    var ids = FOCUS[layer()];
    var out = [];
    for (var i = 0; i < ids.length; i++) { if (!$(ids[i]).hidden) { out.push($(ids[i])); } }
    return out;
  }

  // The element nearest to `cur` in direction dir (left, right, up, down):
  // only elements entirely beyond cur's edge in that direction count (2 px
  // tolerance); the score is the distance between the centres along the
  // direction plus 2.2 times the offset across it. Returns null when there is
  // none (no wrapping).
  function nearest(cur, els, dir) {
    var a = cur.getBoundingClientRect();
    var ax = a.left + a.width / 2;
    var ay = a.top + a.height / 2;
    var best = null;
    var bestScore = Infinity;
    for (var i = 0; i < els.length; i++) {
      var e = els[i];
      if (e === cur) { continue; }
      var b = e.getBoundingClientRect();
      var bx = b.left + b.width / 2;
      var by = b.top + b.height / 2;
      var primary;
      var ortho;
      if (dir === 'right') { if (b.left < a.right - 2) { continue; } primary = bx - ax; ortho = Math.abs(by - ay); }
      else if (dir === 'left') { if (b.right > a.left + 2) { continue; } primary = ax - bx; ortho = Math.abs(by - ay); }
      else if (dir === 'down') { if (b.top < a.bottom - 2) { continue; } primary = by - ay; ortho = Math.abs(bx - ax); }
      else { if (b.bottom > a.top + 2) { continue; } primary = ay - by; ortho = Math.abs(bx - ax); }
      if (primary <= 0) { continue; }
      var score = primary + ortho * 2.2;
      if (score < bestScore) { bestScore = score; best = e; }
    }
    return best;
  }

  function inLayer(el) {
    var f = focusables();
    return f.indexOf(el) >= 0;
  }

  // Keeps the focus on a visible element of the current layer.
  function ensureFocus(prefer) {
    if (inLayer(document.activeElement)) { return; }
    if (prefer && $(prefer) && !$(prefer).hidden && inLayer($(prefer))) { $(prefer).focus(); return; }
    var f = focusables();
    if (f.length) { f[0].focus(); }
  }

  // ---- two-press buttons (outside Voice and power) ------------------------------

  var ARM_TEXT = {
    'a-reset': ['a-reset-t', 'a-reset-sub', 'Reset pairing', 'Unpairs this TV', 'Home Assistant then has to pair again.'],
    'x-uninstall': ['x-uninstall-t', 'x-uninstall-sub', 'Remove service (keep settings)', 'Asks twice', 'The satellite stops and is removed. Settings stay.'],
    'x-purge': ['x-purge-t', 'x-purge-sub', 'Remove service and all data', 'Asks twice', 'Removes the satellite, its settings and its pairing.']
  };

  function renderArmed() {
    for (var id in ARM_TEXT) {
      if (Object.prototype.hasOwnProperty.call(ARM_TEXT, id)) {
        var t = ARM_TEXT[id];
        var on = armedBtn === id;
        text($(t[0]), on ? 'Press OK again' : t[2]);
        text($(t[1]), on ? t[4] : t[3]);
        $(id).className = 'act' + (id.charAt(0) === 'x' ? ' plain' : '') + (on ? ' armed' : '');
      }
    }
  }

  function disarmAll() {
    var changed = !!armedBtn || !!vArmed;
    armedBtn = null;
    armToken++;
    vArmed = null;
    vArmToken++;
    if (changed) { renderArmed(); renderVoice(); }
  }

  // ---- status, Home and the screens built from it --------------------------------

  function refresh() {
    ctl('status --json', function (r) {
      var s = parseJsonLine(r.out);
      if (!s) {
        st = null;
        setMsg('Cannot reach the Homebrew Channel: ' + (r.err || lastLine(r.out) || 'no answer from ' + HBC_EXEC + '.'), 'bad');
        renderAll();
        return;
      }
      st = s;
      if (s.pairing && s.pairing.open === true && typeof s.pairing.ends_in_s === 'number') { pairEnds = now() + s.pairing.ends_in_s * 1000; }
      else if (s.pairing) { pairEnds = 0; }
      renderAll();
      if (!tvData && s.uid === 0 && s.installed) { readTv(); }
      // An update installed through the Homebrew Channel or luna-send is applied
      // when the app is opened (also at the next boot, when start at boot is on).
      if (s.uid === 0 && s.installed && s.update_pending && !autoUpdateTried) {
        autoUpdateTried = true;
        run('install', 'Applying update ' + (s.installed_version || '?') + ' -> ' + (s.package_version || '?') + '...');
      }
    });
  }

  // tv-settings for the Home cards (the Voice and power screen has its own read).
  function readTv() {
    if (tvReading || vReading) { return; }
    tvReading = true;
    ctl('tv-settings --json', function (r) {
      tvReading = false;
      var d = parseJsonLine(r.out);
      tvData = d && typeof d === 'object' ? d : null;
      renderHome();
    });
  }

  var root = function () { return !!st && st.uid === 0; };
  var installed = function () { return root() && !!st.installed; };

  // The facts of tv-settings with the fresher package facts of status.
  function effectiveTv() {
    var d = {};
    var k;
    if (tvData) { for (k in tvData) { if (Object.prototype.hasOwnProperty.call(tvData, k)) { d[k] = tvData[k]; } } }
    if (st && st.installed) {
      d.daemon_running = !!st.running;
      var mic = {};
      var m = d.mic || {};
      for (k in m) { if (Object.prototype.hasOwnProperty.call(m, k)) { mic[k] = m[k]; } }
      mic.kill_switch = !!st.mic_off || mic.kill_switch === true;
      d.mic = mic;
    }
    return d;
  }

  // The one notice on Home (most important first): [text, tone, button label, command, then].
  function homeNotice(s) {
    if (!s) { return null; }
    if (s.uid !== 0) {
      return ['The Homebrew Channel is not running as root. This app needs a rooted TV: open the Homebrew Channel settings and check "Root status".', 'bad', '', '', ''];
    }
    if (!s.installed) {
      return ['Not set up yet. Set up installs the service under ' + s.prefix + ', starts it and opens a 10-minute pairing window for Home Assistant.', 'accent', 'Set up', 'install', 'start'];
    }
    if (s.update_pending) {
      return ['An update is ready: ' + (s.installed_version || '?') + ' to ' + (s.package_version || '?') + '.', 'accent', 'Apply update', 'install', ''];
    }
    if (s.crashloop) {
      return ['The satellite stopped after repeated crashes. See the log in Diagnostics, then clear the marker.', 'bad', 'Clear marker', 'clear-crashloop', ''];
    }
    if (!s.running) {
      if (s.disabled) { return ['The satellite is stopped. Starts are refused while ' + s.prefix + '/disable exists.', 'warn', '', '', '']; }
      return ['The satellite is stopped.', 'warn', 'Start', 'start', ''];
    }
    if (!s.firmware_ok) { return ['The microphone stays off: strict firmware policy and this firmware is not in the list.', 'warn', '', '', '']; }
    var c = VP.compat(s);
    if (c && c.short) { return [c.short, c.tone, '', '', '']; }
    return null;
  }

  function setCard(prefix, value, detail, tone, live) {
    $(prefix + '-card').className = 'scard tone-' + tone;
    text($(prefix + '-value'), value);
    text($(prefix + '-detail'), detail);
    if ($(prefix + '-dot')) { $(prefix + '-dot').className = 'dot' + (live ? ' live' : ''); }
  }

  function haDetail(h) {
    if (h.key === 'pairing') { return 'Add it in Home Assistant. ' + fmt(pairEnds - now()) + ' left.'; }
    return h.full;
  }

  function renderHome() {
    var s = st;
    var known = !!s && installed();
    // Listening and Power: from tv-settings, corrected by status.
    if (known && tvData) {
      var o = VP.outcome(effectiveTv());
      setCard('h-listen', o.listenShort, o.listenDetail, o.listenTone, o.listenCell === 'ok');
      setCard('h-power', o.powerShort, o.detail + (o.extra ? ' ' + o.extra : ''), o.powerTone, false);
    } else if (known && !s.running) {
      var l = VP.LISTEN.stopped;
      setCard('h-listen', l.short, l.detail, l.tone, false);
      setCard('h-power', 'Checking', '', 'idle', false);
    } else {
      setCard('h-listen', s && !known ? 'No' : 'Checking', s && !known ? 'The satellite is not set up.' : '', 'idle', false);
      setCard('h-power', s && !known ? 'Not known' : 'Checking', '', 'idle', false);
    }
    var h = VP.haCard(s || {});
    setCard('h-ha', s ? h.short : 'Checking', s ? haDetail(h) : '', s ? h.tone : 'idle', false);

    // the chip in the top bar
    var lt = known && tvData ? VP.outcome(effectiveTv()) : null;
    var listening = !!lt && lt.listenCell === 'ok';
    $('chip-listen').className = 'chip tone-' + (lt ? lt.listenTone : 'idle');
    $('chip-dot').className = 'dot' + (listening ? ' live' : '');
    text($('chip-text'), lt ? (listening ? 'Listening' : 'Not listening') : (s && !known ? 'Not listening' : 'Checking'));

    var n = homeNotice(s);
    show('h-notice', !!n);
    if (n) {
      $('h-notice').className = 'notice tone-' + n[1];
      text($('h-notice-text'), n[0]);
      show('h-notice-btn', !!n[2]);
      text($('h-notice-btn'), n[2]);
      $('h-notice-btn').setAttribute('data-cmd', n[3]);
      $('h-notice-btn').setAttribute('data-then', n[4]);
    } else {
      show('h-notice-btn', false);
    }

    var micOff = !!s && !!s.mic_off;
    show('h-mic', root());
    show('h-voice', installed());
    show('h-mic-on', !micOff);
    show('h-mic-off', micOff);
    text($('h-mic-pill'), micOff ? 'Off' : 'On');
    $('h-mic-pill').className = 'pill' + (micOff ? ' off' : '');
    text($('h-mic-sub'), micOff ? 'OK turns it on' : 'OK turns it off');
    text($('h-ha-sub'), (tvAddress ? tvAddress + ', port ' : 'Port ') + ((s && s.port) || '6053'));
    var c = VP.compat(s);
    text($('h-diag-sub'), c ? 'Firmware: ' + c.tag.toLowerCase() : (s && s.installed ? 'Version ' + (s.installed_version || '?') : 'Version, firmware, log'));
    if (screen === 'home' && s) { ensureFocus('h-mic'); }
  }

  function renderHA() {
    var s = st || {};
    var h = VP.haSummary(s);
    $('ha-head').className = 'ha-head tone-' + h.tone;
    text($('ha-value'), st ? h.short : 'Checking');
    text($('ha-sub'), st ? h.full : '');
    show('ha-count', h.key === 'pairing');
    text($('ha-count-time'), fmt(pairEnds - now()));
    text($('ha-addr'), tvAddress || 'unknown');
    text($('ha-port'), s.port || '6053');
    text($('ha-steps'), VP.HA_STEPS);
    var inst = installed();
    show('a-pair', inst && !!s.running && (s.key === 'unpaired' || s.key === 'cleared') && h.key !== 'pairing');
    show('a-reset', inst && !!s.running && s.key === 'paired');
    show('a-phone', inst);
    var live = setupLive();
    show('a-endphone', live);
    text($('a-phone-t'), live ? 'Show the phone code' : 'Set up with your phone');
    text($('a-phone-sub'), live ? (setup.state === 'active' ? 'A phone is connected' : 'The code is still open') : (setup.unavailable ? 'Not available in this version' : 'Shows a code to scan'));
    $('a-phone').className = 'act' + ($('a-pair').hidden ? ' lead' : '');
    text($('a-endphone-sub'), setupSessionText());
    renderArmed();
    if (screen === 'ha') { ensureFocus($('a-pair').hidden ? 'a-phone' : 'a-pair'); }
  }

  function checkTag(c) {
    if (!c) { return ['Not run', 'idle']; }
    var t = c.selfcheck || '';
    if (c.capture_problem || /^fail/.test(t)) { return ['Failed', 'bad']; }
    if (/^pass/.test(t)) { return ['Passed', 'ok']; }
    if (/^run|check/.test(t)) { return ['Running', 'idle']; }
    return ['Not run', 'idle'];
  }

  function renderDiag() {
    var s = st || {};
    var inst = installed();
    text($('d-version'), (s.installed_version || s.package_version || '?') + (s.update_pending ? ' (package ' + (s.package_version || '?') + ' waiting)' : '') + (s.build ? ', ' + s.build : ''));
    text($('d-fw'), s.firmware || '?');
    var c = VP.compat(s);
    show('d-fw-tag', !!c);
    if (c) { text($('d-fw-tag'), c.tag); $('d-fw-tag').className = 'tag ' + c.tone; }
    var ct = checkTag(s.compat);
    text($('d-check-tag'), ct[0]);
    $('d-check-tag').className = 'tag ' + ct[1];
    text($('d-check-text'), s.compat ? (s.compat.selfcheck || '') : '');
    text($('d-service'), !s.installed ? 'Not set up' : (s.running ? 'Running' + (s.supervised ? ', supervised' : '') : 'Stopped') + (s.autostart ? ', starts at boot' : ', not at boot'));
    var vol = s.reply_volume ? s.reply_volume + ' %' : (s.reply_volume_default ? s.reply_volume_default + ' % (default)' : 'default');
    text($('d-device'), (s.name || '?') + ', reply volume ' + vol);
    text($('d-tv'), ((s.compat && s.compat.tv_model) || 'TV model not known') + ', ' + (s.machine || '?'));
    text($('d-log'), (s.log || []).slice(-4).join('\n') || '(no log yet)');
    show('d-check', inst && !!s.running && !!s.compat);
    show('d-report', inst);
    show('d-ack', inst && !!s.running && !!s.compat && !!s.compat.firmware_changed);
    $('d-report').className = 'act' + (c && c.name !== 'verified' ? ' lead' : '');
    show('d-compat', !!c && !!c.full);
    text($('d-compat'), c ? c.full : '');
    show('d-more', root());
    if (screen === 'diag') { ensureFocus('d-check'); }
  }

  function renderMore() {
    var s = st || {};
    var inst = installed();
    show('x-stop', inst && !!s.running);
    show('x-start', inst && !s.running);
    show('x-boot-on', inst && !s.autostart);
    show('x-boot-off', root() && !!s.autostart);
    show('x-crash', inst && !!s.crashloop);
    show('x-uninstall', inst);
    show('x-purge', root() && (!!s.installed || s.installed_version !== '' || s.key !== 'none'));
    renderArmed();
    if (screen === 'more') { ensureFocus('x-stop'); }
  }

  function renderAll() {
    renderHome();
    renderHA();
    renderDiag();
    renderMore();
    renderSetup();
    renderChrome();
  }

  // ---- screens -----------------------------------------------------------------------

  var CRUMB = { home: '', voice: 'Voice and power', ha: 'Home Assistant', setup: 'Home Assistant / Phone setup',
    diag: 'Diagnostics', report: 'Diagnostics / Report', more: 'Diagnostics / More actions' };
  var SECTIONS = ['home', 'voice', 'ha', 'setup', 'diag', 'report', 'more'];
  var FIRST = { home: 'h-mic', voice: 'v-row-ar', ha: 'a-pair', setup: 'q-done', diag: 'd-check', report: 'r-back', more: 'x-stop' };

  function renderChrome() {
    text($('crumb'), CRUMB[screen] ? '/ ' + CRUMB[screen] : '');
    var ov = overlayOpen();
    text($('key-ok'), ov ? 'Allow' : 'Select');
    show('key-back-box', ov || screen !== 'home');
    text($('key-back'), ov ? 'Refuse' : (PARENT[screen] ? PARENT_NAME[PARENT[screen][0]] : 'Home'));
    var live = setupLive();
    show('chip-phone', live);
    text($('chip-phone-text'), setup.state === 'active' ? 'Phone connected' : 'Phone code open');
  }

  function go(name, focusId) {
    disarmAll();
    var from = screen;
    if (from === 'voice' && name !== 'voice') { voiceOpen = false; }
    screen = name;
    for (var i = 0; i < SECTIONS.length; i++) { show(SECTIONS[i], SECTIONS[i] === name); }
    if (name !== 'voice') { renderAll(); }
    renderChrome();
    if (!overlayOpen()) {
      var f = focusId && $(focusId) && !$(focusId).hidden ? focusId : null;
      if (f) { $(f).focus(); } else { ensureFocus(FIRST[name]); }
    }
  }

  var HOME_FOCUS = { voice: 'h-voice', ha: 'h-ha', setup: 'h-ha', diag: 'h-diag', report: 'h-diag', more: 'h-diag' };
  function goHome() {
    var from = screen;
    go('home', HOME_FOCUS[from]);
    refresh();
    if (from === 'voice') { /* tvData is fresh from the screen */ } else if (installed()) { readTv(); }
  }

  // An action through ctl.sh with a result line; `then` runs next on success.
  function run(args, label, then, okText) {
    if (busy) { return; }
    busy = true;
    setMsg(label || 'Working...', '');
    ctl(args, function (r) {
      busy = false;
      if (r.ok && then) {
        run(then, null, null, okText);
        return;
      }
      if (r.ok && okText) { setMsg(okText, 'ok'); }
      else { setMsg(lastLine(r.out) || r.err || (r.ok ? 'Done.' : 'Failed.'), r.ok ? '' : 'bad'); }
      refresh();
    });
  }

  function twoPress(id, args, label) {
    if (armedBtn !== id) {
      disarmAll();
      armedBtn = id;
      armToken++;
      var token = armToken;
      renderArmed();
      setTimeout(function () { if (armToken === token && armedBtn === id) { armedBtn = null; renderArmed(); } }, ARM_MS);
      return;
    }
    armedBtn = null;
    armToken++;
    renderArmed();
    run(args, label);
  }

  // ---- compatibility report ------------------------------------------------------------

  function drawQr(canvasId, url, ecc) {
    var canvas = $(canvasId);
    var ctx = canvas.getContext('2d');
    var qr = window.qrcodegen.QrCode.encodeText(url, ecc || window.qrcodegen.QrCode.Ecc.LOW);
    var border = 2;
    var cells = qr.size + border * 2;
    var scale = Math.max(1, Math.floor(QR_SIZE / cells));
    var px = cells * scale;
    canvas.width = px;
    canvas.height = px;
    canvas.style.width = QR_SIZE + 'px';
    canvas.style.height = QR_SIZE + 'px';
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

  function clearQr(canvasId) {
    var canvas = $(canvasId);
    var ctx = canvas.getContext('2d');
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
  }

  function openReport() {
    if (busy) { return; }
    busy = true;
    setMsg('Building the report...', '');
    ctl('report --json', function (r) {
      busy = false;
      var rep = parseJsonLine(r.out);
      if (!rep) {
        setMsg(lastLine(r.out) || r.err || 'The report could not be built.', 'bad');
        return;
      }
      setMsg('', '');
      text($('r-error'), '');
      text($('r-title'), rep.title || '');
      text($('r-url'), rep.url || '');
      text($('r-text'), rep.full || rep.short || '');
      $('r-text').scrollTop = 0;
      if (rep.error || !rep.url) {
        clearQr('r-qr');
        text($('r-error'), 'The report was withheld: ' + (rep.error || 'no link') + '. See ctl.sh report over SSH.');
      } else {
        try { drawQr('r-qr', rep.url); } catch (e) { text($('r-error'), 'The QR code could not be drawn (' + e + '). Type the address instead.'); }
      }
      go('report', 'r-back');
    });
  }

  // ---- Voice and power screen ------------------------------------------------------------
  // Design: docs/design/voice-and-power-screen.md and ADR 0008. The screen
  // reads `ctl.sh tv-settings --json` and changes one setting per press with
  // `ctl.sh set ...` (two presses for every LG change and for turning Keep
  // listening on, one to turn it off). The texts come from voicepower.js.

  var VROWS = [
    { id: 'ar', btn: 'v-row-ar', state: 'v-ar-state', sw: 'v-ar-sw', confirm: 'v-ar-confirm' },
    { id: 'hl', btn: 'v-row-hl', state: 'v-hl-state', sw: 'v-hl-sw', confirm: 'v-hl-confirm' },
    { id: 'keep', btn: 'v-row-keep', state: 'v-keep-state', sw: 'v-keep-sw', confirm: 'v-keep-confirm' }
  ];
  var voiceOpen = false;
  var vData = null;     // last tv-settings reply (null: not read or unreadable)
  var vLoaded = false;  // a read has finished at least once since the screen opened
  var vArmed = null;    // id of the row waiting for the second press
  var vArmToken = 0;
  var vPending = null;  // id of the row whose command runs
  var vReading = false;
  var vExpect = null;   // { id, kind } to compare with the re-read after a change
  var vMsg = '';
  var vMsgCls = '';

  function vModels() { return VP.rows(vLoaded ? (vData || {}) : null); }

  function vFocusedRow() {
    for (var i = 0; i < VROWS.length; i++) { if (document.activeElement === $(VROWS[i].btn)) { return i; } }
    return -1;
  }

  function renderVoice() {
    var m = vModels();
    var o = vLoaded ? VP.outcome(vData || {}) : null;
    text($('v-headline'), o ? o.headline : 'Checking...');
    text($('v-detail'), o ? (o.detail + (o.extra ? ' ' + o.extra : '')) : '');
    text($('v-listening'), o ? o.listening : '');
    $('v-listen-dot').className = 'dot' + (o ? ' ' + o.listenTone + (o.listenCell === 'ok' ? ' live' : '') : '');
    text($('v-notes'), o ? o.notes.join('\n') : '');
    for (var i = 0; i < VROWS.length; i++) {
      var r = VROWS[i];
      var b = $(r.btn);
      var armed = vArmed === r.id;
      var busyRow = vPending === r.id;
      var inert = !m[i].can || !!vPending;
      text($(r.state), busyRow ? 'Changing...' : (armed ? 'Press OK again' : m[i].word));
      $(r.state).className = 'vr-state ' + (busyRow ? 'muted' : (armed ? 'armed' : m[i].cls));
      $(r.sw).className = 'sw' + (m[i].checked === 'true' ? ' on' : (m[i].checked === 'mixed' ? ' mixed' : '')) + (m[i].can ? '' : ' na');
      b.className = 'vrow' + (armed ? ' armed' : '') + (inert ? ' is-disabled' : '');
      b.setAttribute('aria-checked', m[i].checked);
      b.setAttribute('aria-disabled', inert ? 'true' : 'false');
      show(r.confirm, armed);
      text($(r.confirm + '-text'), armed ? m[i].confirm : '');
    }
    var f = vFocusedRow();
    text($('v-warning'), !vArmed && f >= 0 ? m[f].warn : (document.activeElement === $('v-refresh') && !vArmed ? 'Reads the TV settings again.' : ''));
    text($('v-message'), vMsg);
    $('v-message').className = 'msg ' + vMsgCls;
    // Keep listening is on but Always Ready is off: offer to turn it on.
    var hint = vLoaded && vData ? VP.hint(vData) : null;
    var hintOn = !!hint && m[0].can;
    show('v-hint', hintOn);
    show('v-hint-btn', hintOn);
    text($('v-hint-text'), hintOn ? hint.text : '');
    text($('v-hint-btn'), hintOn ? hint.label : '');
    if (!hintOn && document.activeElement === $('v-hint-btn')) { ensureFocus('v-row-keep'); }
  }

  function vDisarm() {
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
      var d = parseJsonLine(r.out);
      if (d && typeof d === 'object') { tvData = d; }
      if (!voiceOpen) { return; }
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
      vData.remote_off = { state: VP.wordOf('keep', after) === 'On' ? 'keep_listening' : 'normal', active: ro.active };
    }
    tvData = vData;
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
      var m = VP.resultMessage(id, res, model);
      vMsg = m[0];
      vMsgCls = m[1];
      vExpect = null;
      if (res && res.after && (res.result === 'applied' || res.result === 'already' || res.result === 'mismatch')) {
        applyLocal(id, res.after);
        if (vData) { vExpect = { id: id, kind: VP.rowKind(id, vData) }; }
      }
      renderVoice();
      // One more read a little later catches LG (or another writer) changing it back.
      setTimeout(function () {
        if (!voiceOpen || vPending || vArmed) { return; }
        readVoice(function (d) {
          if (!d || !vExpect || vExpect.id !== id) { return; }
          var now2 = VP.rowKind(id, d);
          if (now2 !== vExpect.kind) {
            var w = vModels()[VP.rowIndex(id)].word;
            vMsg = id === 'keep' ? 'Keep listening was changed to ' + w + ' by someone else.' : 'LG changed ' + VP.ROWS[VP.rowIndex(id)].label + ' back to ' + w + '.';
            vMsgCls = 'warn';
          }
          vExpect = null;
        });
      }, REREAD_MS);
    });
  }

  function pressRow(id) {
    if (vPending || overlayOpen()) { return; }
    var m = vModels()[VP.rowIndex(id)];
    if (!m.can) { renderVoice(); return; }
    if (m.confirm && vArmed !== id) {
      vArmed = id;
      vArmToken++;
      var token = vArmToken;
      vMsg = '';
      renderVoice();
      setTimeout(function () { if (vArmToken === token && vArmed === id) { vDisarm(); } }, ARM_MS);
      return;
    }
    doChange(id, m);
  }

  function openVoice() {
    voiceOpen = true;
    vData = null;
    vLoaded = false;
    vArmed = null;
    vPending = null;
    vMsg = '';
    vMsgCls = '';
    go('voice', 'v-row-ar');
    renderVoice();
    readVoice(null);
  }

  // ---- phone setup (docs/design/phone-setup.md, ADR 0009) -----------------------------
  // `setup start` starts the server; `setup status --json` is polled once a
  // second from every screen while a session is starting, waiting or active
  // (the poll is the heartbeat); `setup answer ID allow|refuse` answers the
  // pending request; `setup stop` ends the session (Done).

  var LIVE = { starting: true, waiting: true, active: true };
  var setup = {
    state: 'none', data: null, polling: false, pollFails: 0,
    waitingSince: 0, expiresAt: 0, error: '', runningShown: false, seq: 0, appliedSeq: 0,
    awaiting: false,         // a request was seen and its result is not shown yet (fast poll)
    seenResult: undefined,   // id of the last `last_result` already shown (undefined: none seen yet)
    overlayId: null, overlayEnds: 0, overlayShownAt: 0, overlayPrevFocus: null, answered: {}
  };

  function setupLive() { return !!LIVE[setup.state]; }
  function overlayOpen() { return !!setup.overlayId; }

  function setupSessionText() {
    var d = setup.data || {};
    var left = setup.expiresAt ? fmt(setup.expiresAt - now()) : '';
    if (setup.state === 'active') { return 'Phone ' + (VP.isIPv4(d.peer) ? d.peer : '') + (left ? ', ' + left + ' left' : ''); }
    return left ? 'Code open, ' + left + ' left' : 'Code open';
  }

  function validPort(p) { return typeof p === 'number' && p % 1 === 0 && p > 0 && p < 65536; }
  function validPath(p) { return typeof p === 'string' && /^\/s\/[A-Za-z0-9_-]{16,128}$/.test(p); }

  // The QR URL: the app's own address plus the server's port and one-time
  // path (design 5.4); '' when any part is missing or malformed. Never logged.
  function setupUrl() {
    var d = setup.data || {};
    if (setup.state !== 'waiting' || !validPort(d.port) || !validPath(d.path) || !VP.isIPv4(tvAddress)) { return ''; }
    return 'http://' + tvAddress + ':' + d.port + d.path;
  }

  // Replies of setup calls can cross (the first poll and a start): each call
  // takes a sequence number and an older reply than the last applied is dropped.
  function setupCall(args, done) {
    var seq = ++setup.seq;
    ctl(args, function (r) {
      var fresh = seq > setup.appliedSeq;
      if (fresh) { setup.appliedSeq = seq; }
      done(r, fresh);
    });
  }

  // Takes one state object from `setup start|status|stop` (schema 1).
  function applySetup(d) {
    if (!d || typeof d !== 'object' || typeof d.state !== 'string') { return; }
    var prev = setup.state;
    setup.data = d;
    setup.state = d.state;
    if (typeof d.expires_in_s === 'number') { setup.expiresAt = now() + d.expires_in_s * 1000; }
    if (d.state === 'waiting' && prev !== 'waiting') { setup.waitingSince = now(); }
    handleResult(d.last_result || null);
    handlePending(d.pending || null);
    if (!setupLive()) { setup.awaiting = false; }
    if (d.state === 'ended' && LIVE[prev]) {
      setMsg(VP.endedText(d.reason), d.reason === 'done' ? '' : 'warn');
      setup.error = '';
    }
    if (setupLive()) { schedulePoll(); }
    renderSetup();
    renderHA();
    renderChrome();
  }

  function handleResult(lr) {
    var id = lr && typeof lr.id === 'string' ? lr.id : null;
    if (setup.seenResult === undefined) { setup.seenResult = id; return; }
    if (!id || id === setup.seenResult) { return; }
    setup.seenResult = id;
    setup.awaiting = false;
    var t = VP.phoneResult(lr);
    if (!t) { return; }
    setMsg(t[0], t[1]);
    if (voiceOpen) { vMsg = t[0]; vMsgCls = t[1]; }
    // No extra `status --json` (about 950 ms per call on c3-a, EXP-012): the
    // microphone switch is known from the result itself; the LG settings are
    // read again because the screens show them.
    var applied = lr.result === 'ok' || lr.result === 'applied';
    if (lr.action === 'mic' && applied && st && (lr.verdict === 'allowed' || lr.verdict === 'auto')) {
      st.mic_off = lr.value === 'off';
      renderAll();
    }
    if (applied && (lr.action === 'always-ready' || lr.action === 'hi-lg' || lr.action === 'remote-off' || lr.action === 'mic')) {
      if (voiceOpen) { readVoice(null); } else if (installed()) { readTv(); }
    }
  }

  function handlePending(p) {
    if (p && typeof p === 'object') { setup.awaiting = true; }
    if (!p || typeof p !== 'object' || p.state !== 'pending' || p.confirm !== true) {
      if (overlayOpen()) { closeOverlay(); }
      if (p && typeof p === 'object' && p.state === 'running') {
        var rt = VP.requestText(p.action, p.value);
        if (rt && !setup.runningShown) { setMsg('From the phone: ' + rt.ask + '. Changing...', ''); }
        setup.runningShown = true;
      } else {
        setup.runningShown = false;
      }
      return;
    }
    setup.runningShown = false;
    var id = p.id;
    if (typeof id !== 'string' || !ID_RE.test(id) || setup.answered[id]) { return; }
    var t = VP.requestText(p.action, p.value);
    if (!t || !VP.isIPv4(p.peer)) {
      // Not a request this app can put into words: refuse it without asking.
      if (overlayOpen()) { closeOverlay(); }
      setup.answered[id] = true;
      setMsg('Refused a phone request this app does not know.', 'warn');
      ctl('setup answer ' + id + ' refuse', function () {});
      return;
    }
    var left = typeof p.timeout_in_s === 'number' && p.timeout_in_s >= 0 ? Math.min(p.timeout_in_s * 1000, REQUEST_MS) : REQUEST_MS;
    if (setup.overlayId === id) {
      setup.overlayEnds = Math.min(setup.overlayEnds, now() + left);
      renderOverlay();
      return;
    }
    if (overlayOpen()) { closeOverlay(); }
    // The overlay wins over anything armed on the screen below.
    disarmAll();
    setup.overlayId = id;
    setup.overlayEnds = now() + left;
    setup.overlayShownAt = now();
    setup.overlayPrevFocus = document.activeElement;
    text($('m-from'), 'Phone ' + p.peer + ' wants to:');
    text($('m-what'), t.ask);
    text($('m-detail'), t.detail);
    show('overlay', true);
    renderOverlay();
    renderChrome();
    $('m-allow').focus();
  }

  function renderOverlay() {
    if (!overlayOpen()) { return; }
    text($('m-time'), fmt(setup.overlayEnds - now()));
  }

  function closeOverlay() {
    setup.overlayId = null;
    show('overlay', false);
    renderChrome();
    var prev = setup.overlayPrevFocus;
    setup.overlayPrevFocus = null;
    if (prev && prev.focus && inLayer(prev)) { prev.focus(); } else { ensureFocus(FIRST[screen]); }
  }

  function answer(verdict) {
    var id = setup.overlayId;
    if (!id) { return; }
    // Only a deliberate OK allows: never by itself, never at the moment the
    // overlay opens. Refusing is always possible.
    if (verdict !== 'refuse' && (verdict !== 'allow' || now() - setup.overlayShownAt < ALLOW_GUARD_MS)) { return; }
    setup.answered[id] = true;
    setup.awaiting = true;
    closeOverlay();
    setMsg(verdict === 'allow' ? 'Allowed. Changing...' : 'Refused the phone\'s request.', '');
    ctl('setup answer ' + id + ' ' + verdict, function (r) {
      if (!r.ok) { setMsg(lastLine(r.out) || r.err || 'The answer could not be sent.', 'bad'); }
    });
  }

  function schedulePoll() {
    if (setup.polling) { return; }
    setup.polling = true;
    setTimeout(pollSetup, setup.awaiting || overlayOpen() ? SETUP_POLL_FAST_MS : SETUP_POLL_MS);
  }

  // While a poll is scheduled or in flight `polling` is true, so there is
  // only ever one chain.
  function pollSetup() {
    if (setup.polling && !setupLive()) { setup.polling = false; return; } // ended or stopped meanwhile
    setup.polling = true;
    setupCall('setup status --json', function (r, fresh) {
      setup.polling = false;
      var d = parseJsonLine(r.out);
      if (d && fresh) { setup.pollFails = 0; applySetup(d); }
      else if (!d) { setup.pollFails++; }
      if (setupLive()) { schedulePoll(); }
    });
  }

  function startSetup() {
    setup.error = '';
    setup.unavailable = false;
    if (setupLive()) { go('setup', 'q-done'); return; }
    if (busy) { return; }
    busy = true;
    setup.state = 'starting';
    setup.data = null;
    go('setup', 'q-done');
    setupCall('setup start', function (r) {
      busy = false;
      var d = parseJsonLine(r.out);
      if (d && typeof d.state === 'string') {
        applySetup(d);
      } else {
        setup.state = 'none';
        var why = lastLine(r.out) || r.err || 'no answer';
        // This version has no setup server (an ipk built without it): say so plainly.
        if (/setup server is missing/.test(why)) {
          setup.unavailable = true;
          setup.error = VP.SETUP_UNAVAILABLE + '.';
        } else {
          setup.error = 'The phone setup could not start: ' + why + '.';
        }
        renderSetup();
        renderChrome();
      }
    });
  }

  function stopSetup(then) {
    var wasLive = setupLive();
    if (overlayOpen()) { closeOverlay(); }
    if (!wasLive) { if (then) { then(); } return; }
    setupCall('setup stop', function (r) {
      var d = parseJsonLine(r.out);
      setup.state = d && typeof d.state === 'string' ? d.state : 'none';
      setup.data = d;
      setMsg('The phone session ended.', '');
      renderSetup();
      renderHA();
      renderChrome();
      if (then) { then(); }
    });
  }

  var drawnUrl = '';
  function renderSetup() {
    var d = setup.data || {};
    var s = setup.state;
    var waiting = s === 'waiting';
    var active = s === 'active';
    var url = setupUrl();
    var hasQr = !!url;
    if (hasQr && drawnUrl !== url) {
      try { drawQr('q-qr', url, window.qrcodegen.QrCode.Ecc.MEDIUM); drawnUrl = url; } catch (e) { drawnUrl = ''; }
    } else if (!hasQr && drawnUrl) {
      clearQr('q-qr');
      drawnUrl = '';
    }
    $('q-box').className = 'qr-wrap' + (active ? ' used' : (hasQr ? '' : ' empty'));
    show('q-used', active);
    var noQr = '';
    if (waiting && !hasQr) {
      noQr = VP.isIPv4(tvAddress) ? 'The code could not be made.' : 'This TV\'s address is not known, so there is no code.';
    } else if (s === 'starting') { noQr = 'Starting...'; }
    else if (!waiting && !active) { noQr = 'No code.'; }
    show('q-noqr', !!noQr);
    text($('q-noqr'), noQr);
    show('q-urlbox', waiting);
    text($('q-url'), hasQr ? url : (validPort(d.port) && validPath(d.path) ? 'port ' + d.port + ', path ' + d.path + ' on this TV\'s address' : ''));
    show('q-count', waiting && !!setup.expiresAt);
    text($('q-time'), fmt(setup.expiresAt - now()));
    show('q-conn', active);
    text($('q-conn'), active ? 'Phone ' + (VP.isIPv4(d.peer) ? d.peer : '') + ' connected. Session ends in ' + fmt(setup.expiresAt - now()) + '.' : '');
    show('q-other', active && d.peer_other_subnet === true);
    text($('q-other'), 'The phone connected from a different network (' + (VP.isIPv4(d.peer) ? d.peer : '?') + '). That is unusual: check who it is.');
    var hint = waiting && setup.waitingSince && now() - setup.waitingSince >= SETUP_HINT_MS;
    show('q-hint', !!hint);
    text($('q-hint'), 'Nothing has opened the link yet. Your phone must be on the same network as this TV. A guest network, a different VLAN, a Wi-Fi "client isolation" setting, or a VPN on the phone will block it. This TV: ' +
      (tvAddress || 'address not known') + (validPort(d.port) ? ' (port ' + d.port + ')' : '') + '.');
    var line = '';
    if (setup.error) { line = setup.error; }
    else if (s === 'ended') { line = VP.endedText(d.reason); }
    else if (s === 'none' && screen === 'setup') { line = 'No phone session.'; }
    text($('q-state'), line);
    $('q-state').className = 'state-line' + (setup.error ? ' bad-line' : '');
    show('q-again', !LIVE[s] && !setup.unavailable);
    if (screen === 'setup') { ensureFocus('q-done'); }
  }

  // ---- input -----------------------------------------------------------------------------

  var DIRS = { 37: 'left', 38: 'up', 39: 'right', 40: 'down' };
  // Back. webOS TV (docs, "Back button": https://webostv.developer.lge.com/develop/guides/back-button):
  // by default the runtime handles Back through the DOM history and returns to the
  // launcher itself; an app that sets "disableBackHistoryAPI": true in appinfo.json
  // (build_ipk.py writes it) gets a keydown with keyCode 461 (0x1CD) instead and
  // decides. 27 (Escape) and 8 (Backspace) are only for testing in a desktop browser.
  function isBack(k) { return k === 27 || k === 8 || k === 461; }

  // A sub-screen goes back to its parent (the breadcrumb), every other screen to Home.
  var PARENT = { setup: ['ha', 'a-phone'], report: ['diag', 'd-report'], more: ['diag', 'd-more'] };
  var PARENT_NAME = { ha: 'Home Assistant', diag: 'Diagnostics' };

  // Back on Home leaves the app the documented way: webOS.platformBack() when
  // webOSTV.js is loaded (this app does not ship it; webOS TV 6.0+ then asks whether
  // to exit, 5.0 and lower open the launcher), else window.close(), which the same page
  // documents for an app's own exit. Only the remote's key leaves, never Escape or
  // Backspace. Not yet run on a TV (verified-on-target pending).
  function leaveApp() {
    try {
      if (window.webOS && typeof window.webOS.platformBack === 'function') { window.webOS.platformBack(); return; }
    } catch (e) { /* fall through to close */ }
    if (typeof window.close === 'function') { window.close(); }
  }

  function goBack(k) {
    if (screen === 'home') {
      if (k === 461) { leaveApp(); }
      return;
    }
    var p = PARENT[screen];
    if (p) { go(p[0], p[1]); } else { goHome(); }
  }

  // Remote control: arrows move the focus, OK (Enter) presses the focused
  // button (the browser turns it into a click), Back returns to Home.
  document.addEventListener('keydown', function (ev) {
    var k = ev.keyCode;
    if (overlayOpen()) {
      if (isBack(k)) { answer('refuse'); ev.preventDefault(); return; }
      if (DIRS[k]) {
        var n = nearest(document.activeElement && inLayer(document.activeElement) ? document.activeElement : $('m-allow'), focusables(), DIRS[k]);
        if (n) { n.focus(); }
        ev.preventDefault();
      }
      return;
    }
    if (screen === 'report' && (k === 38 || k === 40)) {
      var box = $('r-text');
      box.scrollTop = k === 38 ? Math.max(0, box.scrollTop - 80) : box.scrollTop + 80;
      ev.preventDefault();
      return;
    }
    if (isBack(k)) {
      goBack(k);
      ev.preventDefault();
      return;
    }
    if (k === 13) { return; }
    if (DIRS[k]) {
      disarmAll();
      var cur = document.activeElement;
      if (!inLayer(cur)) { ensureFocus(FIRST[screen]); }
      else {
        var next = nearest(cur, focusables(), DIRS[k]);
        if (next) { next.focus(); }
      }
      if (screen === 'voice') { renderVoice(); }
      ev.preventDefault();
      return;
    }
    disarmAll();
  });

  function onClick(id, f) {
    $(id).addEventListener('click', f);
    // The pointer of the Magic Remote moves the focus like the arrows.
    $(id).addEventListener('mouseover', function () { if (inLayer($(id))) { $(id).focus(); if (screen === 'voice') { renderVoice(); } } });
  }

  onClick('h-notice-btn', function () {
    var b = $('h-notice-btn');
    var cmd = b.getAttribute('data-cmd');
    if (cmd) { run(cmd, b.textContent + '...', b.getAttribute('data-then') || null); }
  });
  onClick('h-mic', function () {
    if (!st) { return; }
    if (st.mic_off) { run('mic on', 'Turning the microphone on...', null, 'Microphone on.'); }
    else { run('mic off', 'Turning the microphone off...', null, 'Microphone off. The satellite cannot hear.'); }
  });
  onClick('h-voice', openVoice);
  onClick('h-ha', function () { go('ha'); });
  onClick('h-diag', function () { go('diag'); });

  VROWS.forEach(function (r) { onClick(r.btn, function () { pressRow(r.id); }); });
  // The hint arms the normal two-press Always Ready change: the focus moves to
  // that row, which shows its confirm text; OK on the row sends the command.
  onClick('v-hint-btn', function () {
    if (vPending || overlayOpen()) { return; }
    $('v-row-ar').focus();
    pressRow('ar');
  });
  onClick('v-refresh', function () {
    vDisarm();
    if (vPending) { return; }
    vMsg = '';
    vMsgCls = '';
    readVoice(null);
  });

  onClick('a-pair', function () {
    var w = st && /^[0-9]+$/.test(st.pairing_window_s || '') ? Math.round(+st.pairing_window_s / 60) : 10;
    run('pair', 'Opening the pairing window...', null, 'Pairing is open for ' + w + ' minutes. Add this TV in Home Assistant now.');
  });
  onClick('a-phone', startSetup);
  onClick('a-reset', function () { twoPress('a-reset', 'reset-pairing', 'Resetting the pairing...'); });
  onClick('a-endphone', function () { stopSetup(null); });

  onClick('q-done', function () { stopSetup(function () { go('ha', 'a-phone'); }); });
  onClick('q-again', startSetup);

  onClick('d-check', function () { run('selfcheck', 'Checking the microphone, about 10 seconds...'); });
  onClick('d-report', openReport);
  onClick('d-ack', function () { run('ack-firmware', 'Acknowledging the firmware...'); });
  onClick('d-more', function () { go('more'); });
  onClick('r-back', function () { go('diag', 'd-report'); });

  onClick('x-stop', function () { run('stop', 'Stopping...'); });
  onClick('x-start', function () { run('start', 'Starting...'); });
  onClick('x-boot-on', function () { run('autostart on', 'Turning start at boot on...'); });
  onClick('x-boot-off', function () { run('autostart off', 'Turning start at boot off...'); });
  onClick('x-crash', function () { run('clear-crashloop', 'Clearing the marker...'); });
  onClick('x-uninstall', function () { twoPress('x-uninstall', 'uninstall', 'Removing the service...'); });
  onClick('x-purge', function () { twoPress('x-purge', 'purge', 'Removing the service and all data...'); });

  onClick('m-allow', function () { answer('allow'); });
  onClick('m-refuse', function () { answer('refuse'); });

  // ---- clocks, refresh, start ----------------------------------------------------------

  // The page is drawn for 1920x1080; scale it when the runtime is smaller.
  function fit() {
    var w = window.innerWidth || 1920;
    var h = window.innerHeight || 1080;
    var s = Math.min(w / 1920, h / 1080);
    $('tv').style.transform = s > 0 && Math.abs(s - 1) > 0.01 ? 'scale(' + s + ')' : '';
  }
  if (window.addEventListener) { window.addEventListener('resize', fit); }
  fit();

  function clock() {
    if (overlayOpen()) {
      if (now() >= setup.overlayEnds) { answer('refuse'); setMsg('No answer on the TV: the phone\'s request was refused.', 'warn'); }
      else { renderOverlay(); }
    }
    if (pairEnds && now() >= pairEnds) { pairEnds = 0; refresh(); }
    if (screen === 'setup') { renderSetup(); }
    if (screen === 'ha') { renderHA(); }
    if (screen === 'home' && pairEnds) { renderHome(); }
  }

  // The 10 s refresh: on Voice and power a tv-settings read when nothing is
  // pending or armed; nothing while the report is open; elsewhere the status
  // (when no action runs), and on Home the TV settings every 3rd time.
  function tick() {
    if (screen === 'voice') { if (!vPending && !vArmed) { readVoice(null); } return; }
    if (screen === 'report') { return; }
    if (busy) { return; }
    refresh();
    refreshTicks++;
    if (screen === 'home' && installed() && refreshTicks % HOME_TV_EVERY === 0) { readTv(); }
  }

  function findAddress(idx) {
    if (idx >= CONN_SERVICES.length) { return; }
    luna(CONN_SERVICES[idx], {}, function (r) {
      if (r.returnValue === false) { findAddress(idx + 1); return; }
      var w = r.wired || {};
      var f = r.wifi || {};
      tvAddress = (w.state === 'connected' && w.ipAddress) || (f.state === 'connected' && f.ipAddress) || w.ipAddress || f.ipAddress || '';
      renderAll();
    });
  }

  for (var i = 0; i < SECTIONS.length; i++) { show(SECTIONS[i], SECTIONS[i] === 'home'); }
  show('overlay', false);
  renderAll();
  renderVoice();
  findAddress(0);
  refresh();
  // A session may still run from before the app was closed and reopened.
  pollSetup();
  setInterval(tick, REFRESH_MS);
  setInterval(clock, CLOCK_MS);

  // For the host tests only (no effect on the TV).
  window.voiceSatelliteApp = { nearest: nearest, FOCUS: FOCUS, SETUP_POLL_MS: SETUP_POLL_MS, SETUP_POLL_FAST_MS: SETUP_POLL_FAST_MS, REQUEST_MS: REQUEST_MS, allowed: allowed };
}());
