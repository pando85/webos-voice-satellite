#!/usr/bin/env python3
"""Tables for the offline front-end A/B: wake rates per condition and variant, per voice,
without the MLS voices, and the agreement with Home Assistant's session 2 results.

    ab_report.py results.jsonl [...] [--by-voice] [--calibration] [--paired BASE] [--markdown]

Input: `semisyn_ab.py` result lines. Detections are scored with the P2-corpus window
(`p2_score.match_wake`: wake start - 0.2 s to wake end + 1.5 s). Rates carry 95 % Wilson
intervals. `--calibration` compares each variant with HA's detections stored in the same rows
(`ha_detections_s`, scored the same way): per-item agreement and Cohen's kappa. `--paired BASE`
compares every variant with variant BASE on the same items of the same condition (exact two-sided
McNemar test on the discordant items). Negatives and near-misses, if present, are reported as
items with any detection (all are false wakes).
"""
import argparse
import collections
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from p2_score import match_wake, wilson  # noqa: E402

MLS = ("es_ES-mls_9972-low", "es_ES-mls_10246-low")


def rate(k, n):
    if n == 0:
        return "0/0"
    lo, hi = wilson(k, n)
    return f"{k}/{n} = {100 * k / n:.0f} % [{100 * lo:.0f}, {100 * hi:.0f}]"


def kappa(pairs):
    """Cohen's kappa for a list of (a, b) booleans."""
    n = len(pairs)
    if n == 0:
        return float("nan")
    po = sum(a == b for a, b in pairs) / n
    pa = sum(a for a, _ in pairs) / n
    pb = sum(b for _, b in pairs) / n
    pe = pa * pb + (1 - pa) * (1 - pb)
    return 1.0 if pe == 1 else (po - pe) / (1 - pe)


def load(paths):
    rows = []
    for p in paths:
        rows += [json.loads(line) for line in open(p)]
    for r in rows:
        r["detected"] = match_wake(r["item"], r["detections_s"])["detected"]
        if r.get("ha_detections_s") is not None:
            r["ha_detected"] = match_wake(r["item"], r["ha_detections_s"])["detected"]
    return rows


def group(rows):
    g = collections.OrderedDict()
    for r in rows:
        g.setdefault((r["condition"], r["variant"]), []).append(r)
    return g


def mcnemar(b, c):
    """Exact two-sided McNemar p-value for b and c discordant pairs."""
    from math import comb
    n = b + c
    if n == 0:
        return 1.0
    k = min(b, c)
    return min(1.0, 2 * sum(comb(n, i) for i in range(k + 1)) / 2 ** n)


def paired(rows, base, md=False):
    out = [["condition", "variant", f"vs {base}", "gained", "lost", "McNemar p"]]
    for (c, v), rs in group(rows).items():
        if v == base:
            continue
        ref = {r["item"]["id"]: r["detected"] for r in rows
               if r["condition"] == c and r["variant"] == base and r["item"]["kind"] == "pos"}
        pairs = [(ref[r["item"]["id"]], r["detected"]) for r in rs
                 if r["item"]["kind"] == "pos" and r["item"]["id"] in ref]
        if not pairs:
            continue
        gained = sum((not a) and b for a, b in pairs)
        lost = sum(a and not b for a, b in pairs)
        out.append([c, v, f"{sum(b for _, b in pairs)} vs {sum(a for a, _ in pairs)} of {len(pairs)}",
                    str(gained), str(lost), f"{mcnemar(gained, lost):.3f}"])
    return fmt(out, md)


def table(rows, md=False):
    out = []
    hdr = ["condition", "variant", "wake (all)", "without MLS", "alone", "with command", "clipped items",
           "neg / near-miss items with a wake"]
    out.append(hdr)
    for (c, v), rs in group(rows).items():
        neg = [r for r in rs if r["item"]["kind"] == "neg"]
        nm = [r for r in rs if r["item"]["kind"] == "nearmiss"]
        fw = (f"{sum(bool(r['detections_s']) for r in neg)}/{len(neg)}, "
              f"{sum(bool(r['detections_s']) for r in nm)}/{len(nm)}") if neg or nm else ""
        pos = [r for r in rs if r["item"]["kind"] == "pos"]
        if not pos:
            out.append([c, v, "", "", "", "", "", fw])
            continue
        nm = [r for r in pos if r["item"]["voice"] not in MLS]
        al = [r for r in pos if r["item"]["subkind"] == "alone"]
        cm = [r for r in pos if r["item"]["subkind"] == "cmd"]
        out.append([c, v, rate(sum(r["detected"] for r in pos), len(pos)),
                    rate(sum(r["detected"] for r in nm), len(nm)),
                    f"{sum(r['detected'] for r in al)}/{len(al)}", f"{sum(r['detected'] for r in cm)}/{len(cm)}",
                    str(sum(r.get("clipped", 0) > 0 for r in pos)), fw])
    return fmt(out, md)


def by_voice(rows, md=False):
    voices = sorted({(r["item"]["voice"], r["item"].get("speaker") or "") for r in rows})
    out = [["condition", "variant"] + [f"{v.split('-')[1] if '-' in v else v}{('/' + s) if s and 'sharvard' in v else ''}"
                                       for v, s in voices]]
    for (c, v), rs in group(rows).items():
        line = [c, v]
        for vo, sp in voices:
            sel = [r for r in rs if r["item"]["voice"] == vo and (r["item"].get("speaker") or "") == sp]
            line.append(f"{sum(r['detected'] for r in sel)}/{len(sel)}")
        out.append(line)
    return fmt(out, md)


def calibration(rows, md=False):
    out = [["condition", "variant", "offline", "HA (session 2)", "both", "neither", "offline only", "HA only",
            "agreement", "kappa"]]
    for (c, v), rs in group(rows).items():
        rs = [r for r in rs if "ha_detected" in r]
        if not rs:
            continue
        pairs = [(r["detected"], r["ha_detected"]) for r in rs]
        n = len(pairs)
        both = sum(a and b for a, b in pairs)
        nei = sum((not a) and (not b) for a, b in pairs)
        out.append([c, v, f"{sum(a for a, _ in pairs)}/{n}", f"{sum(b for _, b in pairs)}/{n}", str(both), str(nei),
                    str(sum(a and not b for a, b in pairs)), str(sum(b and not a for a, b in pairs)),
                    f"{100 * (both + nei) / n:.0f} %", f"{kappa(pairs):.2f}"])
    return fmt(out, md)


def fmt(table_rows, md):
    if md:
        lines = ["| " + " | ".join(table_rows[0]) + " |", "| " + " | ".join("---" for _ in table_rows[0]) + " |"]
        lines += ["| " + " | ".join(r) + " |" for r in table_rows[1:]]
        return "\n".join(lines)
    w = [max(len(r[i]) for r in table_rows) for i in range(len(table_rows[0]))]
    return "\n".join("  ".join(c.ljust(w[i]) for i, c in enumerate(r)) for r in table_rows)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("results", nargs="+")
    ap.add_argument("--by-voice", action="store_true")
    ap.add_argument("--calibration", action="store_true")
    ap.add_argument("--paired", metavar="BASE", help="McNemar comparison of each variant with BASE")
    ap.add_argument("--markdown", action="store_true")
    a = ap.parse_args()
    rows = load(a.results)
    print(table(rows, a.markdown))
    if a.by_voice:
        print()
        print(by_voice(rows, a.markdown))
    if a.paired:
        print()
        print(paired(rows, a.paired, a.markdown))
    if a.calibration:
        print()
        print(calibration(rows, a.markdown))
    return 0


if __name__ == "__main__":
    sys.exit(main())
