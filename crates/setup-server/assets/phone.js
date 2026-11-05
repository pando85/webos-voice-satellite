/*
 * Phone setup page (docs/design/phone-setup.md, sections 8 to 11). It talks
 * only to this server's API (GET /api/session, GET /api/state,
 * POST /api/actions, GET /api/actions/<id>, POST /api/session/end) and takes
 * every sentence about the TV from the shared window.voicePower
 * (packaging/webos/app/voicepower.js). No key, password or Home Assistant
 * login is ever read, asked for or stored.
 *
 * The first part is pure (no DOM, no network; node tests load it with
 * module.exports); the second part drives the page and runs only in a browser.
 */
(function (root) {
  'use strict';

  var MY_HA = 'https://my.home-assistant.io/redirect/config_flow_start/?domain=esphome';
  var STATE_MS = 3000;     // the server refreshes its cache every 5 s, only while this page polls (SR-03)
  var ACTION_MS = 1000;    // while a request waits for the TV
  var GIVE_UP_AFTER = 3;   // failed polls in a row before the session counts as gone

  // ---- pure part ---------------------------------------------------------------

  function clock(sec) {
    sec = Math.max(0, Math.floor(sec));
    var s = sec % 60;
    return Math.floor(sec / 60) + ':' + (s < 10 ? '0' : '') + s;
  }

  // 0.5 dB per percent, 100 % = 0 dB (docs/design/phone-setup.md section 9.2).
  function volumeText(p) {
    if (p === 0) { return 'Silent: replies and timer alarms make no sound.'; }
    var db = (100 - p) / 2;
    return p + ' % is ' + (db === 0 ? '0' : '-' + db) + ' dB.';
  }

  function upper(s) { return s.charAt(0).toUpperCase() + s.slice(1); }

  // The Voice and power model input: the allow-listed tv part plus whether
  // the daemon runs (that one comes from the device part).
  function tvModel(st) {
    if (!st || !st.tv) { return st && st.errors && st.errors.tv ? {} : null; }
    var m = {};
    for (var k in st.tv) { if (Object.prototype.hasOwnProperty.call(st.tv, k)) { m[k] = st.tv[k]; } }
    if (st.device && typeof st.device.running === 'boolean') { m.daemon_running = st.device.running; }
    return m;
  }

  // Everything the page shows for one /api/state reply.
  function buildView(vp, st, busy) {
    st = st || {};
    var dev = st.device || null;
    var model = tvModel(st);
    var o = vp.outcome(model || { daemon_running: dev ? dev.running : undefined });
    var known = !!model;
    var ha = vp.haSummary(dev ? dev : { installed: false });
    var rws = vp.rows(model);
    var mic = model && model.mic ? model.mic : null;
    var micKnown = !!(mic && typeof mic.kill_switch === 'boolean');
    var kill = micKnown && mic.kill_switch === true;

    var cards = {
      listen: known ? { value: o.listenShort, detail: o.listenDetail, tone: o.listenTone }
        : { value: 'Checking...', detail: '', tone: 'idle' },
      ha: dev ? { value: ha.short, detail: ha.full, tone: ha.tone } : { value: 'Checking...', detail: '', tone: 'idle' },
      power: known ? { value: o.powerShort, detail: o.detail + (o.extra ? ' ' + o.extra : ''), tone: o.powerTone }
        : { value: 'Checking...', detail: '', tone: 'idle' }
    };

    // Add to Home Assistant: only while the TV is not paired (section 10).
    var key = ha.key;
    var addable = key === 'unpaired' || key === 'cleared' || key === 'pairing';
    var haView = {
      text: ha.full,
      show: addable,
      canPair: !!dev && dev.running === true && (key === 'unpaired' || key === 'cleared') && !busy,
      showPair: key === 'unpaired' || key === 'cleared',
      pairLeftS: ha.pairingLeftS,
      port: dev && typeof dev.port === 'number' ? String(dev.port) : ''
    };

    var rowViews = [];
    for (var i = 0; i < vp.ROWS.length; i++) {
      var d = vp.ROWS[i];
      var r = rws[i];
      var cmd = r.cmd ? r.cmd.split(' ') : [];
      rowViews.push({
        id: d.id, label: d.label, owner: d.owner.toUpperCase(),
        sub: d.sub.replace(/^A satellite setting\. /, ''),
        word: r.word, cls: r.cls, checked: r.checked,
        can: r.can && !busy, action: d.action, value: cmd.length ? cmd[cmd.length - 1] : '',
        note: r.can ? '' : (r.warn || '')
      });
    }

    // Keep listening on, Always Ready off: offer to turn Always Ready on (the
    // same request as the Always Ready switch, so the TV asks as usual).
    var h = model ? vp.hint(model) : null;
    var hint = h && rws[0].can
      ? { text: h.text, label: h.label, action: h.action, value: h.value, can: !busy } : null;

    var vol = dev && typeof dev.reply_volume === 'number' ? dev.reply_volume : null;
    var warn = '';
    var errs = st.errors || {};
    if (errs.device || errs.tv) {
      warn = 'Some TV status could not be read' + (errs.tv || errs.device ? ': ' + (errs.tv || errs.device) + '.' : '.');
    } else if (typeof st.age_s === 'number' && st.age_s > 15) {
      warn = 'The TV status is ' + Math.round(st.age_s) + ' s old.';
    }

    return {
      cards: cards, ha: haView, warn: warn,
      out: { head: o.headline, detail: o.detail + (o.extra ? ' ' + o.extra : ''), listening: o.listening, notes: o.notes },
      rows: rowViews, hint: hint,
      mic: {
        known: micKnown, checked: micKnown && !kill, can: micKnown && !busy,
        sub: !known ? 'Checking...' : (!micKnown ? 'The microphone state could not be read.'
          : (kill ? 'Off on this TV. The satellite cannot hear.' : 'On. Mute in Home Assistant also works.'))
      },
      volume: { value: vol, can: vol !== null && !busy }
    };
  }

  // The request the TV is asking about, as one short phrase for the banner.
  function askText(vp, action, value) {
    var t = vp.requestText(action, value === undefined ? null : value);
    return t ? upper(t.ask) : '';
  }

  // A finished request (GET /api/actions/<id>) as the `last_result` shape
  // that voicePower.phoneResult() reads.
  function lastResult(vp, req, r) {
    var t = vp.requestText(req.action, req.value === undefined ? null : req.value);
    var verdict = r.state === 'done' ? (t && t.confirm === false ? 'auto' : 'allowed') : r.state;
    var res = r.result || {};
    return { id: req.id, action: req.action, value: req.value === undefined ? null : req.value,
      verdict: verdict, result: res.result || '', error: res.error || '' };
  }

  // [text, class] for a finished request. Refusal is worded for the phone.
  function resultLine(vp, req, r) {
    if (r.state === 'refused') {
      var t = vp.requestText(req.action, req.value === undefined ? null : req.value);
      return ['Refused on the TV' + (t ? ': ' + t.ask : '') + '. Nothing changed.', 'warn'];
    }
    var line = vp.phoneResult(lastResult(vp, req, r));
    return line || ['The TV did not report a result. Check the status above.', 'warn'];
  }

  // The message for a refused or failed API call: a fixed sentence per code,
  // the server's one-line message for "not applicable".
  function errorLine(status, body) {
    var code = body && typeof body.error === 'string' ? body.error : '';
    var msg = body && typeof body.message === 'string' ? body.message : '';
    if (status === 503 || code === 'app_gone') { return 'The TV app is not answering. Open it on the TV.'; }
    if (status === 429) { return 'Wait a moment before the next request.'; }
    if (status === 409) { return 'Another request is waiting for the TV.'; }
    if (status === 422 && msg) { return msg; }
    return msg || 'The request failed (' + status + ').';
  }

  // The session is over for these answers (no cookie, wrong cookie or address,
  // gone, session ended).
  function sessionGone(status) { return status === 401 || status === 403 || status === 410; }

  var pure = {
    clock: clock, volumeText: volumeText, tvModel: tvModel, buildView: buildView, askText: askText,
    lastResult: lastResult, resultLine: resultLine, errorLine: errorLine, sessionGone: sessionGone,
    MY_HA: MY_HA, STATE_MS: STATE_MS, ACTION_MS: ACTION_MS, GIVE_UP_AFTER: GIVE_UP_AFTER
  };
  if (typeof module === 'object' && module.exports) { module.exports = pure; }
  if (typeof document === 'undefined' || !root || !root.voicePower) { return; }

  // ---- page --------------------------------------------------------------------

  var vp = root.voicePower;
  var csrf = '';
  var state = null;
  var stateAt = 0;
  var expiresAt = 0;
  var tracking = null;     // { id, action, value, state, left (ms deadline) }
  var posting = false;
  var ended = false;
  var failures = 0;
  var stateTimer = 0;
  var followTimer = 0;
  var dragging = false;
  var view = null;

  function $(id) { return document.getElementById(id); }
  function show(el, on) { el.hidden = !on; }
  function text(el, t) { el.textContent = t; }

  function api(method, path, body) {
    var opts = { method: method, credentials: 'same-origin', headers: {}, cache: 'no-store' };
    if (body !== undefined) {
      opts.headers['Content-Type'] = 'application/json';
      opts.headers['X-Setup-CSRF'] = csrf;
      opts.body = JSON.stringify(body);
    }
    var ctl = typeof AbortController === 'function' ? new AbortController() : null;
    var timer = ctl ? setTimeout(function () { ctl.abort(); }, 10000) : 0;
    if (ctl) { opts.signal = ctl.signal; }
    return fetch(path, opts).then(function (r) {
      return r.json().then(function (d) { return d; }, function () { return null; })
        .then(function (d) { return { status: r.status, data: d }; });
    }, function () { return { status: 0, data: null }; }).then(function (x) { clearTimeout(timer); return x; });
  }

  function busy() { return posting || !!tracking || !!(state && state.pending); }

  function endSession(reason, fromServer) {
    if (ended) { return; }
    ended = true;
    clearTimeout(stateTimer);
    clearTimeout(followTimer);
    ['p-wait', 'p-main', 'p-foot', 'p-loading', 'p-warn'].forEach(function (id) { show($(id), false); });
    text($('end-text'), vp.endedText(reason));
    show($('end-cookie'), !!fromServer);
    show($('p-ended'), true);
  }

  // ---- rendering ---------------------------------------------------------------

  function setCard(id, c) {
    var el = $(id);
    el.className = 'p-card tone-' + c.tone;
    text(el.querySelector('.v'), c.value);
    text(el.querySelector('.d'), c.detail);
  }

  var rowEls = {};
  function buildRows() {
    var host = $('p-rows');
    vp.ROWS.forEach(function (d) {
      var line = document.createElement('div');
      line.className = 'p-line';
      var grow = document.createElement('div');
      grow.className = 'grow';
      var lbl = document.createElement('div');
      lbl.className = 'p-lbl';
      lbl.id = 'l-' + d.id;
      var name = document.createElement('span');
      name.textContent = d.label;
      var tag = document.createElement('span');
      tag.className = 'p-tag';
      tag.textContent = d.owner.toUpperCase();
      lbl.appendChild(name);
      lbl.appendChild(tag);
      var sub = document.createElement('div');
      sub.className = 'p-rsub';
      grow.appendChild(lbl);
      grow.appendChild(sub);
      var word = document.createElement('span');
      word.className = 'p-word';
      var sw = document.createElement('button');
      sw.type = 'button';
      sw.className = 'p-sw';
      sw.setAttribute('role', 'switch');
      sw.setAttribute('aria-labelledby', 'l-' + d.id);
      sw.setAttribute('aria-checked', 'false');
      sw.disabled = true;
      line.appendChild(grow);
      line.appendChild(word);
      line.appendChild(sw);
      var note = document.createElement('div');
      note.className = 'p-note';
      note.hidden = true;
      host.appendChild(line);
      host.appendChild(note);
      rowEls[d.id] = { sub: sub, word: word, sw: sw, note: note };
      if (d.id === 'keep') {
        var hint = document.createElement('div');
        hint.className = 'p-note p-hint';
        hint.hidden = true;
        var hintText = document.createElement('span');
        var hintBtn = document.createElement('button');
        hintBtn.type = 'button';
        hintBtn.className = 'p-btn small';
        hintBtn.addEventListener('click', function () {
          if (view && view.hint && view.hint.can) { request(view.hint.action, view.hint.value); }
        });
        hint.appendChild(hintText);
        hint.appendChild(hintBtn);
        host.appendChild(hint);
        rowEls.hint = { box: hint, text: hintText, btn: hintBtn };
      }
      sw.addEventListener('click', function () {
        var r = view && view.rows.filter(function (x) { return x.id === d.id; })[0];
        if (r && r.can && r.value) { request(r.action, r.value); }
      });
    });
  }

  function render() {
    if (ended || !state) { return; }
    view = buildView(vp, state, busy());
    setCard('c-listen', view.cards.listen);
    setCard('c-ha', view.cards.ha);
    setCard('c-power', view.cards.power);

    var h = view.ha;
    show($('ha-add'), h.show);
    text($('ha-state'), h.text);
    var addr = $('ha-addr');
    var port = $('ha-port');
    addr.value = root.location.hostname;
    port.value = h.port || 'Not known';
    $('copy-port').disabled = !h.port;
    show($('pair-btn'), h.showPair);
    $('pair-btn').disabled = !h.canPair;
    $('ha-steps').textContent = vp.HA_STEPS;
    show($('p-warn'), !!view.warn);
    text($('p-warn'), view.warn);

    text($('o-head'), view.out.head);
    text($('o-detail'), view.out.detail);
    text($('o-listen'), view.out.listening);
    var notes = $('o-notes');
    notes.textContent = '';
    view.out.notes.forEach(function (n) {
      var s = document.createElement('span');
      s.className = 'p-word warn';
      s.textContent = n;
      notes.appendChild(s);
    });
    view.rows.forEach(function (r) {
      var e = rowEls[r.id];
      text(e.sub, r.sub);
      var changing = tracking && tracking.action === r.action;
      text(e.word, changing ? 'Waiting...' : r.word);
      e.word.className = 'p-word' + (r.cls && !changing ? ' ' + r.cls : '');
      e.sw.setAttribute('aria-checked', r.checked);
      e.sw.disabled = !r.can;
      show(e.note, !!r.note);
      text(e.note, r.note);
    });

    var he = rowEls.hint;
    show(he.box, !!view.hint);
    text(he.text, view.hint ? view.hint.text + ' ' : '');
    text(he.btn, view.hint ? view.hint.label : '');
    he.btn.disabled = !view.hint || !view.hint.can;

    text($('mic-sub'), view.mic.sub);
    $('mic-sw').setAttribute('aria-checked', view.mic.checked ? 'true' : 'false');
    $('mic-sw').disabled = !view.mic.can;

    var vol = $('vol');
    var v = view.volume.value;
    vol.disabled = !view.volume.can;
    if (v !== null && !dragging) {
      vol.value = String(v);
      text($('vol-out'), v + ' %');
      text($('vol-sub'), vp.requestText('reply-volume', String(v)).detail + ' ' + volumeText(v));
    }
    renderBanner();
  }

  function renderBanner() {
    var w = $('p-wait');
    var t = tracking;
    if (!t) { show(w, false); return; }
    show(w, true);
    if (t.state === 'running') {
      text($('p-wait-main'), 'Applying the change...');
      text($('p-wait-sub'), askText(vp, t.action, t.value));
      return;
    }
    var left = Math.max(0, Math.ceil((t.deadline - Date.now()) / 1000));
    text($('p-wait-main'), 'Waiting for OK on the TV');
    text($('p-wait-sub'), (askText(vp, t.action, t.value) || 'A request') + '. Press OK on the TV remote to allow it (' + left + ' s left). Back on the remote refuses.');
  }

  function showResult(line) {
    var el = $('p-result');
    if (!line) { show(el, false); return; }
    el.className = 'p-result' + (line[1] ? ' ' + line[1] : '');
    text(el, line[0]);
    show(el, true);
  }

  function tick() {
    if (ended) { return; }
    var left = (expiresAt - Date.now()) / 1000;
    text($('clock'), clock(left));
    if (left <= 0) { endSession('timeout', false); return; }
    if (tracking && tracking.state !== 'running') { renderBanner(); }
    // The pairing window's remaining time, counted down between polls.
    if (view && view.ha.pairLeftS !== null && state) {
      var rest = view.ha.pairLeftS - (Date.now() - stateAt) / 1000;
      text($('ha-state'), rest > 0 ? 'Pairing is open, ' + clock(rest) + ' left. ' + vp.KEY.pairing.full : vp.KEY.pairing.full);
    }
  }

  // ---- polling -----------------------------------------------------------------

  function noteFailure(r) {
    if (sessionGone(r.status)) { endSession('', true); return true; }
    if (r.status === 0) {
      failures += 1;
      if (failures >= GIVE_UP_AFTER) { endSession('', false); return true; }
    }
    return false;
  }

  function pollState() {
    clearTimeout(stateTimer);
    if (ended) { return Promise.resolve(); }
    return api('GET', '/api/state').then(function (r) {
      if (ended) { return; }
      if (r.status === 200 && r.data) {
        failures = 0;
        state = r.data;
        stateAt = Date.now();
        $('p-loading').hidden = true;
        show($('p-main'), true);
        show($('p-foot'), true);
        if (!tracking && !posting && state.pending && state.pending.id) {
          // A reload, or a request this page did not start: follow it.
          track({ id: state.pending.id, action: state.pending.action, value: state.pending.value,
            state: state.pending.state, timeout: state.pending.timeout_in_s });
        }
        render();
      } else if (noteFailure(r)) { return; }
      if (!document.hidden) { stateTimer = setTimeout(pollState, STATE_MS); }
    });
  }

  function track(t) {
    t.deadline = Date.now() + (typeof t.timeout === 'number' ? t.timeout * 1000 : 60000);
    tracking = t;
    renderBanner();
    clearTimeout(followTimer);
    followTimer = setTimeout(follow, t.state === 'running' ? 400 : ACTION_MS);
  }

  function follow() {
    var t = tracking;
    if (!t || ended) { return; }
    api('GET', '/api/actions/' + t.id).then(function (r) {
      if (ended || tracking !== t) { return; }
      if (r.status === 200 && r.data) {
        failures = 0;
        var s = r.data.state;
        if (s === 'pending' || s === 'running') {
          t.state = s;
          if (typeof r.data.timeout_in_s === 'number') { t.deadline = Date.now() + r.data.timeout_in_s * 1000; }
          render();
          followTimer = setTimeout(follow, ACTION_MS);
          return;
        }
        tracking = null;
        showResult(resultLine(vp, t, r.data));
        afterRequest();
        return;
      }
      if (r.status === 404) {
        // Kept for 60 s only; the page was away for longer.
        tracking = null;
        showResult(['The TV did not report a result. Check the status above.', 'warn']);
        afterRequest();
        return;
      }
      if (noteFailure(r)) { return; }
      followTimer = setTimeout(follow, ACTION_MS);
    });
  }

  function afterRequest() {
    render();
    pollState();
    setTimeout(pollState, 1500);   // the server refreshes its cache right after a change
  }

  // ---- requests ----------------------------------------------------------------

  function request(action, value) {
    if (busy() || ended) { return; }
    posting = true;
    showResult(null);
    render();
    var body = { action: action };
    if (value !== null && value !== undefined) { body.value = value; }
    api('POST', '/api/actions', body).then(function (r) {
      posting = false;
      if (r.status === 202 && r.data && r.data.id) {
        track({ id: r.data.id, action: action, value: value === undefined ? null : value,
          state: r.data.state, timeout: r.data.timeout_s });
        render();
        return;
      }
      if (sessionGone(r.status) && !(r.data && r.data.error === 'csrf')) { endSession('', true); return; }
      showResult([errorLine(r.status, r.data), r.status === 0 ? 'bad' : 'warn']);
      if (r.data && r.data.error === 'csrf') { loadSession(); }
      render();
      pollState();
    });
  }

  function loadSession() {
    return api('GET', '/api/session').then(function (r) {
      if (r.status === 200 && r.data && typeof r.data.csrf === 'string') {
        csrf = r.data.csrf;
        expiresAt = Date.now() + (r.data.expires_in_s | 0) * 1000;
        return true;
      }
      if (sessionGone(r.status)) { endSession('', true); } else { noteFailure(r); }
      return false;
    });
  }

  // ---- copy buttons (plain HTTP is not a secure context: execCommand first) ------

  function copyField(input, btn) {
    var ok = false;
    input.focus();
    input.select();
    try { input.setSelectionRange(0, input.value.length); } catch (e) { /* not selectable */ }
    try { ok = document.execCommand('copy'); } catch (e2) { ok = false; }
    var done = function (copied) {
      var old = btn.dataset.label || btn.textContent;
      btn.dataset.label = old;
      text(btn, copied ? 'Copied' : 'Selected');
      show($('ha-copy-hint'), !copied);
      setTimeout(function () { text(btn, old); }, 2000);
    };
    if (ok) { done(true); return; }
    if (root.isSecureContext && root.navigator && root.navigator.clipboard) {
      root.navigator.clipboard.writeText(input.value).then(function () { done(true); }, function () { done(false); });
      return;
    }
    done(false);
  }

  // ---- start -------------------------------------------------------------------

  function start() {
    buildRows();
    $('ha-link').href = MY_HA;
    $('copy-addr').addEventListener('click', function () { copyField($('ha-addr'), $('copy-addr')); });
    $('copy-port').addEventListener('click', function () { copyField($('ha-port'), $('copy-port')); });
    $('pair-btn').addEventListener('click', function () { request('pair', null); });
    $('mic-sw').addEventListener('click', function () {
      if (view && view.mic.can) { request('mic', view.mic.checked ? 'off' : 'on'); }
    });
    var vol = $('vol');
    vol.addEventListener('input', function () {
      dragging = true;
      var p = +vol.value;
      text($('vol-out'), p + ' %');
      text($('vol-sub'), volumeText(p));
    });
    vol.addEventListener('change', function () {
      dragging = false;
      var p = +vol.value;
      if (view && view.volume.value === p) { return; }
      request('reply-volume', String(p));
    });
    $('end-btn').addEventListener('click', function () {
      $('end-btn').disabled = true;
      api('POST', '/api/session/end', {}).then(function () { endSession('done', false); });
    });
    document.addEventListener('visibilitychange', function () {
      // A hidden page stops polling, so the server's idle timeout can apply.
      if (!document.hidden && !ended) { loadSession().then(pollState); }
    });
    setInterval(tick, 1000);
    loadSession().then(function (ok) { if (ok) { tick(); pollState(); } });
  }

  start();
}(typeof window !== 'undefined' ? window : null));
