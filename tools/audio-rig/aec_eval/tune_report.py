#!/usr/bin/env python3
"""Tables for the AEC tuning experiment: wake rates per variant and condition (from
`tune_eval.py items` trace rows), echo removal and content false wakes (`tune_eval.py content`
rows), the Pareto front of the two, and DET sweeps over the wake threshold and trigger level.

    tune_report.py rates ITEMS.jsonl [...] [--split held] [--threshold 0.5] [--trigger-level 1]
        [--content C.jsonl] [--paired BASE] [--markdown]
    tune_report.py pareto ITEMS.jsonl [...] --content C.jsonl [--split tune]
    tune_report.py det ITEMS.jsonl [...] [--negatives NEG.jsonl] [--content C.jsonl] --variant V [...]
        [--threshold 0.2 ...] [--trigger-level 1 --trigger-level 2]

Positives count as detected when a detection falls in the P2-corpus window (wake start - 0.2 s
to wake end + 1.5 s); a detection before the window in the same stream is a "lead-in" false
wake (in HA it would have ended the wake stage first). Negatives and near-misses count items
with any detection. Content false wakes come from consecutive Wyoming streams; rates carry exact
Poisson 95 % intervals (rule of three for zero).
"""
import argparse
import collections
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
from p2_score import match_wake, poisson_rate_ci, wilson  # noqa: E402
from tune_eval import MLS, detections, in_split  # noqa: E402


def load(paths):
    rows = []
    for p in paths:
        rows += [json.loads(line) for line in open(p)]
    return rows


def score(row, thr, lvl):
    """(detected, lead_in_false_wake) of a positive trace row; for other kinds (False, any)."""
    d = detections(row, thr, lvl)
    it = row["item"]
    if it["kind"] != "pos":
        return False, bool(d)
    m = match_wake(it, d)
    lo = it["wake_start_ms"] / 1000 - 0.2
    return m["detected"], any(t < lo for t in d)


def pct(k, n):
    if n == 0:
        return "0/0"
    lo, hi = wilson(k, n)
    return f"{k}/{n} = {100 * k / n:.0f} % [{100 * lo:.0f}, {100 * hi:.0f}]"


def rates(rows, thr=0.5, lvl=1, split=None, no_mls=False):
    """{(cond, variant): (detected, n, lead_in)} for positives."""
    acc = collections.defaultdict(lambda: [0, 0, 0])
    for r in rows:
        it = r["item"]
        if it["kind"] != "pos" or not in_split(it, split) or (no_mls and it["voice"] in MLS):
            continue
        det, lead = score(r, thr, lvl)
        a = acc[(r["cond"], r["variant"])]
        a[0] += det
        a[1] += 1
        a[2] += lead
    return acc


def content_index(crows):
    """{variant: {capture: row}}."""
    idx = collections.defaultdict(dict)
    for r in crows:
        idx[r["variant"]][r["capture"]] = r
    return idx


def mean_erle(cidx, v, caps=("content-v10", "content-v15")):
    vals = [cidx[v][c].get("erle_mean_db") for c in caps if c in cidx.get(v, {})]
    vals = [x for x in vals if x is not None]
    return sum(vals) / len(vals) if len(vals) == len(caps) else None


def table(md, header, body):
    if md:
        out = ["| " + " | ".join(header) + " |", "| " + " | ".join("---" for _ in header) + " |"]
        out += ["| " + " | ".join(str(c) for c in r) + " |" for r in body]
    else:
        w = [max(len(str(r[i])) for r in [header] + body) for i in range(len(header))]
        out = ["  ".join(str(c).ljust(w[i]) for i, c in enumerate(r)) for r in [header] + body]
    return "\n".join(out)


def cmd_rates(a):
    rows = load(a.items)
    acc = rates(rows, a.threshold, a.trigger_level, a.split, a.no_mls)
    conds = sorted({c for c, _ in acc}, key=lambda c: (c.split("@")[1], c))
    variants = list(dict.fromkeys(v for _, v in acc))
    cidx = content_index(load(a.content)) if a.content else {}
    header = ["variant"] + conds + (["ERLE v10/v15 dB"] if cidx else [])
    body = []
    for v in variants:
        line = [v]
        for c in conds:
            k, n, lead = acc.get((c, v), (0, 0, 0))
            line.append(pct(k, n) + (f" ({lead} lead-in)" if lead else ""))
        if cidx:
            e = [cidx.get(v, {}).get(cap, {}).get("erle_mean_db") for cap in ("content-v10", "content-v15")]
            line.append(" / ".join("n/a" if x is None else f"{x:.1f}" for x in e))
        body.append(line)
    print(table(a.markdown, header, body))
    if a.paired:
        print()
        print(paired(rows, a.paired, a.threshold, a.trigger_level, a.split, a.no_mls, a.markdown))
    return 0


