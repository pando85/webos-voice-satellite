#!/usr/bin/env python3
"""Fetch and prepare the license-clean TV-content clip set for Phase 2 (played from the PC over HDMI).

Output (default devices/private/content/p2-v1/, gitignored; never commit audio):
  <category>__<id>.wav    PCM_16, 48 kHz, stereo, EBU R128 integrated loudness about -23 LUFS
  sampler_10min.wav       10 minutes mixed from the clips (sampler_segments.csv lists the pieces)
  manifest.csv            file, category, duration_s, source_url, license, ... , lufs_i, true_peak_dbtp
  _downloads/             the raw downloaded media (untrusted data; kept for re-runs, delete when done)

Safety: only HTTPS URLs on archive.org hosts are fetched, and only media files; nothing downloaded is
executed or imported. ffmpeg decodes the media with an explicit demuxer (-f mp3 / -f ogg) and
-protocol_whitelist file, so playlists or containers cannot make it open network URLs.
Downloads go into a fresh directory (created here); a non-empty _downloads/ is reused only with --reuse.

License policy: Creative Commons (BY, CC0) or public domain only, license and URL recorded per item
in manifest.csv. See tools/audio-rig/README.md. Re-run is deterministic given the same sources.
"""
import argparse
import csv
import hashlib
import json
import re
import subprocess
import sys
import urllib.parse
import urllib.request
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = ROOT / "devices" / "private" / "content" / "p2-v1"
LUFS_TARGET, TP_MAX, LRA = -23.0, -2.0, 20.0
FS = 48000
MAX_DOWNLOAD = 150_000_000
ALLOWED_SUFFIX = ".archive.org"

PD = "http://creativecommons.org/licenses/publicdomain/"
PDM = "http://creativecommons.org/publicdomain/mark/1.0/"
BY3 = "http://creativecommons.org/licenses/by/3.0/"
LV = "https://archive.org/download/{item}/{file}"

# category, id, download URL, license name, license URL, title, attribution, container, start s, duration s, byte range
SOURCES = [
    dict(cat="speech_es", id="lazarillo_tratados_6_7", item="lazarillo_0912_librivox",
         file="Lazarillo_de_Tormes_5_anonimo_64kb.mp3", lic="Public domain (LibriVox)", lic_url=PD,
         title="Lazarillo de Tormes, tratados sexto y septimo (LibriVox, read by Tux)",
         attr="LibriVox volunteer reader Tux; text anonymous, 16th century", start=0, dur=440),
    dict(cat="speech_es", id="caperucita_roja", item="cuentos_grimm_01_1109_librivox",
         file="cuentos_01_grimm_64kb.mp3", lic="Public domain mark (LibriVox)", lic_url=PDM,
         title="Cuentos de Hadas: Caperucita Roja (LibriVox, Grimm)", attr="LibriVox volunteer reader; Brothers Grimm",
         start=0, dur=640),
    dict(cat="speech_en", id="sherlock_scandal_in_bohemia", item="adventures_holmes",
         file="adventureholmes_01_doyle_64kb.mp3", lic="Public domain (LibriVox)", lic_url=PD,
         title="The Adventures of Sherlock Holmes: A Scandal in Bohemia (LibriVox)",
         attr="LibriVox volunteer reader; Arthur Conan Doyle", start=30, dur=420, byte_range=(0, 4_300_000)),
    dict(cat="speech_en", id="tears_of_steel_dialogue", item="Tears-of-Steel", file="tears_of_steel_720p.ogv",
         lic="CC BY 3.0", lic_url=BY3, title="Tears of Steel, dialogue scenes (film audio, English)",
         attr="(CC) Blender Foundation | mango.blender.org", start=20, dur=370, container="ogg"),
    dict(cat="music_vocals", id="woodward_cherubs", item="JoshWoodwardSelection",
         file="01-JoshWoodward-Ashes-02-Cherubs.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Josh Woodward - Cherubs (Ashes)", attr="Josh Woodward, joshwoodward.com", start=0, dur=300),
    dict(cat="music_vocals", id="woodward_go", item="JoshWoodwardSelection",
         file="06-JoshWoodward-TheSimpleLife-02-Go.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Josh Woodward - Go (The Simple Life)", attr="Josh Woodward, joshwoodward.com", start=0, dur=300),
    dict(cat="music_vocals", id="woodward_saturday", item="JoshWoodwardSelection",
         file="07-JoshWoodward-TheSimpleLife-03-Saturday.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Josh Woodward - Saturday (The Simple Life)", attr="Josh Woodward, joshwoodward.com", start=0, dur=300),
    dict(cat="music_vocals", id="woodward_effortless", item="JoshWoodwardSelection",
         file="11-JoshWoodward-NotQuiteConnected-02-Effortless.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Josh Woodward - Effortless (Not Quite Connected)", attr="Josh Woodward, joshwoodward.com", start=0, dur=300),
    dict(cat="music_novocals", id="macleod_canon_in_d", item="Classical_Sampler-9615",
         file="Kevin_MacLeod_-_Canon_in_D_Major.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Kevin MacLeod - Canon in D Major (performance)", attr="Kevin MacLeod (incompetech.com)", start=0, dur=400),
    dict(cat="music_novocals", id="macleod_gymnopedie_1", item="Classical_Sampler-9615",
         file="Kevin_MacLeod_-_Gymnopedie_No_1.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Kevin MacLeod - Gymnopedie No 1 (performance)", attr="Kevin MacLeod (incompetech.com)", start=0, dur=300),
    dict(cat="music_novocals", id="macleod_backed_vibes_clean", item="Jazz_Sampler-9619",
         file="Kevin_MacLeod_-_Backed_Vibes_Clean.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Kevin MacLeod - Backed Vibes Clean", attr="Kevin MacLeod (incompetech.com)", start=0, dur=300),
    dict(cat="music_novocals", id="macleod_faster_does_it", item="Jazz_Sampler-9619",
         file="Kevin_MacLeod_-_Faster_Does_It.mp3", lic="CC BY 3.0", lic_url=BY3,
         title="Kevin MacLeod - Faster Does It", attr="Kevin MacLeod (incompetech.com)", start=0, dur=300),
    dict(cat="film_action", id="tears_of_steel_battle", item="Tears-of-Steel", file="tears_of_steel_720p.ogv",
         lic="CC BY 3.0", lic_url=BY3, title="Tears of Steel, robot battle and aftermath (film audio)",
         attr="(CC) Blender Foundation | mango.blender.org", start=400, dur=300, container="ogg"),
]

