# Security and privacy

Root-level access to a TV is sensitive. Until a production security design is accepted, the project only ships read-only diagnostics and research documentation.

## Requirements

- No unsolicited ambient recording or audio transmission.
- Explicit user opt-in for microphones; clear local mute and visible/inspectable listening state are future acceptance criteria.
- No capture logs or PCM dumps in Git. Keep device data private and redact identifiers before publishing.
- Never place Home Assistant long-lived tokens in scripts, examples, process arguments, public logs, or PRs. Explore least-privilege credentials and secure provisioning.
- No unprotected listening sockets or unauthenticated control endpoints; network transports must use appropriate authentication.
- Boot hooks, service overlays and firmware modifications need a rollback plan and explicit approval.
- Treat downloaded software, binary dumps and reverse-engineering outputs as untrusted.

For security issues, contact maintainers privately instead of disclosing exploitable device details in a public issue. A dedicated reporting channel will be specified before production releases.
