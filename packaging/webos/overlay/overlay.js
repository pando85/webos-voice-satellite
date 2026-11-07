/*
 * The assistant overlay page (docs/design/assistant-overlay.md, ADR 0014).
 * ES5 only. The satellite daemon starts this app with
 * luna://com.webos.applicationManager/launch, one frame per change in the
 * launch parameters (model.js says what a frame is). The first frame
 * arrives with the page (webOSSystem.launchParams), every later one as a
 * webOSRelaunch event while the page is open.
 *
 * Rules (design section 2):
 *   - never take the remote: the input region is set empty at load, and a
 *     key that still reaches this window closes it at once (so the next
 *     press goes to the app in front);
 *   - never block the picture: one small panel, transparent elsewhere;
 *   - no sound, no network, no storage, no log of the texts;
 *   - close the window when a run is over (an overlay window left open but
 *     hidden is reported to keep the remote on some webOS versions).
 */
(function () {
  'use strict';

  var M = window.overlayModel;
  var FADE_MS = 320;
  var BASE_W = 1920;
  // The listening bars and thinking dots move in steps this often (no CSS
  // animation: 60 fps cost about a third of a core on c3-a, EXP-024).
  var STEP_MS = 200;
  // Bar heights, 0.25 to 1, a fixed pattern that looks like a voice level
  // (it is not one: no audio level reaches this page).
  var LEVELS = [0.35, 0.7, 1, 0.55, 0.3, 0.8, 0.45, 0.9, 0.6, 0.25, 0.75, 0.5];

  var lang = M.langOf(navigator.language);
  var cur = null;        // the frame shown
  var holdTimer = null;
  var closeTimer = null;
  var closing = false;
  var stepTimer = null;
  var step = 0;
  var stepTone = '';
  var probe = typeof window.overlayProbe === 'function' ? window.overlayProbe : function () {};

  function $(id) { return document.getElementById(id); }
  function sys() { return window.webOSSystem || window.PalmSystem || null; }

  function setText(id, s) {
    var e = $(id);
    e.textContent = s;
    e.hidden = !s;
  }

  // webOS window calls; each tried with the array and then the JSON text
  // (one web runtime wants a string, CleanYourTV issue 4).
  function sysCall(name, value) {
    var s = sys();
    if (!s || typeof s[name] !== 'function') { return 'missing'; }
    try { s[name](value); return 'array'; } catch (e1) {
      try { s[name](JSON.stringify(value)); return 'string'; } catch (e2) { return 'error'; }
    }
  }

  function scale() {
    var w = window.innerWidth || BASE_W;
    var k = w / BASE_W;
    $('stage').style.transform = k === 1 ? '' : 'scale(' + k + ')';
  }

  // One step of the bars (listening) or the dots (thinking).
  function stepOnce() {
    var i;
    step++;
    if (stepTone === 'listening') {
      for (i = 0; i < 5; i++) {
        $('b' + i).style.transform = 'scaleY(' + LEVELS[(step * 3 + i * 5) % LEVELS.length] + ')';
      }
    } else {
      for (i = 0; i < 3; i++) { $('d' + i).className = step % 3 === i ? 'on' : ''; }
    }
  }

  function animate(tone) {
    var moving = !closing && (tone === 'listening' || tone === 'thinking');
    if (moving && stepTone === tone && stepTimer) { return; }
    clearInterval(stepTimer);
    stepTimer = null;
    stepTone = moving ? tone : '';
    if (moving) {
      stepOnce();
      stepTimer = setInterval(stepOnce, STEP_MS);
    }
  }

  function close() {
    if (closing) { return; }
    closing = true;
    animate('');
    clearTimeout(holdTimer);
    var p = $('panel');
    p.className = p.className.replace(/\bin\b/, '') + ' out';
    probe('close', null);
    closeTimer = setTimeout(function () { window.close(); }, FADE_MS);
  }

  function arm(ms) {
    clearTimeout(holdTimer);
    holdTimer = ms > 0 ? setTimeout(close, ms) : null;
  }

  function draw(f) {
    var v = M.view(f, lang);
    if (!v.visible) { close(); return; }
    // A frame during the fade-out keeps the window.
    clearTimeout(closeTimer);
    closing = false;
    var p = $('panel');
    setText('heard', v.heard ? '«' + v.heard + '»' : '');
    setText('status', v.status);
    setText('answer', v.answer);
    var wasHidden = p.hidden;
    p.hidden = false;
    p.className = 'panel t-' + v.tone + (wasHidden ? '' : ' in');
    if (wasHidden) {
      // Next frame: let the start position render, then slide in.
      setTimeout(function () { if (!closing) { p.className = 'panel t-' + v.tone + ' in'; } }, 16);
    }
    animate(v.tone);
    arm(v.holdMs);
  }

  function apply(raw, how) {
    var f = M.parse(raw);
    if (!f) { probe('ignored', how); return; }
    if (!M.newer(f, cur)) { probe('stale', how); return; }
    if (M.same(f, cur)) { probe('same', how); return; }
    cur = f;
    probe('frame', { how: how, seq: f.seq, stage: f.stage });
    draw(f);
  }

  function launchParams() {
    var s = sys();
    return s ? s.launchParams : null;
  }

  // ---- start --------------------------------------------------------------
  var region = sysCall('setInputRegion', [{ x: 0, y: 0, width: 0, height: 0 }]);
  var mask = sysCall('setKeyMask', []);
  probe('load', { region: region, mask: mask });
  scale();
  window.addEventListener('resize', scale);

  // A key that reaches this window means it holds the remote: get out of
  // the way at once.
  document.addEventListener('keydown', function (e) {
    probe('key', e.keyCode);
    if (e.preventDefault) { e.preventDefault(); }
    close();
  }, true);

  document.addEventListener('webOSRelaunch', function (e) {
    var d = e && e.detail;
    apply(d && typeof d === 'object' && d.overlay !== undefined ? d : launchParams(), 'relaunch');
  });

  apply(launchParams(), 'launch');
  if (!cur) { close(); }
  // index.html's fallback closes the window if this never runs.
  window.overlayReady = true;
  if (window.overlayFallback) { clearTimeout(window.overlayFallback); }
}());
