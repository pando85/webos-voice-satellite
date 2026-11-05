#!/usr/bin/env python3
"""Transcribe the semi-synthetic STT corpus (stt_corpus.py) with faster-whisper
and score WER and built-in intent success (EXP-019).

transcribe: one CSV per configuration under devices/private/agent_eval/stt-runs/
  python stt_eval.py transcribe --name turbo_b5 --model large-v3-turbo --compute-type int8 --beam-size 5
  python stt_eval.py transcribe --name turbo_b5_prompt ... --initial-prompt auto
score: WER (P2 normalization v1.1, tools/audio-rig/p2_score.py) and the share of
  control phrases that Home Assistant's built-in Spanish intents would still
  resolve to the right call (builtin_intents.py, hassil, fake inventory)
  python stt_eval.py score turbo_b5 turbo_b5_prompt

The transcribe call mirrors wyoming-faster-whisper 2.4.0 (language fixed, beam
size from the CLI, optional initial prompt, no VAD filter); see EXP-019.
Needs faster-whisper (CUDA or CPU); score needs hassil==3.12.1,
home-assistant-intents==2026.10.6 and pyyaml.
"""
import argparse
import csv
import math
import os
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "tools" / "audio-rig"))
CORPUS = ROOT / "devices" / "private" / "agent_eval" / "stt-v1"
RUNS = ROOT / "devices" / "private" / "agent_eval" / "stt-runs"


def auto_prompt():
    """Initial prompt built only from what HA knows: area and entity names/aliases
    of the exposed (here: fake) inventory, plus the assistant's name."""
    import ha_fake
    home = ha_fake.FakeHome()
    words = []
    for a in home.areas.values():
        words += [a["name"]] + a.get("aliases", [])
    for e in home.entities.values():
        words += [e["name"]] + e.get("aliases", [])
    seen, out = set(), []
    for w in words:
        if w.lower() not in seen:
            seen.add(w.lower())
            out.append(w)
    return "Kivi, asistente de casa. " + ", ".join(out) + "."


