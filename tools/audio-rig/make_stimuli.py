#!/usr/bin/env python3
"""Generate acoustic test stimuli (WAV) and a manifest for the audio rig.

Outputs (default: artifacts/audio-rig/stimuli/, which is gitignored):
  sweep_<rate>.wav / sweep_inverse_<rate>.wav   exponential sine sweep + matched inverse filter
  pilots_<rate>.wav                             500/1000/2000/3000 Hz pilot tones
  noise_<rate>.wav                              band-limited pink noise burst
  speech_*_<rate>.wav                           TTS: wake word "alexa" + Spanish phrases
  sequence_<rate>.wav                           all of the above in one file (for one TV recording)
  manifest.json                                 order, offsets, durations, TTS engine used
rate is 16k or 48k. Stimuli are written at -6 dBFS peak; control loudness at playback time.
"""
import argparse
import datetime
import json
import shutil
import subprocess
import sys
import tempfile
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import fftconvolve, resample_poly

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = ROOT / "artifacts" / "audio-rig" / "stimuli"
DEFAULT_VOICES = ROOT / "artifacts" / "audio-rig" / "voices"
RATES = {"16k": 16000, "48k": 48000}
PEAK = 0.5  # -6 dBFS
PILOT_TONES = [500, 1000, 2000, 3000]
PILOT_DUR, PILOT_GAP = 0.5, 0.1
WAKE_TEXT = "alexa"
PHRASES = ["enciende la luz del salón", "qué hora es", "apaga la luz de la cocina"]
WAKE_VOICE = "en_US-lessac-medium"
ES_VOICE = "es_ES-sharvard-medium"


