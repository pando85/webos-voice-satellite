#!/usr/bin/env python3
"""Host audio discovery for the acoustic test rig (PipeWire/PulseAudio via pactl).

Read-only: lists sinks and sources, classifies them and names the speaker
sink, the webcam microphone and the USB microphone if they can be identified.
It never changes default devices, volumes or card profiles.

Usage:
    discover.py            human-readable report
    discover.py --json     machine-readable report
"""
import argparse
import glob
import json
import re
import subprocess
import sys


def pactl_json(kind):
    out = subprocess.run(["pactl", "--format=json", "list", kind],
                         check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def parse_spec(spec):
    """'s16le 2ch 48000Hz' -> dict(format, channels, rate)."""
    m = re.match(r"(\S+)\s+(\d+)ch\s+(\d+)Hz", spec)
    if not m:
        return {"format": "s16le", "channels": 2, "rate": 48000}
    return {"format": m.group(1), "channels": int(m.group(2)), "rate": int(m.group(3))}


def hdmi_monitors():
    """Names of displays that currently report an ELD (HDMI/DP sink info)."""
    names = []
    for path in glob.glob("/proc/asound/card*/eld*"):
        try:
            text = open(path).read()
        except OSError:
            continue
        if re.search(r"^monitor_present\s+1", text, re.M):
            m = re.search(r"^monitor_name\s+(.*)$", text, re.M)
            sp = re.search(r"^speakers\s+(.*)$", text, re.M)
            names.append({"monitor_name": m.group(1).strip() if m else "?",
                          "eld_speakers": sp.group(1).strip() if sp else "?"})
    return names


def classify_sink(s):
    props = s.get("properties", {})
    name = s["name"]
    ports = s.get("ports", [])
    active = s.get("active_port")
    active_port = next((p for p in ports if p["name"] == active), {})
    ptype = (active_port.get("type") or "").lower()
    pname = (active_port.get("name") or "").lower()
    low = name.lower()
    if "hdmi" in low or "hdmi" in pname or ptype in ("hdmi", "displayport"):
        kind = "hdmi"
    elif "bluez" in low:
        kind = "bluetooth"
    elif props.get("device.bus") == "usb":
        kind = "usb"
    elif "iec958" in low:
        kind = "spdif"
    else:
        kind = "analog"
    avail = active_port.get("availability group") or ""
    port_ok = active_port.get("availability", "unknown") != "not available"
    speaker = False
    if kind == "analog" and port_ok:
        if "speaker" in pname or ptype == "speaker" or "lineout" in pname or ptype == "line":
            speaker = True
        elif "headphone" in pname or ptype == "headphones":
            speaker = False
        elif not ports:
            speaker = True
    if kind == "bluetooth":
        speaker = True  # wireless speaker or headset: user must confirm
    return {"name": name, "description": s.get("description", ""), "kind": kind,
            "state": s.get("state"), "active_port": active, "port_type": ptype,
            "volume_percent": next(iter(s.get("volume", {}).values()), {}).get("value_percent"),
            "mute": s.get("mute"), "monitor_source": s.get("monitor_source"),
            "speaker_candidate": speaker, **parse_spec(s["sample_specification"])}


def classify_source(s):
    props = s.get("properties", {})
    text = " ".join([s["name"], s.get("description", ""),
                     props.get("device.product.name", ""),
                     props.get("alsa.card_name", "")]).lower()
    if s.get("monitor_of_sink") is not None and s.get("monitor_of_sink") != 4294967295 \
            or s["name"].endswith(".monitor"):
        kind = "monitor"
    elif re.search(r"webcam|camera|\bc9\d\d\b|uvc", text):
        kind = "webcam"
    elif props.get("device.bus") == "usb":
        kind = "usb-mic"
    else:
        kind = "other"
    return {"name": s["name"], "description": s.get("description", ""), "kind": kind,
            "state": s.get("state"), "mute": s.get("mute"),
            "volume_percent": next(iter(s.get("volume", {}).values()), {}).get("value_percent"),
            **parse_spec(s["sample_specification"])}


def discover():
    sinks = [classify_sink(s) for s in pactl_json("sinks")]
    sources = [classify_source(s) for s in pactl_json("sources")]
    speakers = [s for s in sinks if s["speaker_candidate"]]
    report = {
        "sinks": sinks,
        "sources": sources,
        "speaker_sink": speakers[0]["name"] if speakers else None,
        "webcam_source": next((s["name"] for s in sources if s["kind"] == "webcam"), None),
        "usb_mic_source": next((s["name"] for s in sources if s["kind"] == "usb-mic"), None),
        "hdmi_monitors": hdmi_monitors(),
    }
    if not speakers:
        report["speaker_warning"] = (
            "No obvious speaker output (analog speaker/line-out or Bluetooth). "
            "Sinks present: " + ", ".join(f"{s['name']} [{s['kind']}]" for s in sinks)
            + ". Do not play anything until the user confirms a sink.")
    return report


def resolve(alias, devices, kinds=None):
    """Resolve an alias ('speaker', 'webcam', 'usb') or a (sub)string to a device name."""
    names = [d["name"] for d in devices]
    if alias in names:
        return alias
    kind_alias = {"webcam": "webcam", "usb": "usb-mic", "usbmic": "usb-mic", "usb-mic": "usb-mic",
                  "monitor": "monitor"}
    if alias in kind_alias:
        hits = [d["name"] for d in devices if d["kind"] == kind_alias[alias]]
    elif alias == "speaker":
        hits = [d["name"] for d in devices if d.get("speaker_candidate")]
    else:
        hits = [n for n in names if alias.lower() in n.lower()]
    if len(hits) != 1:
        raise SystemExit(f"cannot resolve device {alias!r} uniquely (matches: {hits or 'none'})")
    return hits[0]


def print_report(r):
    print("Sinks (playback):")
    for s in r["sinks"]:
        print(f"  [{s['kind']:9}] {s['name']}\n      {s['description']} | {s['format']} {s['channels']}ch "
              f"{s['rate']} Hz | vol {s['volume_percent']} mute={s['mute']} state={s['state']} "
              f"port={s['active_port']} speaker_candidate={s['speaker_candidate']}")
    print("Sources (capture):")
    for s in r["sources"]:
        print(f"  [{s['kind']:9}] {s['name']}\n      {s['description']} | {s['format']} {s['channels']}ch "
              f"{s['rate']} Hz | vol {s['volume_percent']} mute={s['mute']} state={s['state']}")
    if r["hdmi_monitors"]:
        print("Displays reporting HDMI/DP audio info:", r["hdmi_monitors"])
    print("speaker sink   :", r["speaker_sink"])
    print("webcam source  :", r["webcam_source"])
    print("usb mic source :", r["usb_mic_source"])
    if r.get("speaker_warning"):
        print("WARNING:", r["speaker_warning"])


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    r = discover()
    print(json.dumps(r, indent=2) if args.json else "", end="" if not args.json else "\n")
    if not args.json:
        print_report(r)
    return 0 if r["speaker_sink"] else 2


if __name__ == "__main__":
    sys.exit(main())
