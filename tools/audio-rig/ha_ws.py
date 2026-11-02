#!/usr/bin/env python3
"""Minimal Home Assistant WebSocket client for the Phase 2 runs (rig venv + aiohttp).

The token file is either a bare access token or the JSON that HA's `/auth/token`
returns (`access_token`, `refresh_token`, `expires_in`). With a refresh token the
access token is renewed before it expires and the file is rewritten with mode 0600.
Tokens are never printed or logged.

Used by `ha_assist_watch.py` and `p2_runner.py`; also usable on its own:

    ha_ws.py --url https://ha.example --token-file tok.json '{"type": "assist_pipeline/pipeline/list"}'
"""
import argparse
import asyncio
import json
import os
import time

import aiohttp


class TokenFile:
    def __init__(self, path, base_url):
        self.path = path
        self.base_url = base_url.rstrip("/")

    def _read(self):
        raw = open(self.path).read().strip()
        if raw.startswith("{"):
            return json.loads(raw)
        return {"access_token": raw}

    async def access_token(self, session):
        d = self._read()
        if "refresh_token" not in d:
            return d["access_token"]
        if d.get("_obtained", 0) + d.get("expires_in", 1800) - 300 > time.time():
            return d["access_token"]
        async with session.post(self.base_url + "/auth/token", data={
                "grant_type": "refresh_token", "refresh_token": d["refresh_token"],
                "client_id": d.get("client_id", self.base_url + "/")}) as r:
            r.raise_for_status()
            n = await r.json()
        d.update(access_token=n["access_token"], expires_in=n["expires_in"], _obtained=time.time())
        tmp = self.path + ".tmp"
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(d, f)
        os.replace(tmp, self.path)
        return d["access_token"]


class HaWs:
    """One authenticated connection. `call()` sends a command and returns its result;
    `subscribe()` registers a callback for an event subscription."""

    def __init__(self, url, token_file):
        self.url = url.rstrip("/")
        self.tokens = TokenFile(token_file, self.url)
        self._id = 0
        self._pending = {}
        self._subs = {}
        self.session = None
        self.ws = None
        self._reader = None

    async def __aenter__(self):
        self.session = aiohttp.ClientSession()
        tok = await self.tokens.access_token(self.session)
        ws_url = self.url.replace("https://", "wss://").replace("http://", "ws://") + "/api/websocket"
        self.ws = await self.session.ws_connect(ws_url, max_msg_size=0, heartbeat=30)
        await self.ws.receive_json()
        await self.ws.send_json({"type": "auth", "access_token": tok})
        r = await self.ws.receive_json()
        if r.get("type") != "auth_ok":
            raise RuntimeError("HA authentication failed")
        self._reader = asyncio.create_task(self._read())
        return self

    async def __aexit__(self, *exc):
        if self._reader:
            self._reader.cancel()
        if self.ws:
            await self.ws.close()
        await self.session.close()

    async def _read(self):
        async for msg in self.ws:
            if msg.type != aiohttp.WSMsgType.TEXT:
                continue
            d = json.loads(msg.data)
            for m in d if isinstance(d, list) else [d]:
                i = m.get("id")
                if m.get("type") == "event" and i in self._subs:
                    self._subs[i](m["event"])
                elif i in self._pending:
                    self._pending.pop(i).set_result(m)
        for f in self._pending.values():
            if not f.done():
                f.set_exception(ConnectionError("HA WebSocket closed"))

    async def call(self, msg, timeout=30):
        self._id += 1
        i = self._id
        fut = asyncio.get_running_loop().create_future()
        self._pending[i] = fut
        await self.ws.send_json(dict(msg, id=i))
        r = await asyncio.wait_for(fut, timeout)
        if not r.get("success", False):
            raise RuntimeError(f"HA error for {msg.get('type')}: {r.get('error')}")
        return r.get("result")

    async def subscribe(self, msg, callback):
        self._id += 1
        i = self._id
        self._subs[i] = callback
        fut = asyncio.get_running_loop().create_future()
        self._pending[i] = fut
        await self.ws.send_json(dict(msg, id=i))
        r = await asyncio.wait_for(fut, 30)
        if not r.get("success", False):
            raise RuntimeError(f"HA subscribe error: {r.get('error')}")
        return i

    # -- helpers used by the P2 tools --
    async def pipeline_id(self, name_or_id):
        res = await self.call({"type": "assist_pipeline/pipeline/list"})
        for p in res["pipelines"]:
            if name_or_id in (p["id"], p["name"]):
                return p["id"]
        raise KeyError(f"pipeline not found: {name_or_id}")

    async def pipeline_runs(self, pipeline_id):
        res = await self.call({"type": "assist_pipeline/pipeline_debug/list", "pipeline_id": pipeline_id})
        return res.get("pipeline_runs", [])

    async def pipeline_run(self, pipeline_id, run_id):
        return await self.call({"type": "assist_pipeline/pipeline_debug/get",
                                "pipeline_id": pipeline_id, "pipeline_run_id": run_id})

    async def recognize(self, sentences, language, device_id=None):
        """Built-in agent recognition only (no intent is executed)."""
        msg = {"type": "conversation/agent/homeassistant/debug", "sentences": sentences, "language": language}
        if device_id:
            msg["device_id"] = device_id
        return (await self.call(msg))["results"]


def summarize_run(run):
    """Condense a pipeline_debug/get result into the fields the P2 scoring needs."""
    out = {"wake": None, "stt_text": None, "intent": None, "errors": [], "events": []}
    for ev in run.get("events", []):
        t, data = ev.get("type"), ev.get("data") or {}
        out["events"].append([t, ev.get("timestamp")])
        if t == "wake_word-end":
            wo = data.get("wake_word_output") or {}
            out["wake"] = {"id": wo.get("wake_word_id"), "audio_ms": wo.get("timestamp"),
                           "at": ev.get("timestamp")} if wo else None
        elif t == "stt-end":
            out["stt_text"] = (data.get("stt_output") or {}).get("text")
        elif t == "intent-end":
            io = data.get("intent_output") or {}
            resp = io.get("response") or {}
            out["intent"] = {"response_type": resp.get("response_type"),
                             "code": (resp.get("data") or {}).get("code")}
        elif t == "error":
            out["errors"].append(data.get("code"))
    return out


async def _main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--url", required=True)
    ap.add_argument("--token-file", required=True)
    ap.add_argument("msg", nargs="+", help="JSON command(s) without id")
    a = ap.parse_args()
    async with HaWs(a.url, a.token_file) as ha:
        for m in a.msg:
            print(json.dumps(await ha.call(json.loads(m)), ensure_ascii=False, indent=1))


if __name__ == "__main__":
    asyncio.run(_main())