def paired(rows, base, thr, lvl, split, no_mls, md):
    from ab_report import mcnemar
    res = collections.defaultdict(dict)
    for r in rows:
        it = r["item"]
        if it["kind"] != "pos" or not in_split(it, split) or (no_mls and it["voice"] in MLS):
            continue
        res[(r["cond"], r["variant"])][it["id"]] = score(r, thr, lvl)[0]
    body = []
    for (c, v), d in res.items():
        if v == base or (c, base) not in res:
            continue
        b = res[(c, base)]
        g = sum(1 for i in d if d[i] and not b.get(i, False))
        lo = sum(1 for i in d if b.get(i, False) and not d[i])
        body.append([c, v, g, lo, f"{mcnemar(g, lo):.3f}"])
    return table(md, ["condition", f"variant vs {base}", "gained", "lost", "McNemar p"], body)


def cmd_pareto(a):
    """Echo removal (mean in-band ERLE over the content recordings) against wake detections
    on content (sum over the content conditions); marks the non-dominated variants."""
    rows = load(a.items)
    acc = rates(rows, a.threshold, a.trigger_level, a.split, a.no_mls)
    cidx = content_index(load(a.content))
    pts = []
    for v in dict.fromkeys(v for _, v in acc):
        k = sum(acc[(c, v)][0] for c, vv in acc if vv == v and not c.startswith("quiet"))
        n = sum(acc[(c, v)][1] for c, vv in acc if vv == v and not c.startswith("quiet"))
        e = mean_erle(cidx, v)
        mx = [cidx.get(v, {}).get(cap, {}).get("max_score") for cap in ("content-v10", "content-v15")]
        pts.append((v, k, n, e, mx))
    front = set()
    for v, k, n, e, _ in pts:
        if e is None:
            continue
        dominated = any(e2 is not None and k2 >= k and e2 >= e and (k2 > k or e2 > e) for _, k2, _, e2, _ in pts)
        if not dominated:
            front.add(v)
    body = [[("* " if v in front else "") + v, pct(k, n), "n/a" if e is None else f"{e:.1f}",
             " / ".join("-" if m is None else f"{m:.2f}" for m in mx)]
            for v, k, n, e, mx in sorted(pts, key=lambda p: (-(p[3] or -99), -p[1]))]
    print(table(a.markdown, ["variant (* Pareto)", "wake on content", "ERLE dB", "max score v10/v15"], body))
    return 0


def cmd_det(a):
    rows = [r for r in load(a.items) if r["variant"] in a.variant]
    negs = [r for r in load(a.negatives) if r["variant"] in a.variant] if a.negatives else []
    cidx = content_index(load(a.content)) if a.content else {}
    thrs = a.threshold or [0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6]
    lvls = a.trigger_level or [1, 2]
    conds = sorted({r["cond"] for r in rows}, key=lambda c: (c.split("@")[1], c))
    body = []
    for v in a.variant:
        for lvl in lvls:
            for t in thrs:
                line = [v, lvl, f"{t:g}"]
                for c in conds:
                    rs = [r for r in rows if r["variant"] == v and r["cond"] == c and r["item"]["kind"] == "pos"
                          and in_split(r["item"], a.split) and not (a.no_mls and r["item"]["voice"] in MLS)]
                    k = sum(score(r, t, lvl)[0] for r in rs)
                    line.append(f"{100 * k / len(rs):.0f} %" if rs else "-")
                if negs:
                    ns = [r for r in negs if r["variant"] == v and r["item"]["kind"] != "pos"]
                    k = sum(score(r, t, lvl)[1] for r in ns)
                    line.append(f"{k}/{len(ns)}")
                if cidx:
                    tot, armed = 0, 0.0
                    for cap, cr in sorted(cidx.get(v, {}).items()):
                        fw = (cr.get("false_wakes") or {}).get(f"{t:g}/{lvl}")
                        if fw is None:
                            continue
                        tot += fw["n"]
                        armed += fw["armed_s"]
                    if armed:
                        lo, hi = poisson_rate_ci(tot, armed / 3600)
                        line.append(f"{tot} in {armed / 60:.0f} min ({tot / (armed / 3600):.1f}/h, <= {hi:.1f}/h)")
                    else:
                        line.append("-")
                body.append(line)
    header = ["variant", "level", "threshold"] + conds + (["neg+near items"] if negs else []) + \
        (["content false wakes"] if cidx else [])
    print(table(a.markdown, header, body))
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = argparse.ArgumentParser(add_help=False)
    c.add_argument("items", nargs="+")
    c.add_argument("--split", choices=["tune", "held", "all"])
    c.add_argument("--no-mls", action="store_true")
    c.add_argument("--markdown", action="store_true")
    c.add_argument("--content", action="append", default=[])
    r = sub.add_parser("rates", parents=[c])
    r.add_argument("--threshold", type=float, default=0.5)
    r.add_argument("--trigger-level", type=int, default=1)
    r.add_argument("--paired")
    p = sub.add_parser("pareto", parents=[c])
    p.add_argument("--threshold", type=float, default=0.5)
    p.add_argument("--trigger-level", type=int, default=1)
    d = sub.add_parser("det", parents=[c])
    d.add_argument("--variant", action="append", required=True)
    d.add_argument("--negatives", action="append", default=[])
    d.add_argument("--threshold", type=float, action="append")
    d.add_argument("--trigger-level", type=int, action="append")
    a = ap.parse_args()
    return {"rates": cmd_rates, "pareto": cmd_pareto, "det": cmd_det}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
