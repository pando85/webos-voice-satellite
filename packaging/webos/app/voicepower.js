/*
 * voicepower.js: the shared model of the Voice Satellite screens. ES5 only,
 * no DOM, no timers, no I/O: the TV app (window.voicePower), the phone setup
 * page (served by webos-satellite-setup, which embeds this same file) and the
 * node tests (module.exports) all run it unchanged.
 *
 * It holds every user-facing sentence about the TV's state and every phone
 * request text, so the TV and the phone always say the same thing:
 *   - the outcome matrix (what the remote's power button does, the listening
 *     line, the notes): docs/design/voice-and-power-screen.md sections 3 and 4;
 *   - the three Voice and power rows, their confirm texts and result lines;
 *   - the two Sound rows (echo cancellation, music), TV app only;
 *   - short card wording next to the full wording, in the same table entry
 *     (LISTEN, POWER, KEY, COMPAT), for the Home cards and the phone page;
 *   - the phone-request texts keyed by action and value
 *     (docs/design/phone-setup.md sections 7 and 9): the TV overlay shows only
 *     these, never text from the server.
 */
(function (root, factory) {
  'use strict';
  var api = factory();
  if (typeof module === 'object' && module.exports) { module.exports = api; }
  if (root) { root.voicePower = api; }
}(typeof window !== 'undefined' ? window : null, function () {
  'use strict';

  var AR_MENU = 'Settings, General, Always Ready';
  var HL_MENU = 'Settings, General, AI Service, Voice recognition settings, Hi LG';
  var AR_KNOWN = ['off', 'alwaysReady', 'allEnabled', 'none', 'lifeOnScreen'];

  // Tones (the dot and text colour of a card): ok, warn, bad, idle, accent.

  // What the remote's power button does, by Always Ready x "Keep listening".
  // P1 off+normal, P2 on+normal, P3 off+keep, P4 on+keep, PX not known.
  // headline + detail: the full sentence (Voice and power screen); short: the
  // Home card and the phone card.
  var POWER = {
    P1: { headline: 'Power button: the TV goes to deep standby.', detail: 'The satellite cannot hear until the TV is turned on.',
      short: 'The TV goes to deep standby.', tone: 'idle' },
    P2: { headline: 'Power button: the TV goes to standby.', detail: 'The satellite still hears you, but you will not hear its replies.',
      short: 'The TV goes to standby.', tone: 'warn' },
    P3: { headline: 'Power button: the TV turns itself back on with the screen dark.',
      detail: 'About 5 to 6 seconds after the key. The satellite hears you and replies; press power again to turn the picture on. The TV may sometimes fall asleep instead, and the satellite then stops until the TV is turned on. Turn Always Ready on to avoid that.',
      short: 'The TV turns back on, screen dark. May fall asleep.', tone: 'warn' },
    P4: { headline: 'Power button: the TV turns itself back on with the screen dark.',
      detail: 'About 5 to 6 seconds after the key. The satellite hears you and replies. Press power again to turn the picture on.',
      short: 'The TV turns back on, screen dark.', tone: 'ok' },
    PX: { headline: 'Power button: not known.', detail: 'The TV\'s settings could not be read. Press Refresh.',
      short: 'Not known.', tone: 'bad' }
  };
  var POWER_EXTRA = {
    kill: 'Keep listening does nothing while the microphone is off with the kill switch.',
    nowatch: 'Keep listening needs the power watch, which is off in the config.'
  };

  // The microphone line, first match wins (see outcome()). full: the Voice and
  // power screen; short + detail: the Listening card.
  var LISTEN = {
    stopped: { full: 'Listening now: no. The satellite is not running.', short: 'No, stopped', detail: 'The satellite is not running.', tone: 'bad' },
    kill: { full: 'Listening now: no. The microphone is off (kill switch).', short: 'No, microphone off', detail: 'Turned off on this TV.', tone: 'warn' },
    muted: { full: 'Listening now: no. The microphone is muted in Home Assistant.', short: 'No, muted', detail: 'Muted in Home Assistant.', tone: 'warn' },
    hilg: { full: 'Listening now: no. The satellite cannot hear: LG\'s voice assistant has the microphone. Turn Hi LG off to use the satellite.',
      short: 'No, busy', detail: 'LG\'s voice assistant has the microphone.', tone: 'bad' },
    busy: { full: 'Listening now: no. Another program has the microphone.', short: 'No, busy', detail: 'Another program has the microphone.', tone: 'bad' },
    ok: { full: 'Listening now: yes.', short: 'Yes', detail: 'The microphone is open.', tone: 'ok' },
    closed: { full: 'Listening now: no. The microphone is closed.', short: 'No, closed', detail: 'The microphone is closed.', tone: 'idle' },
    unknown: { full: 'Listening now: not known.', short: 'Not known', detail: 'The TV settings were not read yet.', tone: 'idle' }
  };
  var NOTE = {
    hilgListening: 'Hi LG is on but this satellite has the microphone now; after a restart LG may take it. Turn Hi LG off to be sure.',
    hilgOn: 'Hi LG is on. If the satellite cannot hear you, turn it off.',
    others: 'Alexa/other hands-free is on: turn it off in LG\'s menu.',
    unreadable: 'Always Ready or Hi LG could not be read, so the sentence above may be wrong.'
  };

  // The three rows of the Voice and power screen (and the phone page).
  var ROWS = [
    { id: 'ar', label: 'Always Ready', owner: 'LG', menu: AR_MENU, action: 'always-ready',
      sub: 'LG\'s setting. After the power button the TV waits in a light standby instead of suspending, so the satellite keeps hearing you. Uses more standby power.' },
    { id: 'hl', label: 'Hi LG', owner: 'LG', menu: HL_MENU, action: 'hi-lg',
      sub: 'LG\'s own voice assistant. When it is on, the satellite may not be able to use the microphone.' },
    { id: 'keep', label: 'Keep listening', owner: 'Satellite', menu: '', action: 'remote-off',
      sub: 'A satellite setting. After the power button the satellite turns the TV back on with the screen dark. You can still talk to it and hear replies. Press power again for the picture.' }
  ];
  // Shown while Keep listening is on and in effect but Always Ready is off
  // (and changeable): see hint().
  var HINT = { text: 'Works best with Always Ready on. Turn it on?', label: 'Turn on Always Ready' };
  var KEEP_BEST = ' Works best with Always Ready on.';
  var CONFIRM_TAIL = ' Press OK again to confirm.';

  // Home Assistant pairing, by the `key` word of `ctl.sh status --json`.
  // short: the Home card value and the Home Assistant screen headline;
  // full: the sentence under it.
  var KEY = {
    fixed: { short: 'Paired (fixed key)', full: 'Uses the fixed key from etc/noise_psk. Nothing to do here.', tone: 'ok' },
    paired: { short: 'Paired', full: 'Paired with Home Assistant. Nothing to do here.', tone: 'ok' },
    unpaired: { short: 'Not paired', full: 'Start pairing, then add this TV in Home Assistant with the address and port below.', tone: 'warn' },
    cleared: { short: 'Unpaired by Home Assistant', full: 'Home Assistant removed this device. Start pairing to pair again.', tone: 'warn' },
    none: { short: 'No key', full: 'No key and pairing off: check the config.', tone: 'bad' },
    pairing: { short: 'Waiting for pairing', full: 'Add this TV in Home Assistant now, with the address and port below.', tone: 'accent' },
    notinstalled: { short: 'Not set up', full: 'Set up the satellite first.', tone: 'idle' }
  };
  // The API connection, by `ha.state` of `ctl.sh status --json` (daemon 0.3.1+,
  // run/voice.json). short: the Home card value; full: the card detail.
  var HA_CONN = {
    connected: { short: 'Connected', full: 'Home Assistant is connected to this TV.', tone: 'ok' },
    disconnected: { short: 'Disconnected', full: 'Home Assistant is not connected now. It reconnects by itself.', tone: 'warn' },
    unpaired: { short: 'Not paired', full: 'Open Home Assistant here and start pairing.', tone: 'warn' },
    unknown: { short: 'Not known', full: 'The connection is not known now: the satellite is stopped or has not reported yet.', tone: 'idle' }
  };
  // Cleaning up Home Assistant's side. The satellite holds no Home Assistant
  // login, so it cannot delete its own entry there: the person has to.
  var HA_DELETE_PATH = 'Settings, Devices & services, ESPHome, this TV, Delete';
  var HA_CLEANUP = {
    // Reset pairing (TV confirm text, after 'Unpairs this TV.').
    reset: 'Home Assistant shows an error for this TV until it is paired again. Or delete this TV in Home Assistant (' + HA_DELETE_PATH + ') and add it again.',
    // Remove service, keep settings: the key stays, so the old entry still fits a later install.
    uninstall: 'Home Assistant keeps this TV and shows it as unavailable. Delete it there if you do not install again: ' + HA_DELETE_PATH + '.',
    // Remove service and all data.
    purge: 'Also delete this TV in Home Assistant: ' + HA_DELETE_PATH + '. The satellite cannot do that itself.',
    // The phone page, next to the link that opens ESPHome in Home Assistant.
    phone: 'Setting this TV up again? Delete the old entry in Home Assistant first: ' + HA_DELETE_PATH + '.'
  };
  var SETUP_UNAVAILABLE = 'Phone setup is not available in this version';
  var HA_STEPS = 'In Home Assistant: Settings, Devices & services, Add integration, ESPHome. Enter the address and the port. No key to type: Home Assistant sets it while pairing is open.';

  // The Home card for Home Assistant: the connection (`ha.state`), with an open
  // pairing window and a missing setup winning over it. Not the key word (that
  // is haSummary, for the Home Assistant screen).
  // { key, short, full, tone, pairingLeftS }; key is one of HA_CONN, 'pairing' or 'notinstalled'.
  function haCard(st) {
    st = st || {};
    var s = haSummary(st);
    if (s.key === 'pairing' || s.key === 'notinstalled') { return s; }
    var ha = st.ha && typeof st.ha === 'object' ? st.ha.state : '';
    var name = HA_CONN[ha] ? ha : 'unknown';
    var e = HA_CONN[name];
    return { key: name, short: e.short, full: e.full, tone: e.tone, pairingLeftS: null };
  }

  // Firmware compatibility (the `compat` object of `ctl.sh status --json`).
  // tag: the Diagnostics label; short: the Home notice; full: Diagnostics.
  var COMPAT = {
    verified: { tag: 'Verified', tone: 'ok', short: '', full: '' },
    series: { tag: 'Series verified', tone: 'ok', short: '',
      full: 'This TV series is verified; this firmware version has not been reported yet. If it works, a report helps everyone: press Report compatibility.' },
    notverified: { tag: 'Not verified', tone: 'warn', short: 'This TV and firmware are not verified yet. If the satellite does not work, open Diagnostics.',
      full: 'This TV and firmware are not in the project\'s verified list yet. The microphone runs anyway and is checked. If it does not work, press Report compatibility.' },
    changed: { tag: 'Firmware changed', tone: 'warn', short: 'The TV firmware changed. When the satellite works for you, acknowledge it in Diagnostics.',
      full: 'The TV firmware changed since the satellite last ran. The microphone keeps running and is being checked again. When it works for you, press Acknowledge firmware. If it does not, press Report compatibility.' },
    capture: { tag: 'Capture problem', tone: 'bad', short: 'The microphone check failed. Open Diagnostics.',
      full: 'The microphone check failed (%s). The microphone is closed and retried a few times. If LG hands-free voice is on, turn it off. Otherwise press Report compatibility so this TV and firmware can be looked at.' }
  };

  // ---- the outcome matrix --------------------------------------------------

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

  // The sentence for a `ctl.sh tv-settings --json` reply:
  // { cell, headline, detail, extra, listenCell, listening, notes,
  //   powerShort, powerTone, listenShort, listenDetail, listenTone }.
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

    var p = POWER[cell];
    var l = LISTEN[listenCell];
    return { cell: cell, headline: p.headline, detail: p.detail, extra: extra,
      listenCell: listenCell, listening: l.full, notes: notes,
      powerShort: p.short, powerTone: p.tone, listenShort: l.short, listenDetail: l.detail, listenTone: l.tone };
  }

  // ---- rows, confirm texts and result lines --------------------------------

  function rowIndex(id) { return id === 'ar' ? 0 : (id === 'hl' ? 1 : 2); }

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

  // arOn: true when Always Ready is known to be on (the Keep listening text
  // then leaves out the "works best" sentence).
  function confirmText(id, turningOn, arOn) {
    if (id === 'ar') {
      return turningOn
        ? 'Turn on Always Ready? After the power button the TV waits in a light standby, so the satellite keeps hearing you. With "Keep listening" on, it reliably turns the TV back on, dark, to reply. With it off, you hear no replies. The TV uses more standby power (not measured).'
        : 'Turn off Always Ready? Without "Keep listening", the TV suspends after the power button and the satellite stops. With it on, the satellite has to wake the TV within about 7 seconds, so the TV may sometimes fall asleep instead.';
    }
    if (id === 'hl') {
      return turningOn
        ? 'Turn on Hi LG? LG\'s assistant may take the microphone, so the satellite may not hear you. It did not while the satellite already had the microphone, but it can after a restart.'
        : 'Turn off Hi LG? The TV stops answering "Hi LG". The satellite can use the microphone.';
    }
    return turningOn
      ? 'Turn on Keep listening? About 5 to 6 seconds after the power button the satellite turns the TV back on with the screen dark. You can talk to it and hear replies; press power again for the picture. It uses more power than standby (not measured), LG\'s OLED care at power-off may not run, and the screen may flash briefly (not observed).' +
        (arOn === true ? '' : KEEP_BEST)
      : 'Keep listening is on. Press OK to turn it off. The power button then turns the TV off the normal way.';
  }

  // The three rows: { word, cls, checked, can, cmd, confirm, warn }.
  function rows(s) {
    function wait() { return { word: 'Checking...', cls: 'muted', checked: 'false', can: false, cmd: '', confirm: '', warn: '' }; }
    if (!s) { return [wait(), wait(), wait()]; }
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
    if (a.can) { a.warn = a.confirm; a.confirm = a.confirm + CONFIRM_TAIL; }

    if (hl.kind === 'on' || hl.kind === 'off') {
      var hon = hl.kind === 'on';
      h = { word: hon ? 'On' : 'Off', cls: hon ? 'warn' : '', checked: hon ? 'true' : 'false', can: true,
        cmd: hon ? 'set hi-lg off' : 'set hi-lg on', confirm: confirmText('hl', !hon) };
      h.warn = h.confirm;
      h.confirm = h.confirm + CONFIRM_TAIL;
    } else {
      h = { word: hl.kind === 'error' ? 'Cannot read' : 'Not known', cls: hl.kind === 'error' ? 'bad' : 'warn', checked: 'false',
        can: false, cmd: '', confirm: '', warn: menuNote('Hi LG could not be read' + (hl.error ? ': ' + hl.error + '.' : '.'), HL_MENU) };
    }

    if (ro.kind === 'unknown') {
      k = { word: 'Cannot read', cls: 'bad', checked: 'false', can: false, cmd: '', confirm: '', warn: 'Keep listening could not be read.' };
    } else if (ro.kind === 'keep') {
      k = { word: 'On', cls: 'warn', checked: 'true', can: true, cmd: 'set remote-off normal', confirm: '', warn: confirmText('keep', false) };
    } else {
      k = { word: 'Off', cls: '', checked: 'false', can: true, cmd: 'set remote-off keep', confirm: confirmText('keep', true, ar.kind === 'alwaysReady' || ar.kind === 'allEnabled') };
      k.warn = k.confirm;
      k.confirm = k.confirm + CONFIRM_TAIL;
    }
    return [a, h, k];
  }

  // The hint under the Voice and power screen (and on the phone page): Keep
  // listening is on and in effect (the daemon acts on it: no kill switch, a
  // power watch) while Always Ready is off. Only for raw `off`: a value this app
  // does not change (none, lifeOnScreen, allEnabled, unknown) gives no hint, since
  // the button could not do what it says. The action is the normal two-press
  // `set always-ready on`.
  // { text, label, action, value, cmd } or null.
  function hint(s) {
    s = s || {};
    var ar = arInfo(s.always_ready);
    var ro = roInfo(s.remote_off);
    var mic = s.mic || {};
    var kill = mic.kill_switch === true || mic.state === 'kill';
    if (ar.kind !== 'off' || ar.raw !== 'off' || ro.kind !== 'keep' || ro.noWatch || kill) { return null; }
    return { text: HINT.text, label: HINT.label, action: 'always-ready', value: 'on', cmd: 'set always-ready on' };
  }

  // One line for the result of a `set` (the JSON line of ctl.sh), as [text, class].
  function resultMessage(id, r, model) {
    var row = ROWS[rowIndex(id)];
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

  // ---- cards: Home Assistant and firmware ------------------------------------

  // The Home Assistant card from `ctl.sh status --json`:
  // { key, short, full, tone, pairingLeftS }. key is a KEY entry name.
  function haSummary(st) {
    st = st || {};
    var name;
    var p = st.pairing || null;
    var left = p && p.open === true && typeof p.ends_in_s === 'number' && p.ends_in_s > 0 ? p.ends_in_s : null;
    if (!st.installed) { name = 'notinstalled'; }
    else if (left !== null) { name = 'pairing'; }
    else { name = KEY[st.key] && st.key !== 'pairing' && st.key !== 'notinstalled' ? st.key : 'none'; }
    var e = KEY[name];
    return { key: name, short: e.short, full: e.full, tone: e.tone, pairingLeftS: left };
  }

  // Firmware compatibility from `ctl.sh status --json`:
  // { name, tag, tone, short, full } or null when there is no compat object.
  function compat(st) {
    var c = st && st.compat;
    if (!st || !st.installed || !c) { return null; }
    var name;
    if (c.capture_problem) { name = 'capture'; }
    else if (c.firmware_changed) { name = 'changed'; }
    else if (c.compat_status === 'verified') { name = 'verified'; }
    else if (c.compat_status === 'series verified (firmware not yet reported)') { name = 'series'; }
    else { name = 'notverified'; }
    var e = COMPAT[name];
    return { name: name, tag: e.tag, tone: e.tone, short: e.short,
      full: e.full.replace('%s', c.selfcheck || 'no details') };
  }

  // ---- Sound screen: echo cancellation, music and the remote mic button ------
  // Three keys of the satellite's config file, [aec] enabled (on by default,
  // ADR 0011), [music] enabled (off by default, ADR 0010) and [remote]
  // mic_button (off by default, ADR 0013), from `aec_enabled`, `music_enabled`
  // and `mic_button` of `ctl.sh status --json`. The mic button shows what
  // runs: on only while the satellite holds the button (LG's voice key taken
  // over); `mic_button_active` false with "ptt" configured means the takeover
  // failed, and `mic_button_error` says why.
  // The daemon reads them at start only, so `ctl.sh config aec|music on|off`
  // rewrites the config and restarts it. TV app only: the phone page does not
  // offer them (a restart ends the phone session), so REQUEST has no entry.

  var RESTART = 'Applying it restarts the voice service: the satellite stops listening for a few seconds.';
  var SOUND_NOTE = {
    head: 'A change restarts the voice service briefly.',
    detail: 'The satellite stops listening for a few seconds, then Home Assistant reconnects by itself. An open phone setup session ends.'
  };
  var SOUND_ROWS = [
    { id: 'aec', label: 'Echo cancellation', key: 'aec_enabled', action: 'aec',
      sub: 'Lets the TV hear you while it plays sound. Uses a little more CPU.' },
    { id: 'music', label: 'Music', key: 'music_enabled', action: 'music',
      sub: 'Lets Home Assistant or Music Assistant play music on this TV. Experimental.' },
    { id: 'mic', label: 'Remote mic button \u2192 Home Assistant', key: 'mic_button', action: 'mic-button',
      sub: 'Replaces LG\'s voice assistant on the mic button while on. LG\'s toast "not supported" may appear.' }
  ];

  function soundIndex(id) { return id === 'mic' ? 2 : (id === 'music' ? 1 : 0); }

  // The value of a row from the status: a boolean, or undefined when this
  // version does not report it. The mic button is on only while it runs.
  function soundValue(st, d) {
    if (d.id !== 'mic') { return typeof st[d.key] === 'boolean' ? st[d.key] : undefined; }
    if (typeof st.mic_button !== 'string') { return undefined; }
    return st.mic_button === 'ptt' && st.mic_button_active !== false;
  }

  // Configured but not running: the takeover of LG's voice key failed.
  function micFailed(st) { return !!st && st.mic_button === 'ptt' && st.mic_button_active === false; }

  // After a `config` result: the row's status value, until the next status read.
  function soundApply(st, id, after) {
    if (!st || (after !== 'on' && after !== 'off')) { return; }
    var d = SOUND_ROWS[soundIndex(id)];
    if (d.id === 'mic') {
      st.mic_button = after === 'on' ? 'ptt' : 'off';
      st.mic_button_active = after === 'on' ? null : false;
      st.mic_button_error = null;
    } else {
      st[d.key] = after === 'on';
    }
  }

  function soundConfirm(id, turningOn) {
    if (id === 'mic') {
      return turningOn
        ? 'Turn on the remote mic button? Holding the mic button on the Magic Remote then talks to Home Assistant, and LG\'s voice assistant no longer opens from that button (LG may show "This function is not supported"). The satellite gives the button back to LG when this is turned off, when the satellite stops, and at every TV restart. ' + RESTART
        : 'Give the mic button back to LG? LG\'s voice assistant opens from the mic button again, and the button no longer talks to Home Assistant. ' + RESTART;
    }
    if (id === 'music') {
      return turningOn
        ? 'Turn on music? Home Assistant and Music Assistant can then play music on this TV through the satellite. The satellite fetches the music from the address Home Assistant sends, only over its encrypted connection, never from the TV itself. Experimental: not yet tested on a TV. ' + RESTART
        : 'Turn off music? The satellite then plays only replies, announcements and timer alarms. ' + RESTART;
    }
    return turningOn
      ? 'Turn on echo cancellation? The satellite removes the TV\'s own sound from what it hears, so it can hear you while the TV plays. It uses a little more CPU. ' + RESTART
      : 'Turn off echo cancellation? While the TV plays sound, the satellite may hear you less well. ' + RESTART;
  }

  // The two rows from `ctl.sh status --json`: { word, cls, checked, can, cmd,
  // confirm, warn }. Both directions take two presses (a restart each time).
  function soundRows(st) {
    var out = [];
    for (var i = 0; i < SOUND_ROWS.length; i++) {
      var d = SOUND_ROWS[i];
      var v = st ? soundValue(st, d) : undefined;
      if (!st) {
        out.push({ word: 'Checking...', cls: 'muted', checked: 'false', can: false, cmd: '', confirm: '', warn: '' });
      } else if (typeof v !== 'boolean') {
        out.push({ word: 'Not known', cls: 'warn', checked: 'false', can: false, cmd: '', confirm: '',
          warn: d.label + ' is not reported by this version of the satellite. Apply the update first.' });
      } else if (d.id === 'mic' && micFailed(st)) {
        // Configured but not running: "on" again is a retry (a restart).
        var why = st.mic_button_error ? String(st.mic_button_error) : 'no reason given';
        var rc = soundConfirm('mic', true);
        out.push({ word: 'Off (LG kept it)', cls: 'warn', checked: 'false', can: true,
          cmd: 'config mic-button on', confirm: rc + CONFIRM_TAIL,
          warn: 'The mic button could not be taken from LG\'s assistant, so LG keeps it: ' + why + '. Press OK twice to try again.' });
      } else {
        var c = soundConfirm(d.id, !v);
        out.push({ word: v ? 'On' : 'Off', cls: '', checked: v ? 'true' : 'false', can: true,
          cmd: 'config ' + d.action + (v ? ' off' : ' on'), confirm: c + CONFIRM_TAIL, warn: c });
      }
    }
    return out;
  }

  // The Home tile's second line: "Echo cancellation on, music off".
  function soundSummary(st) {
    if (!st || typeof st.aec_enabled !== 'boolean' || typeof st.music_enabled !== 'boolean') { return 'Echo cancellation, music'; }
    var tail = soundValue(st, SOUND_ROWS[2]) ? ', mic button on' : '';
    return 'Echo cancellation ' + (st.aec_enabled ? 'on' : 'off') + ', music ' + (st.music_enabled ? 'on' : 'off') + tail;
  }

  // One line for the JSON result of `ctl.sh config aec|music|mic-button`, as [text, class].
  function soundResult(id, r) {
    var row = SOUND_ROWS[soundIndex(id)];
    var w = function (v) { return v === 'on' ? 'On' : (v === 'off' ? 'Off' : 'not known'); };
    if (!r || r.setting !== row.action) { return ['Could not confirm the change of ' + row.label + '. The screen shows the state again in a few seconds.', 'bad']; }
    if (r.result === 'applied') {
      var tail = r.daemon === 'restarted' ? ' The voice service restarted.'
        : (r.daemon === 'not running' ? ' The satellite is not running; it uses this when it starts.' : '');
      if (id === 'mic' && r.after === 'on' && r.daemon === 'restarted') {
        // Whether LG gave the button up is known only from the next status.
        tail += ' It takes the button from LG\'s assistant now; this screen shows in a few seconds whether that worked.';
      }
      return [row.label + ' is now ' + w(r.after) + '.' + tail, 'ok'];
    }
    if (r.result === 'already') { return ['Already ' + w(r.after || r.before) + '. Nothing changed.', 'ok']; }
    // Refused by a guard of ctl.sh (the same for Home Assistant's switch):
    // nothing was changed, and it is no failure.
    if (r.result === 'refused' && r.reason === 'cooldown') {
      return [row.label + ' was changed moments ago. Try again in ' + (r.retry_in_s || 30) + ' s. Nothing changed.', 'warn'];
    }
    if (r.result === 'refused' && r.reason === 'busy') { return ['Not now: ' + r.error + '. Nothing changed.', 'warn']; }
    var why = r.error ? r.error : 'no reason given';
    return ['Could not change ' + row.label + ': ' + why + '.' + (r.daemon === 'failed' ? ' The satellite is not running: see Diagnostics.' : ''), 'bad'];
  }

  // ---- phone requests (docs/design/phone-setup.md sections 7 and 9) ---------

  function strip(t) { return t.replace(/^[^?]*\?\s*/, ''); }

  // ask: continues "Phone <address> wants to:"; detail: one more sentence;
  // done: the result line after it ran.
  var REQUEST = {
    'always-ready': {
      on: { ask: 'turn Always Ready on', detail: strip(confirmText('ar', true)), done: 'Always Ready is on' },
      off: { ask: 'turn Always Ready off', detail: strip(confirmText('ar', false)), done: 'Always Ready is off' }
    },
    'hi-lg': {
      on: { ask: 'turn Hi LG on', detail: strip(confirmText('hl', true)), done: 'Hi LG is on' },
      off: { ask: 'turn Hi LG off', detail: strip(confirmText('hl', false)), done: 'Hi LG is off' }
    },
    'remote-off': {
      keep: { ask: 'turn Keep listening on', detail: strip(confirmText('keep', true)), done: 'Keep listening is on' },
      normal: { ask: 'turn Keep listening off', detail: 'The power button then turns the TV off the normal way.', done: 'Keep listening is off' }
    },
    mic: {
      // The phone turns the microphone off without a TV confirmation (the
      // safe direction, owner decision 2026-11-05); this text is for the
      // result line only.
      off: { ask: 'turn the microphone off', detail: 'The satellite stops hearing until the microphone is turned on again.', done: 'The microphone is off' },
      on: { ask: 'turn the microphone on', detail: 'The satellite will listen again.', done: 'The microphone is on' }
    },
    pair: {
      '': { ask: 'open the pairing window for 10 minutes', detail: 'Anyone on your network who connects first can claim this TV while it is open.',
        done: 'The pairing window is open' }
    }
  };
  // Actions the phone may run without the OK on the TV.
  var NO_CONFIRM = { 'mic off': true };

  function validVolume(v) { return typeof v === 'string' && /^(0|[1-9][0-9]?|100)$/.test(v); }

  // The texts for one phone request, or null when the action or the value is
  // not one this app knows (the TV then refuses it without asking).
  function requestText(action, value) {
    if (typeof action !== 'string') { return null; }
    var v = value === null || value === undefined ? '' : value;
    if (typeof v !== 'string') { return null; }
    if (action === 'reply-volume') {
      if (!validVolume(v)) { return null; }
      return { ask: 'set the reply volume to ' + v + ' %',
        detail: v === '0' ? 'Replies and timer alarms will be silent.' : 'How loud the satellite answers. The TV volume does not change.',
        done: 'The reply volume is ' + v + ' %', confirm: true };
    }
    if (!Object.prototype.hasOwnProperty.call(REQUEST, action) || !Object.prototype.hasOwnProperty.call(REQUEST[action], v)) { return null; }
    var e = REQUEST[action][v];
    return { ask: e.ask, detail: e.detail, done: e.done, confirm: !NO_CONFIRM[action + ' ' + v] };
  }

  // An IPv4 literal (the only peer form the setup server reports).
  function isIPv4(s) {
    if (typeof s !== 'string' || !/^\d{1,3}(\.\d{1,3}){3}$/.test(s)) { return false; }
    var p = s.split('.');
    for (var i = 0; i < 4; i++) {
      if (p[i].length > 1 && p[i].charAt(0) === '0') { return false; }
      if (+p[i] > 255) { return false; }
    }
    return true;
  }

  // The result line for a finished phone request (`last_result` of
  // `ctl.sh setup status --json`), as [text, class]. Never uses server text
  // except the one-line `error` reason.
  function phoneResult(lr) {
    if (!lr) { return null; }
    var t = requestText(lr.action, lr.value);
    if (!t) { return ['The phone asked for something this app does not know. Nothing changed.', 'warn']; }
    var why = typeof lr.error === 'string' && lr.error ? lr.error : 'no reason given';
    var ok = lr.result === 'ok' || lr.result === 'applied' || lr.result === 'already';
    if (lr.verdict === 'auto') {
      // Only the microphone off runs without the OK on the TV.
      if (!ok) { return ['The phone could not ' + t.ask + ': ' + why + '.', 'bad']; }
      return [lr.action === 'mic' && lr.value === 'off' ? 'Microphone turned off from the phone.' : 'From the phone: ' + t.done + '.', 'warn'];
    }
    if (lr.verdict === 'refused') { return ['Refused the phone: ' + t.ask + '. Nothing changed.', '']; }
    if (lr.verdict === 'expired') { return ['No answer on the TV in time: ' + t.ask + '. Nothing changed.', 'warn']; }
    if (lr.verdict === 'aborted') { return ['The phone session ended first: ' + t.ask + '. Nothing changed.', 'warn']; }
    if (lr.verdict === 'allowed') {
      if (ok) { return ['From the phone: ' + t.done + '.', 'ok']; }
      if (lr.result === 'mismatch') { return ['From the phone: the TV did not keep the change (' + t.ask + ').', 'warn']; }
      return ['Could not ' + t.ask + ': ' + why + '.', 'bad'];
    }
    return null;
  }

  // Why a phone session ended (`reason` of an ended state), one line.
  var ENDED = {
    done: 'The phone session ended.',
    timeout: 'The phone session ended after 10 minutes.',
    idle: 'The phone session ended: the phone was idle for 3 minutes.',
    'app-gone': 'The phone session ended: this app stopped answering.',
    'parse-error': 'The phone session ended: the TV received a request it did not understand.',
    'too-many-refusals': 'The phone session ended after three refused requests.',
    'too-many-bad-requests': 'The phone session ended after too many bad requests.',
    crashed: 'The phone setup server stopped unexpectedly.'
  };
  function endedText(reason) {
    return Object.prototype.hasOwnProperty.call(ENDED, reason) ? ENDED[reason] : 'The phone session ended.';
  }

  return {
    // matrix and rows (unchanged contract of 0.3.0, tables now with short forms)
    outcome: outcome, rows: rows, hint: hint, HINT: HINT, resultMessage: resultMessage, confirmText: confirmText, wordOf: wordOf,
    rowKind: rowKind, rowIndex: rowIndex, arInfo: arInfo, hlInfo: hlInfo, roInfo: roInfo,
    POWER: POWER, LISTEN: LISTEN, NOTE: NOTE, POWER_EXTRA: POWER_EXTRA, ROWS: ROWS,
    AR_MENU: AR_MENU, HL_MENU: HL_MENU, CONFIRM_TAIL: CONFIRM_TAIL,
    // cards
    haSummary: haSummary, haCard: haCard, SETUP_UNAVAILABLE: SETUP_UNAVAILABLE, HA_CONN: HA_CONN, compat: compat, KEY: KEY, COMPAT: COMPAT, HA_STEPS: HA_STEPS, HA_CLEANUP: HA_CLEANUP, HA_DELETE_PATH: HA_DELETE_PATH,
    // Sound screen (TV app only)
    soundRows: soundRows, soundSummary: soundSummary, soundResult: soundResult, soundConfirm: soundConfirm, soundIndex: soundIndex,
    soundApply: soundApply,
    SOUND_ROWS: SOUND_ROWS, SOUND_NOTE: SOUND_NOTE,
    // phone requests
    requestText: requestText, phoneResult: phoneResult, endedText: endedText, isIPv4: isIPv4,
    REQUEST: REQUEST, ENDED: ENDED
  };
}));
