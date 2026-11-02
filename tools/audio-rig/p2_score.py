#!/usr/bin/env python3
"""Phase 2 scoring helpers, as defined in docs/experiments/P2-corpus.md.

- `normalize()` and `wer_counts()`: the WER text normalization and word-level Levenshtein.
- `wilson()`: 95 % Wilson interval for a proportion; `poisson_rate_ci()`: exact 95 % Poisson
  interval for a count over an exposure (rule of three when the count is 0).
- `wake_window()` / `match_wake()`: the wake detection window per positive item.
- `INTENT_MAP` / `score_intent()`: the class-to-outcome mapping written down before the runs,
  applied to a built-in-agent recognition result (`conversation/agent/homeassistant/debug`).
- `score_items()` / `report()`: per-condition tables from result JSON lines written by
  `p2_runner.py` or `p2_digital.py`.

    p2_score.py results.jsonl [more.jsonl ...] [--by voice] [--json]
"""
import argparse
import collections
import json
import math
import re
import sys
import unicodedata

# ---- text normalization and WER -------------------------------------------------------------

_UNITS = ["cero", "uno", "dos", "tres", "cuatro", "cinco", "seis", "siete", "ocho", "nueve", "diez",
          "once", "doce", "trece", "catorce", "quince", "dieciseis", "diecisiete", "dieciocho",
          "diecinueve", "veinte", "veintiuno", "veintidos", "veintitres", "veinticuatro", "veinticinco",
          "veintiseis", "veintisiete", "veintiocho", "veintinueve"]
_TENS = {30: "treinta", 40: "cuarenta", 50: "cincuenta", 60: "sesenta", 70: "setenta", 80: "ochenta",
         90: "noventa"}


def spell_es(n):
    """Spanish words for 0..99 (accents dropped, as after normalization)."""
    if not 0 <= n <= 99:
        raise ValueError(n)
    if n < 30:
        return _UNITS[n]
    t, u = divmod(n, 10)
    return _TENS[t * 10] if u == 0 else f"{_TENS[t * 10]} y {_UNITS[u]}"


def _strip(text):
    t = unicodedata.normalize("NFD", text)
    t = "".join(c for c in t if not unicodedata.combining(c)).lower()
    return t


def normalize(text, spell_numbers=False):
    """Normalized word list. Returns (words, numerals_converted)."""
    t = _strip(text or "")
    converted = 0
    if spell_numbers:
        def rep(m):
            nonlocal converted
            n = int(m.group(0))
            if n > 99:
                return m.group(0)
            converted += 1
            return " " + spell_es(n) + " "
        t = re.sub(r"\d+", rep, t)
    t = re.sub(r"[^a-z0-9 ]", " ", t)
    return t.split(), converted


def edit_distance(ref, hyp):
    """Word-level Levenshtein distance (S + D + I)."""
    prev = list(range(len(hyp) + 1))
    for i, r in enumerate(ref, 1):
        cur = [i] + [0] * len(hyp)
        for j, h in enumerate(hyp, 1):
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (r != h))
        prev = cur
    return prev[-1]


def _v11(words):
    # "veintiun" (apocope before a noun) and "veintiuno" are the same number
    return ["veintiuno" if w == "veintiun" else w for w in words]


def wer_counts(ref_text, hyp_text, v11=False):
    """(errors, reference_words, numerals_converted) after normalization.

    v11 (not part of the P2-corpus v1 definition, reported separately): the hypothesis' "%"
    becomes "por ciento" before the numerals are spelled, and "veintiun" equals "veintiuno"."""
    ref, _ = normalize(ref_text)
    if v11:
        hyp_text = (hyp_text or "").replace("%", " por ciento ")
    hyp, conv = normalize(hyp_text, spell_numbers=True)
    if v11:
        ref, hyp = _v11(ref), _v11(hyp)
    return edit_distance(ref, hyp), len(ref), conv


# ---- intervals ---------------------------------------------------------------------------------

def wilson(k, n, z=1.959964):
    """95 % Wilson score interval for k successes out of n."""
    if n == 0:
        return (float("nan"), float("nan"))
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return (max(0.0, c - h), min(1.0, c + h))


