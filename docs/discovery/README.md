# Discovery logs

Device discovery results, one directory per TV. Each log states the date, firmware, TV state and a confidence label for every finding (`verified`, `inferred`, `hypothesis`, `unknown`). Findings are scoped to one model, firmware and power state; they are not confirmation for other devices.

## Index

### c3-a (LG C3, webOS TV 8.3.0, firmware 03.30.74)

| Log | Content |
| --- | --- |
| [D0-D1-runtime.md](c3-a/D0-D1-runtime.md) | Baseline and runtime: firmware, root method, mounts, 32-bit ARM soft-float glibc 2.31 userland, init system |
| [D2-audio-stack.md](c3-a/D2-audio-stack.md) | Audio stack and LG voice services inventory, including the kernel oops incident |
| [D3-mic-identification.md](c3-a/D3-mic-identification.md) | Built-in mic identification (`hw:LGSIC,0`), capture characterization, offline Home Assistant (Wyoming) check |
| [rooted-tv-improvements.md](c3-a/rooted-tv-improvements.md) | Hardening and hygiene backlog for the rooted TV (documented only, not applied) |

Not yet done: D4 (behaviour while the TV plays sound, echo, SpeakerFeedback as reference) and D5 (power states). They are scheduled in [roadmap](../project/roadmap.md) Phases 2 and 3. The D3 log's section "Step 5b: Hands-free" still says PENDING although the hands-free PCM ownership is recorded in the capture section of the same log; treat the capture section as the newer record until the log is tidied.

## Naming convention

- In committed documents a TV is named by an anonymous label: `c3-a` is the first LG C3, the second C3 is proposed to be `c3-b`. No serial numbers, MAC addresses, IP addresses, account names or network names appear in committed text.
- Raw private output (probe dumps, logs, analysis plots, recordings) lives under `devices/private/c3-a/` and subfolders such as `D0-D1/` and `D3/`. This path is gitignored and **never committed**.
- Redacted conclusions go in these logs and in [compatibility](../compatibility/README.md).

## Safety rules learned

- **Never read ASoC DAPM widget sysfs nodes** (`dapm_widget` under `soc-audio` or `lgsic-audio`), nor `/sys/kernel/debug/asoc/*`. Reading them caused two kernel oopses on c3-a (NULL pointer in `mutex_lock`); the TV stayed up but the kernel went into a tainted post-oops state until the owner rebooted it. See [D2 incident](c3-a/D2-audio-stack.md#incident-during-this-step-read-first) and [improvement item 4](c3-a/rooted-tv-improvements.md) (a probe denylist is proposed, not yet applied).
- Wrap TV commands in `timeout`. Run read-only observations first.
- **Start sound tests low.** Raise playback levels in steps and only at the owner-approved reference level (+12 dB gain on the JBL Go 3 at system volume 82 %, as used in the D3 tests).
- **No TV playback without owner consent**, including test tones, replies and TTS through the TV's own audio. Announce the test to the room beforehand.
- Recordings are opt-in, short and local, with consent from anyone whose voice may be captured. Delete them from the TV and the host after analysis. Never commit audio.
- LG hands-free must stay off while the satellite uses `hw:LGSIC,0` ([ADR 0004](../decisions/0004-mvp-capture-device-and-lg-handsfree-off.md)). Do not change native LG voice services.
- Reboots, service changes, mounts and persistence need explicit authorization. `/tmp` content is lost at reboot, which makes it the preferred place for experiment binaries.
- Never execute unreviewed binaries or remote install pipes on the TV; inspect and checksum first.
- `luna-send` printed nothing unless stdout was a pipe and `-w <ms>` was given; silence does not prove "unsupported" ([improvement item 6](c3-a/rooted-tv-improvements.md)).
