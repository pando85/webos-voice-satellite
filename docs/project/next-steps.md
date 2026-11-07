# Next steps: what is still pending after 0.6.0 (2026-11-07 close-out)

- **Date:** 2026-11-07. **Status:** a hand-over list for the next session. It records what is open, who has to act, what must be true first and which document to follow. It decides nothing new and does not repeat the documents it links to.
- **Written without TV access:** nothing here was run. The 0.6.0 release records (install results on c3-a and c3-b, compatibility rows, the P4 logs) come from the release commit and are not copied here. Read them there.
- **Related:** [roadmap](roadmap.md), [backlog](backlog.md), [mid-term plan](mid-term-plan.md), [ADR 0016](../decisions/0016-remote-key-grab.md), [design: the input grab](../design/remote-mic-button.md#12-the-input-grab-adr-0016), [install permissions](../security/install-permissions.md), [HA-side audio processing](../research/ha-side-audio-processing.md).

Device rules from [AGENTS.md](../../AGENTS.md) still apply to every item: each run on a TV needs the owner's authorization, raw probe output and recordings are never committed, and the TV access policy is "install only" unless the owner says otherwise for a named experiment.

## Summary table

| # | Item | Who acts | Needs first | Follow |
| --- | --- | --- | --- | --- |
| 1 | 0.6.0 acceptance test of the mic-key grab on c3-a | owner, at the TV | 0.6.0 installed on c3-a, about 20 minutes, someone free to use the remote | [section 1](#1-060-owner-acceptance-test-on-c3-a), [ADR 0016](../decisions/0016-remote-key-grab.md#implementation-plan-if-accepted) |
| 2 | Grab on c3-b (firmware `03.30.73`) | owner runs, agent prepares the record | c3-b unmuted in HA for the run; EXP-026 Part B2 and B3 on c3-b | [section 2](#2-c3-b-is-not-on-the-grab-allow-list) |
| 3 | Does the TV's app installer reset the app directory to 0777? | agent, at the next app update | an app update the owner approves | [section 3](#3-security-follow-up-the-installer-and-the-app-directory-mode) |
| 4 | ADR 0015 (silent key filter) | nobody now | only if the grab cannot work on some firmware | [section 4](#4-adr-0015-deferred-fallback) |
| 5 | Remote-proxy features | owner decides, later | 0.6.0 accepted on the TV and soaked | [section 5](#5-future-remote-proxy-features) |
| 6 | Mid-term plan (diagnostics, front-end evaluation) | agent and owner | owner decisions in the plan's section 5 | [section 6](#6-mid-term-plan) |
| 7 | HA audio processing: next candidates | agent (PC and HA, owner approves HA changes) | the offline replay result is in | [section 7](#7-ha-audio-processing) |
| 8 | Custom wake word | private model repository | | [section 8](#8-wake-word) |

## 1. 0.6.0 owner acceptance test on c3-a

- **Who:** the owner, c3-a, mic button feature turned on. About 20 minutes. Install only plus the owner's own use of the remote; nothing else is changed on the TV.
- **Why it is open:** the grab was built and host-tested and then released as 0.6.0 (off by default). It has run on the TV in EXP-026 (a test tool, with a 60 s cap), not yet as the product. Until this list passes, the release is not accepted on the device.
- **Where the steps came from:** ADR 0016 has an earlier seven-step version under "On-TV acceptance". The list below is the current one and replaces it: it adds the number key, the 2.5 s timing and the exact status strings. Update the ADR's text from the results rather than keeping two lists.

Steps:

1. Turn on the mic button feature, then check the keys that were unconfirmed in the grabbed trace: Home, volume held up and down, and a number key.
2. `ctl.sh status --json` shows `lg_voice_key` `ours (grab + configd)` and `key_grab` `on: ...`. The TV app reads "On, no LG toast".
3. Ten mic presses, taps and holds: our assistant runs every time, no LG toast, no LG assistant.
4. Pointer, click, wheel, Back, arrows and OK all work normally.
5. Leave the remote idle for 5 minutes, then press the mic button right away (the wake path).
6. Kill test, while using the remote: `kill -9 $(head -n 1 /var/lib/webos-satellite/run/remote-proxy.pid)` (the file's second line names the target). The remote keeps working, and the grab comes back within about 2.5 s. Until then a mic press shows LG's toast (the safety net of ADR 0016 decision 5).
7. Turn the feature off: the mic button opens LG's assistant again, and `lg_voice_key` reads `lg`.
8. Diagnostics: forwarding delay median under 2 ms and maximum under 10 ms, and the helper's CPU use (with the pointer moving and at rest).

Pass: every step passes. Record the result as a short entry in the c3-a P4 log and the [c3-a record](../compatibility/c3-a.md), with counts and timings only. Fail: turn the feature off (the kernel releases the grab, ADR 0013's restore runs), or `ctl.sh restore-lg-voice`, or restart the TV; write down which step failed before changing anything. Two remotes in one household are a known unverified case (ADR 0016, risks).

## 2. c3-b is not on the grab allow list

- **State:** c3-b runs firmware `03.30.73`. The grab is enabled only on firmware where EXP-026 passed, and that is c3-a's `03.30.74` only ([verified models](../compatibility/verified-models.md), [ADR 0006](../decisions/0006-soft-firmware-policy-and-compatibility-reports.md)). On c3-b the feature runs as ADR 0013 describes: the configd override alone, so the toast shows.
- **To enable it:** the owner runs EXP-026 Part B2 (grab test) and B3 (kill test) on c3-b ([EXP-026](../experiments/EXP-026-mic-key-grab-reinject.md)). If both pass, add the firmware to the allow-list table and record the result in the c3-b record. Part A (read-only) on c3-b comes first if it was not run.
- **Prerequisite:** c3-b is still **muted by HA's switch**, so its microphone is closed. The acceptance steps that use the assistant need it unmuted, and only the owner decides when to change that.
- **Also open:** the second-remote question (section 1).

## 3. Security follow-up: the installer and the app directory mode

- **Open question:** at the 0.5.1 install, app directories were 0777 right after the app installer ran and 0755 after our fix. c3-b's directory was 0755 on 2026-11-03 and 0777 later, which suggests the installer resets it ([install permissions, section 8, result 2](../security/install-permissions.md#8-what-needs-a-tv-check)). That is a likely explanation, not proof.
- **To confirm:** at the next app update the owner approves, run `stat -c '%a %U %n'` on the app directory and `payload/` right after the installer finishes and before opening the app or running `ctl.sh install`. Expected: 0777 again, from 0755. Either way the fix at every install and start stays.
- **Who:** an agent, with the owner's go for the update. Read-only after the install.

## 4. ADR 0015: deferred fallback

[ADR 0015](../decisions/0015-silent-mic-key-filter.md) (a silent key filter inside LG's compositor) stays "proposed, deferred". It is the only known route for firmware where the grab cannot work (no identical sibling device, or a voice key that no longer reaches the input layer, as reported for webOS 11.2). Reconsider it only for such a firmware. Nothing to do now.

## 5. Future remote-proxy features

Button remapping, long and double press, assistant controls, a "remote in use" signal, key blocking and injection: ideas only, in [remote key proxy: future features](../design/remote-key-proxy-future-features.md). They build on the helper of ADR 0016 and need their own decisions, after the acceptance in section 1 and a soak.

## 6. Mid-term plan

Follow [mid-term-plan.md](mid-term-plan.md); do not restate it here.

- **Order of work:** diagnostics first (the plan's MT-3, named "0.5.2" there), then a front-end change only after an offline go (MT-7). Note: the plan's version numbers date from before 0.6.0 shipped the grab, so the release number for the diagnostics build is for the owner to pick when it is cut.
- **Owner decisions still open** (plan section 5):
  - pull requests or direct commits to main;
  - consent and recording sessions for raw two-microphone captures (who, which TV content, where the files live);
  - the 4 hour empty-room soak;
  - the retrain policy (may a front-end change trigger a retrain, may two model variants run at once);
  - which diagnostic entities to add;
  - the false-wake button in HA and whether the household uses it.

## 7. HA audio processing

Facts and numbers: [HA-side audio processing, section 10](../research/ha-side-audio-processing.md#10-results-of-the-offline-replay-2026-11-07). Server-side plan and the pending list: the homelab `docs/voice-assistant/audio-pipeline-plan.md` (public repository).

- **Do not filter audio before the wake word.** The offline replay showed no filter lowered the deployed wake word's false accepts, and two of the three cost recall in models trained for this front-end. Stage 5 is rejected.
- **Next candidates (not started):**
  1. Denoising in front of speech-to-text only (stage 3), measured against Whisper's `--vad-filter`, which is on since homelab commit `5690fe717`. The filter's own effect on noise-only runs has to be measured first, from counts only.
  2. A voice-activity gate, or a second check, inside a wake word proxy (stage 4), simulated in the offline scorer first. It leaves the model's input unchanged.
- **Rules:** keep model names generic in this public repository. Counts and rates only; no audio, no transcripts.

## 8. Wake word

Custom wake word follow-up tracked in the private model repo.