def _chi2_ppf(q, df):
    from scipy.stats import chi2
    return float(chi2.ppf(q, df))


def poisson_rate_ci(k, exposure):
    """Exact (Garwood) 95 % interval for a Poisson rate k/exposure; for k = 0 the upper bound
    is the rule of three (3 / exposure), as P2-corpus.md asks."""
    if exposure <= 0:
        return (float("nan"), float("nan"))
    if k == 0:
        return (0.0, 3.0 / exposure)
    lo = _chi2_ppf(0.025, 2 * k) / 2
    hi = _chi2_ppf(0.975, 2 * (k + 1)) / 2
    return (lo / exposure, hi / exposure)


# ---- wake windows ------------------------------------------------------------------------------

def wake_window(item, pre=0.2, post=1.5):
    """(start, end) in seconds of the item's own timeline for a positive's wake detection."""
    return (item["wake_start_ms"] / 1000 - pre, item["wake_end_ms"] / 1000 + post)


def match_wake(item, detections_s, pre=0.2, post=1.5):
    """Classify detection times (seconds in the item timeline) for one item.

    Returns dict(detected, first_s, latency_s, repeats, outside). For a positive, `detected`
    is true if a detection falls in the window; detections in the item but outside the window
    are `outside` (false wakes in the strict sense). For a negative or near-miss every
    detection counts as `outside`."""
    det = sorted(detections_s)
    if item.get("kind") != "pos":
        return {"detected": False, "first_s": None, "latency_s": None, "repeats": 0, "outside": len(det)}
    lo, hi = wake_window(item, pre, post)
    inside = [t for t in det if lo <= t <= hi]
    outside = [t for t in det if not lo <= t <= hi]
    if not inside:
        return {"detected": False, "first_s": None, "latency_s": None, "repeats": 0, "outside": len(outside)}
    first = inside[0]
    return {"detected": True, "first_s": first, "latency_s": first - item["wake_end_ms"] / 1000,
            "repeats": len(inside) - 1, "outside": len(outside)}


# ---- intents -----------------------------------------------------------------------------------

# Mapping from the corpus' expected_intent class to the built-in agent outcome that counts,
# written down before the runs (P2-corpus.md asks for this). `slots` are checked on the
# normalized slot text (slots or unmatched_slots of the recognition result); an area slot must
# contain the word given. Entities need not exist: recognition of the intent and its slots is
# scored, the effect is not (no device was actuated in session 2).
INTENT_MAP = {
    "light_on": {"intents": {"HassTurnOn"}, "slots": {"area": "from_text"}},
    "light_off": {"intents": {"HassTurnOff"}, "slots": {"area": "cocina"}},
    "light_off_all": {"intents": {"HassTurnOff"}, "slots": {"domain": "light"}},
    "light_set": {"intents": {"HassLightSet"}, "slots": {"area": "salon", "brightness": "cincuenta"}},
    "time_query": {"intents": {"HassGetCurrentTime"}, "slots": {}},
    "timer_start": {"intents": {"HassStartTimer"}, "slots": {"minutes": "from_text"}},
    "timer_cancel": {"intents": {"HassCancelTimer"}, "slots": {}},
    "timer_status": {"intents": {"HassTimerStatus"}, "slots": {}},
    "temperature_query": {"intents": {"HassClimateGetTemperature", "HassGetWeather"}, "slots": {}},
    "weather_query": {"intents": {"HassGetWeather"}, "slots": {}},
    "climate_set": {"intents": {"HassClimateSetTemperature"}, "slots": {"temperature": "veintiun"}},
}

_AREAS = ("salon", "dormitorio", "cocina")
_MINUTES = ("cinco", "diez")


def _slot_text(v):
    if isinstance(v, dict):
        v = v.get("text", v.get("value"))
    return " ".join(normalize(str(v), spell_numbers=True)[0])


