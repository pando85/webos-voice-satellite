#!/usr/bin/env python3
"""Generate the Phase 2 wake-word / STT test corpus (synthetic Piper voices only).

Output (default devices/private/corpus/p2-v1/, gitignored; never commit audio):
  items/48k_stereo/<id>.wav   16-bit PCM, 48 kHz, 2 ch (dual mono), equal active RMS, peak <= -6 dBFS
  items/16k_mono/<id>.wav     16-bit PCM, 16 kHz, 1 ch,           same scaling
  manifest.csv                one row per item (see COLUMNS)
  session_<kind>_48k_stereo.wav / session_<kind>_16k_mono.wav   one long file per kind (pos, neg, nearmiss)
  session_<kind>_timestamps.csv                                  item and wake-word times in the session
  corpus.json                 generator settings, voices, counts, durations, sha256 of manifest.csv

Level contract: play_record.py rescales a stimulus so that its PEAK is -30 dBFS + gain_db, so the
absolute scale of a file does not matter, but the PEAK of the file does. Every item is scaled so that its
active-speech RMS is TARGET_RMS_DBFS (equal loudness) unless that would push its peak above PEAK_DBFS
(-6 dBFS, same ceiling as make_stimuli.py); the few high-crest items stay at the ceiling and are a little
quieter. Session files therefore peak at -6 dBFS, so +12 dB playback gain gives -18 dBFS peak for the
loudest item and about -18 to -22 dBFS for the others. Active-speech RMS spread is reported.

Items are padded to a multiple of 3 samples at 48 kHz, so the 16 kHz variants are exactly 1/3 as long
and every timestamp is identical in both variants.

Piper noise sampling is not seedable: regenerating gives an equivalent but not bit-identical corpus.
Identify a corpus by corpus.json["manifest_sha256"] and keep the generated directory for comparisons.

Voices come from the official rhasspy/piper-voices release (Hugging Face), downloaded with
`python -m piper.download_voices`. See tools/audio-rig/README.md.
"""
import argparse
import csv
import datetime
import hashlib
import json
import sys
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = ROOT / "devices" / "private" / "corpus" / "p2-v1"
DEFAULT_VOICES = ROOT / "artifacts" / "audio-rig" / "voices"

PEAK_DBFS = -6.0        # ceiling for any sample of any item
TARGET_RMS_DBFS = -22.0  # active-speech RMS every item is scaled to, within the ceiling
LEAD_PAD_S = 0.30       # silence at the start of every item
TAIL_PAD_S = 0.50       # silence at the end of every item
WAKE_CMD_GAP_S = 0.40   # silence between the end of "alexa" and the start of the command
SESSION_HEAD_S = 1.0    # silence before the first item of a session file
SESSION_GAP_S = 4.0     # silence between consecutive items of a session file (on top of item padding)
TRIM_DB = -45.0         # TTS leading/trailing silence is trimmed below this level (re peak)
WAKE_TEXT = "alexa"

# (voice model, speaker_id or None, speaker label, language, gender, short code)
# Gender is the speaker's apparent gender (sharvard: M/F speakers in the model; the others judged
# from the median f0 of the generated audio: >165 Hz female, see corpus.json), not a claim about the data donors.
VOICES = [
    ("es_ES-sharvard-medium", 0, "M", "es", "male", "sharM"),
    ("es_ES-sharvard-medium", 1, "F", "es", "female", "sharF"),
    ("es_ES-davefx-medium", None, "davefx", "es", "male", "davefx"),
    ("es_ES-mls_9972-low", None, "mls_9972", "es", "female", "mls9972"),
    ("es_ES-mls_10246-low", None, "mls_10246", "es", "female", "mls10246"),
    ("es_MX-ald-medium", None, "ald", "es", "male", "ald"),
    ("es_MX-claude-high", None, "claude", "es", "female", "claude"),
    ("en_US-lessac-medium", None, "lessac", "en", "female", "lessac"),
    ("en_US-ryan-medium", None, "ryan", "en", "male", "ryan"),
]
# (length_scale, noise_scale, noise_w_scale) variations for "alexa" alone; Piper defaults first.
WAKE_VARIANTS = [(1.0, 0.667, 0.8), (0.85, 0.8, 0.9), (1.2, 0.4, 0.6)]

