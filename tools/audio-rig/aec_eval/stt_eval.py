#!/usr/bin/env python3
"""Offline STT word error rate of the front-end variants (faster-whisper, as HA's STT runs it).

Transcribes the command segment of every `pos_cmd` item in the WAVs `semisyn_ab.py --keep-wavs`
kept (16 kHz mono S16, the stream HA would receive), per condition and variant, and scores WER
with P2-corpus.md's normalization (`p2_score.wer_counts`, v1 and v1.1). The segment is the
P2-corpus "command segment": from `command_start` - 0.5 s to the item end + 0.3 s, in the
item timeline (the kept WAVs start `--lead` s before the item). This does not depend on the
wake word result, so variants are compared on the same 70 commands. `--digital` adds the clean
corpus items (same segment) as the ceiling.

    stt_eval.py --results final.jsonl [--results ...] --variant noaec --variant aec [...] \
        [--model large-v3-turbo] [--device cuda] [--digital devices/private/corpus/p2-v1] --out stt.jsonl

Install (separate venv): pip install faster-whisper. Transcripts stay in the private --out
file; only WER numbers are printed. Never commit the transcripts or audio.
"""
import argparse
import collections
import json
import os
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from p2_score import wer_counts  # noqa: E402

RATE = 16000


def segment(x, item, lead):
    a = int((lead + item["command_start_ms"] / 1000 - 0.5) * RATE)
    b = int((lead + item["duration_ms"] / 1000 + 0.3) * RATE)
    return x[max(0, a):min(len(x), b)]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--results", action="append", required=True)
    ap.add_argument("--variant", action="append", required=True)
    ap.add_argument("--condition", action="append", help="only these conditions")
    ap.add_argument("--digital", help="corpus dir: also transcribe the clean items")
    ap.add_argument("--model", default="large-v3-turbo")
    ap.add_argument("--device", default="cuda")
    ap.add_argument("--beam-size", type=int, default=1)
    ap.add_argument("--lead", type=float, default=2.0)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    import soundfile as sf
    from faster_whisper import WhisperModel
    from scipy.io import wavfile
    model = WhisperModel(a.model, device=a.device, compute_type="float16" if a.device == "cuda" else "int8")

    def transcribe(x):
        segs, _ = model.transcribe(x.astype(np.float32) / 32768, language="es", beam_size=a.beam_size,
                                   vad_filter=False, condition_on_previous_text=False)
        return "".join(s.text for s in segs).strip()

    rows = []
    for p in a.results:
        rows += [json.loads(line) for line in open(p)]
    jobs = {}
    for r in rows:
        it = r["item"]
        if it["kind"] != "pos" or it["subkind"] != "cmd" or r["variant"] not in a.variant or not r.get("wav"):
            continue
        cond = f"{r['condition']}@{r['speech_gain_db']:g}"
        if a.condition and cond not in a.condition and r["condition"] not in a.condition:
            continue
        jobs[(cond, r["variant"], it["id"])] = r
    acc = collections.defaultdict(lambda: [0, 0, 0, 0])  # errors, words, v11 errors, items word-perfect
    with open(a.out, "w") as fo:
        if a.digital:
            items = {r["item"]["id"]: r["item"] for r in jobs.values()}
            for iid, it in sorted(items.items()):
                x, sr = sf.read(os.path.join(a.digital, it["_file"]), dtype="int16")
                pad = np.concatenate([np.zeros(int(a.lead * RATE), np.int16), x, np.zeros(RATE, np.int16)])
                hyp = transcribe(segment(pad, it, a.lead))
                e, n, _ = wer_counts(it["expected_command_text"], hyp)
                e11 = wer_counts(it["expected_command_text"], hyp, v11=True)[0]
                k = ("digital", "clean")
                acc[k][0] += e
                acc[k][1] += n
                acc[k][2] += e11
                acc[k][3] += e == 0
                fo.write(json.dumps({"condition": "digital", "variant": "clean", "id": iid, "voice": it["voice"],
                                     "hyp": hyp, "errors": e, "words": n, "errors_v11": e11}, ensure_ascii=False) + "\n")
        for (cond, var, iid), r in sorted(jobs.items()):
            fs, x = wavfile.read(r["wav"])
            assert fs == RATE
            it = r["item"]
            hyp = transcribe(segment(x, it, a.lead))
            e, n, _ = wer_counts(it["expected_command_text"], hyp)
            e11 = wer_counts(it["expected_command_text"], hyp, v11=True)[0]
            k = (cond, var)
            acc[k][0] += e
            acc[k][1] += n
            acc[k][2] += e11
            acc[k][3] += e == 0
            fo.write(json.dumps({"condition": cond, "variant": var, "id": iid, "voice": it["voice"], "hyp": hyp,
                                 "errors": e, "words": n, "errors_v11": e11}, ensure_ascii=False) + "\n")
    print("| condition | variant | WER v1 | WER v1.1 | word-perfect |")
    print("| --- | --- | --- | --- | --- |")
    for (cond, var), (e, n, e11, ok) in sorted(acc.items()):
        print(f"| {cond} | {var} | {e}/{n} = {100 * e / n:.1f} % | {100 * e11 / n:.1f} % | {ok} |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