# 10 segments of 60 s: (clip id, offset s inside the normalized clip)
SAMPLER = [("lazarillo_tratados_6_7", 60), ("tears_of_steel_dialogue", 30), ("woodward_cherubs", 40),
           ("tears_of_steel_battle", 0), ("caperucita_roja", 200), ("sherlock_scandal_in_bohemia", 120),
           ("macleod_canon_in_d", 30), ("woodward_saturday", 60), ("tears_of_steel_battle", 120),
           ("macleod_backed_vibes_clean", 60)]
SAMPLER_SEG_S, SAMPLER_FADE_S = 60, 0.5

COLUMNS = ["file", "category", "duration_s", "source_url", "license", "license_url", "title", "attribution",
           "source_item_page", "segment_start_s", "segment_s", "lufs_i", "true_peak_dbtp", "sha256_download"]


def url_of(s):
    return LV.format(item=urllib.parse.quote(s["item"]), file=urllib.parse.quote(s["file"]))


def check_url(url):
    p = urllib.parse.urlparse(url)
    if p.scheme != "https" or not (p.hostname == "archive.org" or p.hostname.endswith(ALLOWED_SUFFIX)):
        sys.exit(f"refusing non-HTTPS or non-archive.org URL: {url}")
    if not re.search(r"\.(mp3|ogv|ogg|mp4)$", p.path):
        sys.exit(f"refusing non-media URL: {url}")


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        check_url(newurl)  # redirects (archive.org -> dnNNN.us.archive.org) must stay on the allowlist
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def download(url, dest, byte_range=None):
    check_url(url)
    req = urllib.request.Request(url, headers={"User-Agent": "webos-voice-satellite-p2-corpus/1"})
    if byte_range:
        req.add_header("Range", f"bytes={byte_range[0]}-{byte_range[1]}")
    opener = urllib.request.build_opener(SafeRedirect)
    with opener.open(req, timeout=60) as r, open(dest, "wb") as fh:
        n = 0
        while True:
            b = r.read(1 << 20)
            if not b:
                break
            n += len(b)
            if n > MAX_DOWNLOAD:
                sys.exit(f"download too large: {url}")
            fh.write(b)
    if dest.stat().st_size < 10_000:
        sys.exit(f"suspiciously small download ({dest.stat().st_size} B): {url}")


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for b in iter(lambda: fh.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def ffmpeg(args, capture=False):
    cmd = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "info" if capture else "error",
           "-protocol_whitelist", "file"] + args
    r = subprocess.run(cmd, stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
    if r.returncode:
        sys.exit(f"ffmpeg failed: {' '.join(cmd)}\n{r.stderr[-800:]}")
    return r.stderr


def measure(path, extra=""):
    err = ffmpeg(["-i", str(path), "-af", f"loudnorm=I={LUFS_TARGET}:TP={TP_MAX}:LRA={LRA}:print_format=json{extra}",
                  "-f", "null", "-"], capture=True)
    return json.loads(err[err.rindex("{"):err.rindex("}") + 1])


def normalize(src, dst):
    """Two-pass linear loudnorm to -23 LUFS, 48 kHz stereo PCM_16. Returns the measurement of the result."""
    m = measure(src)
    af = (f"loudnorm=I={LUFS_TARGET}:TP={TP_MAX}:LRA={LRA}:measured_I={m['input_i']}:measured_TP={m['input_tp']}:"
          f"measured_LRA={m['input_lra']}:measured_thresh={m['input_thresh']}:offset={m['target_offset']}:linear=true")
    ffmpeg(["-i", str(src), "-af", af, "-ar", str(FS), "-ac", "2", "-c:a", "pcm_s16le", "-y", str(dst)])
    out = measure(dst)
    return {"lufs_i": float(out["input_i"]), "true_peak_dbtp": float(out["input_tp"]), "pass1": m}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--reuse", action="store_true", help="reuse files already in _downloads/ (re-run)")
    args = ap.parse_args()
    out = args.out
    dl = out / "_downloads"
    work = out / "_work"
    if dl.exists() and any(dl.iterdir()) and not args.reuse:
        sys.exit(f"{dl} is not empty; pass --reuse to reuse it, or delete it (downloads must start fresh)")
    for d in (dl, work):
        d.mkdir(parents=True, exist_ok=True)

    # 1. downloads (one file per distinct URL + range)
    fetched = {}
    for s in SOURCES:
        url = url_of(s)
        key = (url, s.get("byte_range"))
        if key not in fetched:
            dest = dl / f"{len(fetched):02d}_{Path(urllib.parse.urlparse(url).path).name}"
            if not (args.reuse and dest.exists()):
                print(f"download {url} {s.get('byte_range') or ''}")
                download(url, dest, s.get("byte_range"))
            fetched[key] = dest
        s["_path"], s["_url"] = fetched[key], url

    # 2. decode, cut, normalize
    rows = []
    for s in SOURCES:
        fmt = s.get("container", "mp3")
        seg = work / f"{s['id']}__raw.wav"
        ffmpeg(["-f", fmt, "-ss", str(s["start"]), "-t", str(s["dur"]), "-i", str(s["_path"]), "-vn",
                "-ac", "2", "-ar", str(FS), "-c:a", "pcm_f32le", "-y", str(seg)])
        final = out / f"{s['cat']}__{s['id']}.wav"
        res = normalize(seg, final)
        info = sf.info(str(final))
        rows.append({"file": final.name, "category": s["cat"], "duration_s": round(info.duration, 2),
                     "source_url": s["_url"], "license": s["lic"], "license_url": s["lic_url"], "title": s["title"],
                     "attribution": s["attr"], "source_item_page": f"https://archive.org/details/{s['item']}",
                     "segment_start_s": s["start"], "segment_s": s["dur"], "lufs_i": res["lufs_i"],
                     "true_peak_dbtp": res["true_peak_dbtp"], "sha256_download": sha256(s["_path"])})
        print(f"{final.name}: {info.duration:.1f} s, {res['lufs_i']:.1f} LUFS, TP {res['true_peak_dbtp']:.1f}")
        seg.unlink()

    # 3. 10-minute sampler
    by_id = {s["id"]: s for s in SOURCES}
    fade = int(SAMPLER_FADE_S * FS)
    pieces, seg_rows, t = [], [], 0.0
    for cid, off in SAMPLER:
        f = out / f"{by_id[cid]['cat']}__{cid}.wav"
        x, fs = sf.read(str(f), dtype="float64", always_2d=True)
        assert fs == FS
        a = int(off * FS)
        x = x[a:a + SAMPLER_SEG_S * FS].copy()
        assert len(x) == SAMPLER_SEG_S * FS, f"{cid}: clip too short for offset {off}"
        ramp = np.linspace(0, 1, fade)[:, None]
        x[:fade] *= ramp
        x[-fade:] *= ramp[::-1]
        pieces.append(x)
        seg_rows.append({"start_s": t, "end_s": t + SAMPLER_SEG_S, "category": by_id[cid]["cat"], "clip": f.name,
                         "clip_offset_s": off})
        t += SAMPLER_SEG_S
    raw = work / "sampler_raw.wav"
    sf.write(str(raw), np.concatenate(pieces), FS, subtype="FLOAT")
    final = out / "sampler_10min.wav"
    res = normalize(raw, final)
    raw.unlink()
    info = sf.info(str(final))
    rows.append({"file": final.name, "category": "sampler", "duration_s": round(info.duration, 2),
                 "source_url": "derived from the clips above (see sampler_segments.csv)",
                 "license": "CC BY 3.0 / public domain (as the clips)", "license_url": "", "title": "10-minute mixed sampler",
                 "attribution": "see the clips", "source_item_page": "", "segment_start_s": "", "segment_s": "",
                 "lufs_i": res["lufs_i"], "true_peak_dbtp": res["true_peak_dbtp"], "sha256_download": ""})
    print(f"{final.name}: {info.duration:.1f} s, {res['lufs_i']:.1f} LUFS, TP {res['true_peak_dbtp']:.1f}")
    with open(out / "sampler_segments.csv", "w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=list(seg_rows[0]))
        w.writeheader()
        w.writerows(seg_rows)
    with open(out / "manifest.csv", "w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=COLUMNS)
        w.writeheader()
        w.writerows(rows)
    try:
        work.rmdir()
    except OSError:
        pass
    clips = [r for r in rows if r["category"] != "sampler"]
    tot = {}
    for r in clips:
        tot[r["category"]] = tot.get(r["category"], 0) + r["duration_s"]
    print("category totals (s):", {k: round(v, 1) for k, v in tot.items()},
          "| clips total min:", round(sum(tot.values()) / 60, 1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