# (text, intent class). Classes are generic labels; the Phase 2 run maps them to the expected HA outcome.
COMMANDS = [
    ("enciende la luz del salón", "light_on"),
    ("apaga la luz de la cocina", "light_off"),
    ("¿qué hora es?", "time_query"),
    ("pon un temporizador de cinco minutos", "timer_start"),
    ("¿qué temperatura hay en el salón?", "temperature_query"),
    ("¿qué tiempo hace hoy?", "weather_query"),
    ("sube la temperatura del termostato a veintiún grados", "climate_set"),
    ("apaga todas las luces", "light_off_all"),
    ("cancela el temporizador", "timer_cancel"),
    ("¿va a llover mañana?", "weather_query"),
    ("enciende la luz del dormitorio", "light_on"),
    ("baja la luz del salón al cincuenta por ciento", "light_set"),
    ("¿cuánto falta para que termine el temporizador?", "timer_status"),
    ("pon un temporizador de diez minutos", "timer_start"),
    ("¿qué temperatura hay fuera?", "temperature_query"),
]
CMDS_PER_SPEAKER = 10

NEGATIVES = [
    "Hoy hemos comido en casa de mis padres.",
    "¿Has visto dónde dejé las llaves?",
    "Mañana tengo que ir al médico por la mañana.",
    "No me acuerdo de lo que dijo el profesor.",
    "Vamos a cenar pronto porque los niños tienen cole.",
    "El partido empieza a las nueve y media.",
    "Me encanta el olor del café recién hecho.",
    "Creo que va a hacer mucho calor este fin de semana.",
    "Tenemos que comprar pan y leche antes de volver.",
    "La película de anoche estuvo bastante bien.",
    "Dile a tu hermana que me llame cuando pueda.",
    "Ese restaurante nuevo está en la calle de la iglesia.",
    "Pásame el mando de la tele, por favor.",
    "Qué lástima que no pudieras venir a la fiesta.",
    "El domingo vamos a la playa si no llueve.",
    "Estaba pensando en cambiar el sofá del salón.",
    "¿Quieres un poco más de ensalada?",
    "Mi vecino ha pintado la puerta de verde.",
    "Hay que sacar la basura antes de las diez.",
    "Me duele un poco la cabeza desde esta mañana.",
    "El tren llegará con diez minutos de retraso.",
    "Lo siento, no he oído lo que me has dicho.",
    "Voy a darme una ducha y luego salimos.",
    "Han anunciado lluvia para el jueves en el norte.",
    # commands without the wake word: a satellite must ignore these
    "enciende la luz del salón",
    "¿qué hora es?",
    "pon un temporizador de cinco minutos",
    "apaga la luz de la cocina",
]
NEARMISSES = [
    "Alexis",
    "a la mesa",
    "relaja",
    "Alexandria",
    "Alejandra, ven un momento.",
    "Alejandro, enciende la luz.",
    "Alex, ¿qué hora es?",
    "A la mesa, que se enfría la comida.",
    "Relájate un poco.",
    "La alejada",
    "Alexia no ha llegado todavía.",
    "Alexander ha ganado el torneo.",
    "Alerta roja en la costa.",
    "Oye, Alexis, pon un temporizador de cinco minutos.",
]

COLUMNS = ["id", "voice", "speaker", "kind", "text", "expected_wake", "expected_command_text", "duration_ms",
           "peak_dbfs", "subkind", "lang", "gender_est", "expected_intent", "length_scale", "noise_scale",
           "noise_w_scale", "wake_start_ms", "wake_end_ms", "command_start_ms", "active_rms_dbfs",
           "file_48k_stereo", "file_16k_mono", "sha256_48k_stereo"]

