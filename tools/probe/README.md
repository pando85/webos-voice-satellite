# Read-only webOS inventory

`probe.sh` uses POSIX shell and basic system utilities; no installation, microphone recording, service tracing, or device changes.

Run locally on an authorized TV from SSH:

```sh
ssh root@TV_IP 'sh -s' < tools/probe/probe.sh > tv-private-report.txt
```

**Privacy warning:** even the inventory may disclose identifiers, process arguments and hardware details. The report is for local analysis only. Do not commit/share it without review and redaction. Results can vary by firmware and BusyBox build. The tool intentionally does **not** read environment variables, credentials, wireless configuration, or open file descriptors.

Compare the results privately, then manually create a sanitized record using `docs/compatibility/DEVICE-TEMPLATE.md`. No device has been probed by this repository.
