//! Microphone self-checks and the sanitized compatibility report.
//!
//! Two groups of pure modules (no I/O, no clock, no dependencies), designed in
//! `docs/design/firmware-policy-and-reports.md` (ADR 0006):
//!
//! * the self-check engine ([`engine`], [`window`], [`pcm`], [`check`]): turns
//!   the normal capture frames the daemon already reads into the results of
//!   checks C2 to C7 and a retry/backoff state machine. It never opens a PCM,
//!   never reads sysfs or debugfs and never stores samples; only level
//!   statistics are kept. Time is injected by the caller.
//! * the report builder ([`report`], [`link`], [`sanitize`]): builds a short
//!   and a full text report from a typed allow-list struct, and a prefilled
//!   GitHub issue link. Free strings are sanitized, and a final scan fails
//!   closed.
//!
//! All thresholds are `hypothesis` until they are run on c3-a and c3-b.

pub mod check;
pub mod engine;
pub mod link;
pub mod pcm;
pub mod report;
pub mod sanitize;
pub mod window;
