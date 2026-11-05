#!/usr/bin/env python3
"""Per-cell table and paired McNemar tests from one or more live AEC A/B trial CSVs.

Input: CSVs in the schema of `aec_live_lib.CSV_COLUMNS` (from `aec_live_ab.py` or from
`aec_live_convert.py` for EXP-018). Several files can be pooled. A cell is a (condition, TV volume,
talker level) combination, so a loud-talker cell of one run is never mixed with a natural-talker
cell of another. Within a cell, wake detections are compared against the baseline arm (default
OFF) on the same item (pairs are (run, cell, item); an item played again in the same arm is paired
by occurrence), with the exact two-sided McNemar test on the discordant pairs.

    aec_live_report.py docs/experiments/data/EXP-018-trials.csv runs/exp019-trials.csv [--baseline OFF]

Rows with an empty `detected` (invalid trials) are skipped and counted. Wilson intervals are 95 %.
"""
import argparse
import collections
import statistics
import sys

import aec_live_lib as L
import p2_score

ARM_ORDER = ["OFF", "DEFAULT", "B", "B+NS"]


def _num(x):
    return float(x) if x not in (None, "") else None


def cell_key(r):
    return (r["condition"], r["tv_volume"], r["talker_level"])


def arm_sort(a):
    return (ARM_ORDER.index(a), a) if a in ARM_ORDER else (len(ARM_ORDER), a)


def analyze(rows, baseline="OFF"):
    """Returns (cells, skipped). cells: list of dicts, one per (cell, arm), in cell/arm order."""
    skipped = 0
    cells = collections.OrderedDict()
    for r in rows:
        if r["detected"] in ("", None):
            skipped += 1
            continue
        key = cell_key(r)
        arm = r["arm"]
        c = cells.setdefault(key, collections.OrderedDict()).setdefault(arm, {"trials": []})
        c["trials"].append(r)
    out = []
    for key in sorted(cells, key=lambda k: (k[0], int(k[1]), k[2])):
        arms = cells[key]
        base_map = None
        if baseline in arms:
            base_map = _pair_map(arms[baseline]["trials"])
        for arm in sorted(arms, key=arm_sort):
            tr = arms[arm]["trials"]
            n = len(tr)
            k = sum(int(r["detected"]) for r in tr)
            lo, hi = p2_score.wilson(k, n)
            row = {"condition": key[0], "tv_volume": key[1], "talker_level": key[2], "arm": arm, "n": n, "k": k,
                   "rate": k / n if n else float("nan"), "lo": lo, "hi": hi,
                   "runs": sorted({r["run_id"] for r in tr})}
            lat = [_num(r["wake_latency_ms"]) for r in tr if r["wake_latency_ms"] not in ("", None)]
            row["latency_ms_median"] = statistics.median(lat) if lat else None
            stt = [int(r["stt_match"]) for r in tr if r["stt_match"] not in ("", None)]
            row["stt"] = (sum(stt), len(stt))
            for col, name in (("erle_db", "erle_median"), ("delay_ms", "delay_median"),
                              ("daemon_cpu_pct", "cpu_median")):
                v = [_num(r[col]) for r in tr if r[col] not in ("", None)]
                row[name] = statistics.median(v) if v else None
            row["underruns"] = sum(int(float(r["underruns"])) for r in tr if r["underruns"] not in ("", None))
            if base_map is not None and arm != baseline:
                a_only = b_only = pairs = 0
                for pk, v in _pair_map(tr).items():
                    if pk in base_map:
                        pairs += 1
                        a_only += v and not base_map[pk]
                        b_only += base_map[pk] and not v
                row["paired"] = {"pairs": pairs, "arm_only": a_only, "base_only": b_only,
                                 "p": L.mcnemar_exact(a_only, b_only)}
            out.append(row)
    return out, skipped


def _pair_map(trials):
    seen = collections.Counter()
    m = {}
    for r in trials:
        base = (r["run_id"], r["item_id"])
        seen[base] += 1
        m[base + (seen[base],)] = int(r["detected"])
    return m


def format_table(cells, baseline="OFF"):
    hdr = (f"{'condition':9s} {'vol':>3s} {'talker':8s} {'arm':8s} {'wake k/n':>9s} {'rate':>5s} {'95% Wilson':>12s}  "
           f"{'vs ' + baseline + ' (arm-only/' + baseline + '-only, p)':36s} {'STT ok':>7s} {'lat ms':>6s} "
           f"{'ERLE dB':>7s} {'delay ms':>8s}")
    lines = [hdr, "-" * len(hdr)]
    for c in cells:
        p = c.get("paired")
        pt = f"{p['arm_only']}/{p['base_only']}, p {p['p']:.3f} ({p['pairs']} pairs)" if p else ""
        stt = f"{c['stt'][0]}/{c['stt'][1]}" if c["stt"][1] else ""
        lines.append(f"{c['condition']:9s} {c['tv_volume']:>3s} {c['talker_level']:8s} {c['arm']:8s} "
                     f"{c['k']:>4d}/{c['n']:<4d} {100 * c['rate']:4.0f}% [{100 * c['lo']:3.0f}, {100 * c['hi']:3.0f}]  "
                     f"{pt:36s} {stt:>7s} {'' if c['latency_ms_median'] is None else format(c['latency_ms_median'], '.0f'):>6s} "
                     f"{'' if c['erle_median'] is None else format(c['erle_median'], '.1f'):>7s} "
                     f"{'' if c['delay_median'] is None else format(c['delay_median'], '.1f'):>8s}")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("csv", nargs="+")
    ap.add_argument("--baseline", default="OFF")
    a = ap.parse_args()
    rows = L.read_rows(a.csv)
    cells, skipped = analyze(rows, a.baseline)
    print(f"{len(rows)} rows from {len(a.csv)} file(s), runs: {', '.join(sorted({r['run_id'] for r in rows}))}"
          + (f"; {skipped} invalid trials skipped" if skipped else ""))
    print(format_table(cells, a.baseline))
    print("\nLatency is from the item's nominal start to the entity's idle to listening change (median of "
          "detected trials); ERLE and delay are medians of the 60 s windows covering each trial.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