def cmd_transcribe(a):
    from faster_whisper import WhisperModel
    import soundfile as sf
    prompt = auto_prompt() if a.initial_prompt == "auto" else (a.initial_prompt or None)
    hot = auto_prompt() if a.hotwords == "auto" else (a.hotwords or None)
    kw = {}
    if a.cpu_threads:
        kw["cpu_threads"] = a.cpu_threads
    model = WhisperModel(a.model, device=a.device, compute_type=a.compute_type, **kw)
    rows = list(csv.DictReader(open(CORPUS / "manifest.csv")))
    if a.conds:
        rows = [r for r in rows if r["cond"] in a.conds]
    if a.limit:
        rows = rows[: a.limit]
    RUNS.mkdir(parents=True, exist_ok=True)
    out = RUNS / f"{a.name}.csv"
    # warm-up (first call loads kernels)
    x, _ = sf.read(CORPUS / rows[0]["cond"] / rows[0]["file"], dtype="float32")
    list(model.transcribe(x, language="es", beam_size=a.beam_size, initial_prompt=prompt, hotwords=hot)[0])
    with open(out, "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["id", "group", "kind", "voice", "cond", "text", "hyp", "audio_s", "stt_s"])
        w.writeheader()
        for i, r in enumerate(rows):
            x, fs = sf.read(CORPUS / r["cond"] / r["file"], dtype="float32")
            t0 = time.perf_counter()
            segs, _info = model.transcribe(x, language="es", beam_size=a.beam_size, initial_prompt=prompt,
                                           hotwords=hot)
            hyp = " ".join(s.text.strip() for s in segs).strip()
            dt = time.perf_counter() - t0
            w.writerow(dict(id=r["id"], group=r["group"], kind=r["kind"], voice=r["voice"], cond=r["cond"],
                            text=r["text"], hyp=hyp, audio_s=round(len(x) / fs, 2), stt_s=round(dt, 3)))
            if i % 200 == 0:
                print(a.name, i, len(rows), file=sys.stderr, flush=True)
    print(out)


def wilson(k, n, z=1.96):
    if n == 0:
        return (math.nan, math.nan)
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return c - h, c + h


def intent_scorer():
    """Returns f(phrase_id, text) -> 'pass' | 'fail' | 'nomatch' | 'n/a' using the
    built-in intent matcher of builtin_intents.py against the fake inventory."""
    import builtin_intents as bi
    import grade
    import ha_fake
    import run as ae
    home = ha_fake.FakeHome()
    match = bi.build_matcher(home)
    tool_names = [t["function"]["name"] for t in ha_fake.build_tools()]
    expect = {i["id"]: i["turns"][0]["expect"] for i in ae.load_phrases()}

    def score(pid, text):
        res = match(bi.remove_punctuation(text))
        if res is None:
            return "nomatch"
        intent, tool, slots = bi.to_call(res, home, tool_names)
        if tool is None:
            return "matched-other"
        _result, meta = home.execute(tool, slots)
        call = {"name": tool, "args": slots, "ok": meta["ok"], "targets": meta["targets"], "meta_args": meta["args"]}
        g = grade.grade_turn(bi._strip_answer(expect[pid]), {"rounds": [{"tool_calls": [call]}], "final_text": "?"},
                             ha_fake.READ_ONLY_TOOLS)
        return g["status"]
    return score


def cmd_score(a):
    import p2_score
    score = intent_scorer()
    ref_ok = {}
    print("| config | cond | n | WER (95% CI by item bootstrap n/a) | items WER=0 | built-in intent pass (control) | p50 / p90 STT s |")
    print("| --- | --- | --- | --- | --- | --- | --- |")
    for name in a.names:
        rows = list(csv.DictReader(open(RUNS / f"{name}.csv")))
        by = {}
        for r in rows:
            by.setdefault(r["cond"], []).append(r)
        for cond in sorted(by, key=lambda c: (c != "clean", c)):
            rs = by[cond]
            err = nref = zero = 0
            ip = ic = 0
            for r in rs:
                e, n = p2_score.wer_counts(r["text"], r["hyp"], v11=True)[:2]
                err += e
                nref += n
                zero += e == 0
                if r["group"] == "control":
                    key = r["id"]
                    if key not in ref_ok:
                        ref_ok[key] = score(key, r["text"])
                    if ref_ok[key] == "pass":  # only phrases the built-in agent handles from the reference text
                        ic += 1
                        ip += score(key, r["hyp"]) == "pass"
            t = sorted(float(r["stt_s"]) for r in rs)
            lo, hi = wilson(ip, ic)
            print(f"| {name} | {cond} | {len(rs)} | {err / nref:.3f} | {zero}/{len(rs)} | {ip}/{ic} ({ip / max(ic, 1):.0%}, "
                  f"{lo:.0%}-{hi:.0%}) | {t[len(t) // 2]:.2f} / {t[int(len(t) * 0.9)]:.2f} |")
    n_ok = sum(v == "pass" for v in ref_ok.values())
    print(f"\nReference texts the built-in agent resolves correctly: {n_ok}/{len(ref_ok)} control phrases "
          f"(intent pass is scored on those only).")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("transcribe")
    t.add_argument("--name", required=True)
    t.add_argument("--model", default="large-v3-turbo")
    t.add_argument("--device", default="cuda")
    t.add_argument("--compute-type", default="int8")
    t.add_argument("--beam-size", type=int, default=5)
    t.add_argument("--initial-prompt", default=None, help="text, or 'auto' (areas and entity names)")
    t.add_argument("--hotwords", default=None, help="text, or 'auto'")
    t.add_argument("--cpu-threads", type=int, default=0)
    t.add_argument("--conds", nargs="*")
    t.add_argument("--limit", type=int, default=0)
    s = sub.add_parser("score")
    s.add_argument("names", nargs="+")
    a = ap.parse_args()
    {"transcribe": cmd_transcribe, "score": cmd_score}[a.cmd](a)


if __name__ == "__main__":
    main()
