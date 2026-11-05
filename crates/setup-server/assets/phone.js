/* Placeholder client (setup server step P4) so the API can be exercised by
   hand; the real phone page is step P6 of docs/design/phone-setup.md. */
(function () {
  "use strict";
  var csrf = "";
  var ACTIONS = [
    ["mic", "off"], ["mic", "on"], ["hi-lg", "on"], ["hi-lg", "off"],
    ["always-ready", "on"], ["always-ready", "off"],
    ["remote-off", "keep"], ["remote-off", "normal"],
    ["reply-volume", "40"], ["pair", null]
  ];
  function $(id) { return document.getElementById(id); }
  function show(text) { $("result").textContent = text; }

  async function api(method, path, body) {
    var opts = { method: method, credentials: "same-origin", headers: {} };
    if (body !== undefined) {
      opts.headers["Content-Type"] = "application/json";
      opts.headers["X-Setup-CSRF"] = csrf;
      opts.body = JSON.stringify(body);
    }
    var r = await fetch(path, opts);
    var data = null;
    try { data = await r.json(); } catch (e) { data = null; }
    return { status: r.status, data: data };
  }

  async function follow(id) {
    for (;;) {
      var r = await api("GET", "/api/actions/" + id);
      if (r.status !== 200 || !r.data) { show("Request " + id + ": unknown"); return; }
      var st = r.data.state;
      if (st === "pending" || st === "running") {
        show(st === "pending" ? "Press OK on the TV remote to allow (" + r.data.timeout_in_s + " s)" : "Applying...");
        await new Promise(function (res) { setTimeout(res, 1000); });
        continue;
      }
      show("Request " + id + ": " + st + (r.data.result ? " (" + r.data.result.result + ")" : ""));
      return;
    }
  }

  async function act(action, value) {
    var body = { action: action };
    if (value !== null) { body.value = value; }
    var r = await api("POST", "/api/actions", body);
    if (r.status !== 202) {
      show((r.data && r.data.message) || ("Error " + r.status));
      return;
    }
    follow(r.data.id);
  }

  async function refresh() {
    var r = await api("GET", "/api/state");
    if (r.status !== 200) {
      $("line").textContent = "The session has ended. Scan the QR code on the TV again.";
      return false;
    }
    $("line").textContent = "Connected to the TV.";
    $("state").textContent = JSON.stringify(r.data, null, 2);
    return true;
  }

  async function start() {
    var s = await api("GET", "/api/session");
    if (s.status !== 200 || !s.data) {
      $("line").textContent = "No session. Scan the QR code on the TV again.";
      return;
    }
    csrf = s.data.csrf;
    ACTIONS.forEach(function (a) {
      var b = document.createElement("button");
      b.type = "button";
      b.textContent = a[0] + (a[1] === null ? "" : " " + a[1]);
      b.addEventListener("click", function () { act(a[0], a[1]); });
      $("actions").appendChild(b);
    });
    $("done").addEventListener("click", async function () {
      await api("POST", "/api/session/end", {});
      $("line").textContent = "Done. You can close this page.";
    });
    var timer = setInterval(async function () {
      if (!(await refresh())) { clearInterval(timer); }
    }, 3000);
    refresh();
  }

  start();
}());
