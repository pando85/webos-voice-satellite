# Contributing

This project currently welcomes documented research, safe probes, reproducible hardware tests, and reviews of technical assumptions.

Read [AGENTS.md](AGENTS.md). Before submitting a finding, identify the tested TV model, webOS/firmware version, capture or playback state, and exact evidence. Avoid serial numbers, IPs, credentials, private recordings, or redistribution of proprietary firmware.

Prefer a focused pull request per experiment or research topic. Explain what is **verified**, **inferred**, or **still unknown**. No persistent or destructive test should be performed as part of automated CI. See [experiments](docs/experiments/README.md).

Local checks:

```sh
sh -n tools/probe/probe.sh
python3 tools/check_docs.py
```