def expected_slots(intent_class, command_text):
    spec = INTENT_MAP[intent_class]["slots"]
    words = normalize(command_text)[0]
    out = {}
    for k, v in spec.items():
        if v != "from_text":
            out[k] = v
        elif k == "area":
            out[k] = next(a for a in _AREAS if a in words)
        elif k == "minutes":
            out[k] = next(m for m in _MINUTES if m in words)
    return out


def score_intent(intent_class, command_text, recog):
    """True if the recognition result (one entry of the debug endpoint's `results`) matches
    the mapping for this class. `recog` None (no intent) is a failure."""
    if not recog or "intent" not in recog:
        return False
    if recog["intent"].get("name") not in INTENT_MAP[intent_class]["intents"]:
        return False
    got = {}
    for src in ("slots", "unmatched_slots"):
        for k, v in (recog.get(src) or {}).items():
            got.setdefault(k, _slot_text(v))
    return all(k in got and v in got[k] for k, v in expected_slots(intent_class, command_text).items())


# ---- tables ------------------------------------------------------------------------------------

def fmt_rate(k, n):
    if n == 0:
        return "0/0"
    lo, hi = wilson(k, n)
    return f"{k}/{n} = {100 * k / n:.0f} % [{100 * lo:.0f}, {100 * hi:.0f}]"


def score_items(rows):
    """rows: result dicts with keys item (manifest row), detections_s (list), stt_text, recog,
    recog_ref. Returns a per-item list with derived fields."""
    out = []
    for r in rows:
        it = r["item"]
        m = match_wake(it, r.get("detections_s", []))
        d = {"id": it["id"], "voice": it["voice"], "speaker": it.get("speaker"), "kind": it["kind"],
             "subkind": it.get("subkind"), "lang": it.get("lang"), "condition": r.get("condition"), **m}
        if it["kind"] == "pos" and it.get("subkind") == "cmd":
            d["expected_intent"] = it["expected_intent"]
            if m["detected"] and r.get("stt_text") is not None:
                e, n, conv = wer_counts(it["expected_command_text"], r["stt_text"])
                d.update(wer_errors=e, wer_words=n, numerals=conv, stt_text=r["stt_text"],
                         wer11_errors=wer_counts(it["expected_command_text"], r["stt_text"], v11=True)[0])
                d["intent_ok"] = score_intent(it["expected_intent"], it["expected_command_text"], r.get("recog"))
                if "recog_ref" in r:
                    d["intent_ref_ok"] = score_intent(it["expected_intent"], it["expected_command_text"],
                                                      r["recog_ref"])
            else:
                d["intent_ok"] = False
        out.append(d)
    return out