def resample(x, fs_in, fs_out):
    if fs_in == fs_out:
        return np.asarray(x, dtype=np.float64)
    g = gcd(fs_in, fs_out)
    return resample_poly(np.asarray(x, dtype=np.float64), fs_out // g, fs_in // g)

def trim_silence(x, db=TRIM_DB):
    thr = np.max(np.abs(x)) * 10 ** (db / 20)
    idx = np.nonzero(np.abs(x) > thr)[0]
    return x[idx[0]:idx[-1] + 1] if len(idx) else x

def loudness_normalize(x, fs, target=TARGET_RMS_DBFS, ceiling=PEAK_DBFS):
    """Scale x to the target active RMS, but never above the peak ceiling."""
    gain_db = min(target - active_rms_dbfs(x, fs), ceiling - dbfs(np.max(np.abs(x))))
    return x * 10 ** (gain_db / 20)

def dbfs(v):
    return 20 * np.log10(max(float(v), 1e-12))

def active_rms_dbfs(x, fs):
    """RMS over 20 ms frames within 25 dB of the loudest frame (speech-active frames)."""
    n = int(0.02 * fs)
    frames = x[:len(x) // n * n].reshape(-1, n)
    rms = np.sqrt(np.mean(frames ** 2, axis=1))
    act = rms[rms > rms.max() * 10 ** (-25 / 20)]
    return dbfs(np.sqrt(np.mean(act ** 2)))

def median_f0(x, fs):
    """Rough median f0 (Hz) by autocorrelation over voiced 40 ms frames; only used to label gender."""
    n = int(0.04 * fs)
    f0s = []
    lo, hi = int(fs / 400), int(fs / 70)
    thr = np.max(np.abs(x)) * 0.1
    for i in range(0, len(x) - n, n // 2):
        f = x[i:i + n] - np.mean(x[i:i + n])
        if np.max(np.abs(f)) < thr:
            continue
        ac = np.correlate(f, f, "full")[n - 1:]
        if ac[0] <= 0:
            continue
        k = lo + int(np.argmax(ac[lo:hi]))
        if ac[k] / ac[0] > 0.5:
            f0s.append(fs / k)
    return float(np.median(f0s)) if f0s else float("nan")

class Synth:
    def __init__(self, voices_dir):
        from piper import PiperVoice, SynthesisConfig
        self.PiperVoice, self.SynthesisConfig = PiperVoice, SynthesisConfig
        self.voices_dir = Path(voices_dir)
        self.loaded = {}

    def say(self, voice, speaker_id, text, ls, ns, nw):
        if voice not in self.loaded:
            model = self.voices_dir / f"{voice}.onnx"
            if not model.exists():
                sys.exit(f"missing voice {model}; run: python -m piper.download_voices "
                         f"--download-dir {self.voices_dir} {voice}")
            self.loaded[voice] = self.PiperVoice.load(str(model))
        v = self.loaded[voice]
        cfg = self.SynthesisConfig(speaker_id=speaker_id, length_scale=ls, noise_scale=ns, noise_w_scale=nw)
        parts, fs = [], v.config.sample_rate
        for ch in v.synthesize(text, syn_config=cfg):
            parts.append(ch.audio_float_array)
        x = np.concatenate(parts).astype(np.float64)
        x = trim_silence(resample(x, fs, 48000))  # speech at 48 kHz, silence trimmed
        return x

def build_item(speech_parts, gap_s):
    """speech_parts: list of 48 kHz arrays joined by gap_s of silence. Returns audio and offsets (samples)."""
    fs = 48000
    pos = int(LEAD_PAD_S * fs)
    bufs = [np.zeros(pos)]
    spans = []
    for i, p in enumerate(speech_parts):
        if i:
            g = int(round(gap_s * fs))
            bufs.append(np.zeros(g))
            pos += g
        bufs.append(p)
        spans.append((pos, pos + len(p)))
        pos += len(p)
    bufs.append(np.zeros(int(TAIL_PAD_S * fs)))
    x = np.concatenate(bufs)
    x = np.concatenate([x, np.zeros((-len(x)) % 3)])  # multiple of 3 samples -> exact 16 kHz length
    return x, spans

def rows_for_items(synth):
    """Yield dicts with 'audio48' (mono float, not yet normalized) and the metadata."""
    es = [v for v in VOICES if v[3] == "es"]
    en = [v for v in VOICES if v[3] == "en"]
    items = []
    # 1. "alexa" alone, per voice, 3 length_scale/noise variations
    for voice, spk, label, lang, gender, code in VOICES:
        for k, (ls, ns, nw) in enumerate(WAKE_VARIANTS):
            w = synth.say(voice, spk, WAKE_TEXT, ls, ns, nw)
            x, sp = build_item([w], 0)
            items.append(dict(id=f"pos_alone_{code}_v{k}", voice=voice, speaker=label, kind="pos", subkind="alone",
                              text=WAKE_TEXT, expected_wake=True, expected_command_text="", lang=lang,
                              gender_est=gender, expected_intent="", ls=ls, ns=ns, nw=nw, audio=x,
                              wake=sp[0], command=None))
    # 2. "alexa" + 0.4 s + Spanish command; each Spanish speaker gets CMDS_PER_SPEAKER rotating commands
    for si, (voice, spk, label, lang, gender, code) in enumerate(es):
        for j in range(CMDS_PER_SPEAKER):
            text, intent = COMMANDS[(si * 2 + j) % len(COMMANDS)]
            ls, ns, nw = WAKE_VARIANTS[j % len(WAKE_VARIANTS)]
            w = synth.say(voice, spk, WAKE_TEXT, ls, ns, nw)
            c = synth.say(voice, spk, text, 1.0, 0.667, 0.8)
            x, sp = build_item([w, c], WAKE_CMD_GAP_S)
            items.append(dict(id=f"pos_cmd_{code}_{j:02d}", voice=voice, speaker=label, kind="pos", subkind="cmd",
                              text=f"{WAKE_TEXT} {text}", expected_wake=True, expected_command_text=text,
                              lang=lang, gender_est=gender, expected_intent=intent, ls=ls, ns=ns, nw=nw,
                              audio=x, wake=sp[0], command=sp[1]))
    # 3. negatives and near-misses, rotated over the Spanish speakers
    for kind, prefix, texts, off in (("neg", "neg", NEGATIVES, 0), ("nearmiss", "near", NEARMISSES, 3)):
        for i, text in enumerate(texts):
            voice, spk, label, lang, gender, code = es[(i + off) % len(es)]
            w = synth.say(voice, spk, text, 1.0, 0.667, 0.8)
            x, sp = build_item([w], 0)
            items.append(dict(id=f"{prefix}_{i:02d}_{code}", voice=voice, speaker=label, kind=kind, subkind="",
                              text=text, expected_wake=False, expected_command_text="", lang=lang,
                              gender_est=gender, expected_intent="", ls=1.0, ns=0.667, nw=0.8, audio=x,
                              wake=None, command=None))
    return items

def write_wav(path, mono, channels, fs):
    data = np.stack([mono] * channels, axis=1) if channels > 1 else mono
    sf.write(str(path), data, fs, subtype="PCM_16")

def sha256(path):
    h = hashlib.sha256()
    h.update(Path(path).read_bytes())
    return h.hexdigest()

def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--voices-dir", type=Path, default=DEFAULT_VOICES)
    ap.add_argument("--force", action="store_true", help="overwrite an existing output directory")
    args = ap.parse_args()
    out = args.out
    if out.exists() and any(out.iterdir()) and not args.force:
        sys.exit(f"{out} is not empty; pass --force to overwrite")
    (out / "items" / "48k_stereo").mkdir(parents=True, exist_ok=True)
    (out / "items" / "16k_mono").mkdir(parents=True, exist_ok=True)

    synth = Synth(args.voices_dir)
    items = rows_for_items(synth)

    # per-item normalization, files, manifest rows
    rows, audio48, audio16 = [], {}, {}
    f0 = {}
    for it in items:
        a48 = loudness_normalize(it["audio"], 48000)
        a16 = resample(a48, 48000, 16000)
        a16 = a16 * min(1.0, 10 ** (PEAK_DBFS / 20) / float(np.max(np.abs(a16))))
        assert len(a16) * 3 == len(a48)
        p48 = out / "items" / "48k_stereo" / f"{it['id']}.wav"
        p16 = out / "items" / "16k_mono" / f"{it['id']}.wav"
        write_wav(p48, a48, 2, 48000)
        write_wav(p16, a16, 1, 16000)
        # the manifest describes what is in the 16-bit files
        r48, _ = sf.read(str(p48), dtype="float64", always_2d=True)
        r16, _ = sf.read(str(p16), dtype="float64", always_2d=True)
        audio48[it["id"]], audio16[it["id"]] = r48[:, 0], r16[:, 0]
        ms = lambda s: "" if s is None else int(round(s * 1000 / 48000))
        rows.append({
            "id": it["id"], "voice": it["voice"], "speaker": it["speaker"], "kind": it["kind"], "text": it["text"],
            "expected_wake": str(it["expected_wake"]).lower(), "expected_command_text": it["expected_command_text"],
            "duration_ms": int(round(len(a48) * 1000 / 48000)), "peak_dbfs": round(dbfs(np.max(np.abs(r48))), 2),
            "subkind": it["subkind"], "lang": it["lang"], "gender_est": it["gender_est"],
            "expected_intent": it["expected_intent"], "length_scale": it["ls"], "noise_scale": it["ns"],
            "noise_w_scale": it["nw"],
            "wake_start_ms": ms(it["wake"][0]) if it["wake"] else "", "wake_end_ms": ms(it["wake"][1]) if it["wake"] else "",
            "command_start_ms": ms(it["command"][0]) if it["command"] else "",
            "active_rms_dbfs": round(active_rms_dbfs(r48[:, 0], 48000), 2),
            "file_48k_stereo": f"items/48k_stereo/{it['id']}.wav", "file_16k_mono": f"items/16k_mono/{it['id']}.wav",
            "sha256_48k_stereo": sha256(p48)})
        if it["lang"] == "es" and it["subkind"] == "":
            f0.setdefault(it["voice"] + f"/{it['speaker']}", []).append(median_f0(a48, 48000))
    with open(out / "manifest.csv", "w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=COLUMNS)
        w.writeheader()
        w.writerows(rows)

    # session files: head silence, then items separated by SESSION_GAP_S
    sessions = {}
    for kind in ("pos", "neg", "nearmiss"):
        sel = [r for r in rows if r["kind"] == kind]
        t = int(SESSION_HEAD_S * 48000)
        chunks48 = [np.zeros(t)]
        chunks16 = [np.zeros(t // 3)]
        ts = []
        for r in sel:
            a48, a16 = audio48[r["id"]], audio16[r["id"]]
            start = t
            chunks48.append(a48)
            chunks16.append(a16)
            t += len(a48)
            gap = int(SESSION_GAP_S * 48000)
            chunks48.append(np.zeros(gap))
            chunks16.append(np.zeros(gap // 3))
            t += gap
            sec = lambda ms_: "" if ms_ == "" else round(start / 48000 + ms_ / 1000, 3)
            ts.append({"id": r["id"], "kind": kind, "expected_wake": r["expected_wake"],
                       "item_start_s": round(start / 48000, 3), "item_end_s": round((start + len(a48)) / 48000, 3),
                       "wake_start_s": sec(r["wake_start_ms"]), "wake_end_s": sec(r["wake_end_ms"]),
                       "command_start_s": sec(r["command_start_ms"]),
                       "expected_command_text": r["expected_command_text"], "text": r["text"],
                       "voice": r["voice"], "speaker": r["speaker"]})
        s48, s16 = np.concatenate(chunks48), np.concatenate(chunks16)
        write_wav(out / f"session_{kind}_48k_stereo.wav", s48, 2, 48000)
        write_wav(out / f"session_{kind}_16k_mono.wav", s16, 1, 16000)
        with open(out / f"session_{kind}_timestamps.csv", "w", newline="", encoding="utf-8") as fh:
            w = csv.DictWriter(fh, fieldnames=list(ts[0]))
            w.writeheader()
            w.writerows(ts)
        sessions[kind] = {"items": len(sel), "duration_s": round(len(s48) / 48000, 2),
                          "peak_dbfs": round(dbfs(np.max(np.abs(s48))), 2)}

    # summary
    by = {}
    for r in rows:
        by.setdefault(r["kind"], []).append(r)
    rms = [r["active_rms_dbfs"] for r in rows]
    summary = {
        "corpus": "p2-v1", "generated_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "generator": "tools/audio-rig/make_corpus.py", "tts": "piper-tts (rhasspy/piper-voices)",
        "format": {"items_48k": "PCM_16, 48000 Hz, 2 ch (dual mono)", "items_16k": "PCM_16, 16000 Hz, 1 ch",
                   "peak_ceiling_dbfs": PEAK_DBFS, "target_active_rms_dbfs": TARGET_RMS_DBFS, "lead_pad_s": LEAD_PAD_S, "tail_pad_s": TAIL_PAD_S,
                   "wake_command_gap_s": WAKE_CMD_GAP_S, "session_head_s": SESSION_HEAD_S,
                   "session_gap_s": SESSION_GAP_S},
        "voices": [{"voice": v[0], "speaker_id": v[1], "speaker": v[2], "lang": v[3], "gender_est": v[4], "code": v[5]}
                   for v in VOICES],
        "counts": {k: len(v) for k, v in by.items()} | {"total": len(rows),
                   "pos_alone": sum(r["subkind"] == "alone" for r in rows),
                   "pos_cmd": sum(r["subkind"] == "cmd" for r in rows)},
        "item_audio_seconds": {k: round(sum(r["duration_ms"] for r in v) / 1000, 1) for k, v in by.items()},
        "sessions": sessions,
        "active_rms_dbfs": {"min": min(rms), "max": max(rms), "mean": round(float(np.mean(rms)), 2),
                            "std": round(float(np.std(rms)), 2)},
        "median_f0_hz_of_neg_items": {k: round(float(np.nanmedian(v)), 1) for k, v in f0.items()},
        "manifest_sha256": sha256(out / "manifest.csv")}
    (out / "corpus.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({k: summary[k] for k in ("counts", "item_audio_seconds", "sessions", "active_rms_dbfs",
                                              "median_f0_hz_of_neg_items", "manifest_sha256")}, indent=2))
    return 0

if __name__ == "__main__":
    sys.exit(main())