def resample(x, fs_in, fs_out):
    if fs_in == fs_out:
        return x.astype(np.float64)
    g = gcd(fs_in, fs_out)
    return resample_poly(x, fs_out // g, fs_in // g)


def ramp(x, fs, ms=20):
    n = min(int(fs * ms / 1000), len(x) // 2)
    w = 0.5 - 0.5 * np.cos(np.pi * np.arange(n) / n)
    x = x.copy()
    x[:n] *= w
    x[len(x) - n:] *= w[::-1]
    return x


def normalize_peak(x, peak=PEAK):
    return x * (peak / np.max(np.abs(x)))


def make_sweep(fs, dur=3.0, f1=100.0, f2_req=8000.0):
    f2 = min(f2_req, 0.95 * fs / 2)
    t = np.arange(int(dur * fs)) / fs
    k = np.log(f2 / f1)
    x = np.sin(2 * np.pi * f1 * dur / k * (np.exp(t * k / dur) - 1))
    x = ramp(x, fs, 20) * PEAK
    # Farina inverse filter: time reversal with -6 dB/octave amplitude correction
    inv = x[::-1] * np.exp(-t * k / dur)
    conv = fftconvolve(x, inv)
    inv = inv / np.max(np.abs(conv))  # unit-peak impulse after convolution
    conv = fftconvolve(x, inv)
    pk = int(np.argmax(np.abs(conv)))
    side = np.delete(np.abs(conv), np.arange(max(pk - int(0.01 * fs), 0), pk + int(0.01 * fs)))
    check = {"conv_peak_index": pk, "expected_index": len(x) - 1,
             "peak_to_max_sidelobe_db": float(20 * np.log10(1.0 / side.max()))}
    return x, inv, {"f1": f1, "f2": f2, "duration_s": dur, "inverse_check": check}


def make_pilots(fs):
    parts = []
    for i, f in enumerate(PILOT_TONES):
        t = np.arange(int(PILOT_DUR * fs)) / fs
        parts.append(ramp(np.sin(2 * np.pi * f * t), fs, 10))
        if i < len(PILOT_TONES) - 1:
            parts.append(np.zeros(int(PILOT_GAP * fs)))
    return np.concatenate(parts) * PEAK


def pink_noise_48k(dur=1.0, seed=20240601):
    fs = 48000
    rng = np.random.default_rng(seed)
    n = int(dur * fs)
    spec = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1 / fs)
    spec[1:] /= np.sqrt(f[1:])
    spec[(f < 100) | (f > 7500)] = 0  # identical band at every rate
    x = np.fft.irfft(spec, n)
    return normalize_peak(ramp(x, fs, 10))


# ---------------------------------------------------------------- TTS

def tts_piper(text, voice_name, voices_dir):
    from piper import PiperVoice
    model = voices_dir / f"{voice_name}.onnx"
    if not model.exists():
        raise FileNotFoundError(f"{model} (run: python -m piper.download_voices --download-dir {voices_dir} {voice_name})")
    voice = PiperVoice.load(str(model))
    chunks = [c.audio_float_array for c in voice.synthesize(text)]
    return np.concatenate(chunks).astype(np.float64), voice.config.sample_rate


def tts_espeak(text, lang):
    exe = shutil.which("espeak-ng")
    if not exe:
        raise RuntimeError("espeak-ng not found")
    with tempfile.TemporaryDirectory() as d:
        out = Path(d) / "t.wav"
        subprocess.run([exe, "-v", lang, "-s", "150", "-w", str(out), text], check=True)
        x, fs = sf.read(out, dtype="float64")
    return x, fs


def synth(text, lang, engine, voices_dir):
    if engine == "piper":
        return tts_piper(text, WAKE_VOICE if lang == "en" else ES_VOICE, voices_dir)
    return tts_espeak(text, "en-us" if lang == "en" else "es")


def trim_silence(x, fs, thr_db=-45, pad_ms=60):
    env = np.abs(x)
    idx = np.where(env > env.max() * 10 ** (thr_db / 20))[0]
    pad = int(fs * pad_ms / 1000)
    return x[max(idx[0] - pad, 0): idx[-1] + pad]


def build_speech(engine, voices_dir):
    fs = 48000
    segs = {}
    for name, text, lang in [("wake", WAKE_TEXT, "en")] + [(f"es{i+1}", p, "es") for i, p in enumerate(PHRASES)]:
        x, r = synth(text, lang, engine, voices_dir)
        x = trim_silence(resample(x, r, fs), fs)
        segs[name] = (text, lang, ramp(normalize_peak(x), fs, 15))
    return segs


def gap(sec, fs):
    return np.zeros(int(sec * fs))


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--voices-dir", type=Path, default=DEFAULT_VOICES)
    ap.add_argument("--tts", choices=["auto", "piper", "espeak-ng"], default="auto")
    args = ap.parse_args()

    out = args.out_dir.resolve()
    try:
        rel = out.relative_to(ROOT)
        if subprocess.run(["git", "-C", str(ROOT), "check-ignore", "-q", str(rel / "x.wav")]).returncode != 0:
            sys.exit(f"refusing to write: {rel} is not gitignored")
    except ValueError:
        pass  # outside the repository
    out.mkdir(parents=True, exist_ok=True)

    engine = args.tts
    speech = None
    if engine in ("auto", "piper"):
        try:
            speech = build_speech("piper", args.voices_dir)
            engine = "piper"
        except Exception as exc:  # noqa: BLE001
            if engine == "piper":
                raise
            print(f"piper unavailable ({exc}); falling back to espeak-ng", file=sys.stderr)
    if speech is None:
        speech = build_speech("espeak-ng", args.voices_dir)
        engine = "espeak-ng"
    tts_info = {"engine": engine,
                "voices": {"wake": WAKE_VOICE, "es": ES_VOICE} if engine == "piper"
                else {"wake": "espeak-ng en-us", "es": "espeak-ng es"},
                "note": "HA pipeline uses es_ES-sharvard-medium; alexa is spoken with an English voice"}

    manifest = {"generated_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
                "tts": tts_info, "peak_dbfs": round(20 * np.log10(PEAK), 1), "rates": {}}

    for label, fs in RATES.items():
        stim = {}
        sweep, inv, sp = make_sweep(fs)
        stim["sweep"] = {"kind": "sweep", "x": sweep, "params": sp}
        stim["pilots"] = {"kind": "pilots", "x": make_pilots(fs),
                          "params": {"tones_hz": PILOT_TONES, "tone_s": PILOT_DUR, "gap_s": PILOT_GAP}}
        stim["noise"] = {"kind": "noise", "x": resample(pink_noise_48k(), 48000, fs) * 1.0,
                         "params": {"type": "pink", "band_hz": [100, 7500], "seed": 20240601}}
        speech_fs = {k: (t, l, resample(x, 48000, fs)) for k, (t, l, x) in speech.items()}
        for k, (t, l, x) in speech_fs.items():
            stim[f"speech_{k}"] = {"kind": "speech", "x": x, "params": {"text": t, "lang": l}}
        # full utterance: wake word then the three phrases
        parts, segs, pos = [], [], 0
        for k, g in [("wake", 1.2), ("es1", 1.5), ("es2", 1.5), ("es3", 0.0)]:
            x = speech_fs[k][2]
            segs.append({"name": k, "text": speech_fs[k][0], "start_s": round(pos / fs, 3),
                         "end_s": round((pos + len(x)) / fs, 3)})
            parts += [x, gap(g, fs)]
            pos += len(x) + int(g * fs)
        stim["speech_full"] = {"kind": "speech", "x": np.concatenate(parts), "params": {"segments": segs}}

        # combined sequence
        order = ["sweep", "pilots", "noise", "speech_full"]
        seq, items, pos = [], [], 0
        seq.append(gap(0.5, fs)); pos = int(0.5 * fs)
        for k in order:
            x = stim[k]["x"]
            items.append({"name": k, "start_s": round(pos / fs, 3), "end_s": round((pos + len(x)) / fs, 3)})
            seq += [x, gap(1.5, fs)]
            pos += len(x) + int(1.5 * fs)
        seqx = np.concatenate(seq)

        files = {}
        for k, s in stim.items():
            fn = f"{k}_{label}.wav"
            sf.write(out / fn, s["x"], fs, subtype="PCM_16")
            files[k] = {"file": fn, "kind": s["kind"], "duration_s": round(len(s["x"]) / fs, 3),
                        "rms_dbfs": round(float(20 * np.log10(np.sqrt(np.mean(s["x"] ** 2)))), 1),
                        "params": s["params"]}
        sf.write(out / f"sweep_inverse_{label}.wav", inv, fs, subtype="FLOAT")
        files["sweep"]["inverse_file"] = f"sweep_inverse_{label}.wav"
        sf.write(out / f"sequence_{label}.wav", seqx, fs, subtype="PCM_16")
        manifest["rates"][label] = {
            "sample_rate": fs, "stimuli": files,
            "sequence": {"file": f"sequence_{label}.wav", "duration_s": round(len(seqx) / fs, 3),
                         "order": items}}
        print(f"{label}: wrote {len(files) + 2} files; sweep inverse check: {sp['inverse_check']}")

    (out / "manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    print(f"TTS engine: {engine}\nmanifest: {out / 'manifest.json'}")


if __name__ == "__main__":
    main()