def summarize(scored, by=None):
    groups = collections.defaultdict(list)
    for s in scored:
        key = (s["condition"],) + ((s.get(by),) if by else ())
        groups[key].append(s)
    lines = []
    for key in sorted(groups, key=lambda k: tuple(str(x) for x in k)):
        g = groups[key]
        pos = [s for s in g if s["kind"] == "pos"]
        neg = [s for s in g if s["kind"] != "pos"]
        cmd = [s for s in pos if s.get("subkind") == "cmd"]
        det_cmd = [s for s in cmd if s["detected"]]
        werr = sum(s.get("wer_errors", 0) for s in det_cmd if "wer_errors" in s)
        wn = sum(s.get("wer_words", 0) for s in det_cmd if "wer_words" in s)
        lat = sorted(s["latency_s"] for s in pos if s["detected"])
        row = {"group": " / ".join(str(x) for x in key),
               "wake": fmt_rate(sum(s["detected"] for s in pos), len(pos)) if pos else "",
               "false_wakes_on_neg": f"{sum(s['outside'] > 0 for s in neg)}/{len(neg)} items" if neg else "",
               "outside_on_pos": sum(s["outside"] for s in pos),
               "repeats": sum(s["repeats"] for s in pos),
               "latency_median_s": round(lat[len(lat) // 2], 2) if lat else None,
               "latency_p90_s": round(lat[min(len(lat) - 1, int(0.9 * len(lat)))], 2) if lat else None,
               "wer": f"{werr}/{wn} = {100 * werr / wn:.1f} %" if wn else "",
               "wer_v11": (lambda e: f"{e}/{wn} = {100 * e / wn:.1f} %")(
                   sum(s.get("wer11_errors", 0) for s in det_cmd)) if wn else "",
               "wer0_items": f"{sum(s.get('wer_errors', 1) == 0 for s in det_cmd)}/{len(det_cmd)}" if det_cmd else "",
               "intent_given_wake": fmt_rate(sum(s["intent_ok"] for s in det_cmd), len(det_cmd)) if cmd else "",
               "intent_end_to_end": fmt_rate(sum(s["intent_ok"] for s in cmd), len(cmd)) if cmd else "",
               "intent_ref_ceiling": fmt_rate(sum(s.get("intent_ref_ok", False) for s in det_cmd), len(det_cmd))
               if any("intent_ref_ok" in s for s in det_cmd) else ""}
        lines.append(row)
    return lines


def false_wakes(watch_rows, playlog_rows, rearm_s=0.3):
    """False wakes per content category from `ha_assist_watch.py` and `content_play.py` logs.

    A detection is a satellite state change from `idle` to `listening` (PC time). It belongs
    to the clip playing at that moment. `hours` is the content played; `hours_armed` subtracts
    the time the wake word was not listening (from each detection until the entity is `idle`
    again, plus `rearm_s`). Rates use the armed time."""
    states = [r for r in watch_rows if r.get("kind") == "state"]
    dets, dead = [], []
    t_det = None
    for r in states:
        if r["old"] == "idle" and r["new"] == "listening":
            t_det = r["t_pc"]
            dets.append(t_det)
        elif r["new"] == "idle" and t_det is not None:
            dead.append((t_det, r["t_pc"] + rearm_s))
            t_det = None
    cats = collections.OrderedDict()
    for c in playlog_rows:
        g = cats.setdefault(c["category"], {"k": 0, "seconds": 0.0, "dead": 0.0, "clips": 0})
        g["clips"] += 1
        g["seconds"] += c["t_end"] - c["t_start"]
        for a, b in dead:
            g["dead"] += max(0.0, min(b, c["t_end"]) - max(a, c["t_start"]))
        g["k"] += sum(c["t_start"] <= t < c["t_end"] for t in dets)
    unassigned = sum(not any(c["t_start"] <= t < c["t_end"] for c in playlog_rows) for t in dets)
    rows = []
    tot = {"k": 0, "seconds": 0.0, "dead": 0.0, "clips": 0}
    for name, g in list(cats.items()) + [("total", tot)]:
        if name != "total":
            for k in tot:
                tot[k] += g[k]
        h = g["seconds"] / 3600
        ha = (g["seconds"] - g["dead"]) / 3600
        lo, hi = poisson_rate_ci(g["k"], ha)
        rows.append({"category": name, "clips": g["clips"], "false_wakes": g["k"], "hours": round(h, 3),
                     "hours_armed": round(ha, 3), "per_hour": round(g["k"] / ha, 2) if ha else None,
                     "ci95": [round(lo, 2), round(hi, 2)]})
    return rows, {"detections": len(dets), "outside_playlog": unassigned}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("results", nargs="*")
    ap.add_argument("--false-wakes", nargs=2, metavar=("WATCH", "PLAYLOG"),
                    help="false wakes per category from ha_assist_watch.py and content_play.py logs")
    ap.add_argument("--by", choices=["voice", "subkind", "kind", "expected_intent", "lang"])
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    if a.false_wakes:
        load = lambda f: [json.loads(l) for l in open(f) if l.strip()]
        table, info = false_wakes(load(a.false_wakes[0]), load(a.false_wakes[1]))
        for row in table:
            print(json.dumps(row))
        print(json.dumps(info))
        return 0
    rows = [json.loads(l) for f in a.results for l in open(f) if l.strip()]
    rows = [r for r in rows if "item" in r]
    scored = score_items(rows)
    table = summarize(scored, a.by)
    if a.json:
        print(json.dumps({"items": scored, "summary": table}, ensure_ascii=False, indent=1))
        return 0
    for row in table:
        print(json.dumps(row, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
