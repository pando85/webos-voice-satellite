//! Test helpers (compiled for tests only).
#![allow(clippy::unwrap_used)]

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Writes an executable `/bin/sh` script that fakes a TV tool (`luna-send`,
/// `pacat`) and returns only once it can be executed.
///
/// Tests run in parallel threads and fork children. A child forked while this
/// file is still open for writing holds a copy of the descriptor until it
/// execs, and executing the script in that window fails with `ETXTBSY`
/// ("Text file busy"): `player::tests::pacat_stream_with_fake_binary_writes_
/// zeros_at_gain_zero` and the `luna-send` tests failed about one run in a
/// hundred that way. The script answers `--exec-ready` with exit 0 before
/// anything else, and this function runs it that way until the exec works.
pub(crate) fn write_script(path: &Path, body: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        path,
        format!("#!/bin/sh\n[ \"${{1:-}}\" = \"--exec-ready\" ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..500 {
        match Command::new(path)
            .arg("--exec-ready")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => return,
        }
    }
    panic!("{} stayed busy", path.display());
}
