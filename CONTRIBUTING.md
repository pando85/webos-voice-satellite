# Contributing

This project currently welcomes documented research, safe probes, reproducible hardware tests, and reviews of technical assumptions.

Read [AGENTS.md](AGENTS.md). Before submitting a finding, identify the tested TV model, webOS/firmware version, capture or playback state, and exact evidence. Avoid serial numbers, IPs, credentials, private recordings, or redistribution of proprietary firmware.

Prefer a focused pull request per experiment or research topic. Explain what is **verified**, **inferred**, or **still unknown**. No persistent or destructive test should be performed as part of automated CI. See [experiments](docs/experiments/README.md).

**Compatibility reports.** If you run the satellite on a TV, a report is the most useful contribution, whether it works or not. Use the TV app's "Report compatibility" button (QR code) or `ctl.sh report`; both open the prefilled [compatibility report form](.github/ISSUE_TEMPLATE/compatibility-report.yml). The report is built from a fixed allow-list (TV model, webOS and firmware, architecture, audio device ids, check results, level numbers, power states, TV control results) and never holds a serial number, MAC or IP address, device or Home Assistant name, key, audio or transcript; read it before you submit, because the issue is public. Maintainers turn reports into rows of the [verified TVs list](docs/compatibility/verified-models.md) (two independent reports or a maintainer run for "Verified"); a report alone stays `observed-on-other-device`.

Local checks:

```sh
sh -n tools/probe/probe.sh
python3 tools/check_docs.py
python3 -m unittest discover -s tools -p 'test_*.py'
```
