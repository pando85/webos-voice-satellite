//! Finding input nodes **by name** in sysfs (never by a fixed `eventN`) and
//! reading their capabilities.

use std::fs;
use std::path::{Path, PathBuf};

/// sysfs root for event nodes.
pub(crate) const SYS_INPUT: &str = "/sys/class/input";

/// One event node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    /// `eventN`.
    pub(crate) event: String,
    /// `/dev/input/eventN`.
    pub(crate) dev: PathBuf,
    pub(crate) name: String,
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

/// All `eventN` nodes under `root` with their names, ordered by N.
pub(crate) fn list(root: &Path) -> Vec<Node> {
    let mut out: Vec<(u32, Node)> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let event = e.file_name().to_string_lossy().into_owned();
            let n: u32 = event.strip_prefix("event")?.parse().ok()?;
            let name = read_trim(&e.path().join("device/name"))?;
            Some((
                n,
                Node {
                    dev: PathBuf::from("/dev/input").join(&event),
                    event,
                    name,
                },
            ))
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out.into_iter().map(|(_, n)| n).collect()
}

/// The single node named exactly `name`; an error for none or several.
pub(crate) fn find(root: &Path, name: &str) -> Result<Node, String> {
    let hits: Vec<Node> = list(root).into_iter().filter(|n| n.name == name).collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().unwrap_or_else(|| unreachable!())),
        0 => Err(format!("no input node named {name:?}")),
        k => Err(format!("{k} input nodes named {name:?}; refusing")),
    }
}

/// A node's capabilities as code lists (from `EVIOCGBIT`, which gives the
/// same bytes to 32- and 64-bit readers, unlike the sysfs text whose word
/// width depends on the reader) and its id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Caps {
    pub(crate) ev: Vec<u16>,
    pub(crate) key: Vec<u16>,
    pub(crate) rel: Vec<u16>,
    pub(crate) abs: Vec<u16>,
    pub(crate) msc: Vec<u16>,
    /// bustype, vendor, product, version (from sysfs `device/id`).
    pub(crate) id: [u16; 4],
}

impl Caps {
    /// The first capability that differs, or `None` if all are equal (the
    /// id is not compared: the siblings differ only by name).
    pub(crate) fn first_difference(&self, other: &Self) -> Option<&'static str> {
        [
            ("ev", &self.ev, &other.ev),
            ("key", &self.key, &other.key),
            ("rel", &self.rel, &other.rel),
            ("abs", &self.abs, &other.abs),
            ("msc", &self.msc, &other.msc),
        ]
        .into_iter()
        .find(|(_, a, b)| a != b)
        .map(|(n, _, _)| n)
    }
}

/// The node's id from sysfs (hex fields; 0 when unreadable).
pub(crate) fn read_id(root: &Path, node: &Node) -> [u16; 4] {
    let d = root.join(&node.event).join("device/id");
    let f = |n: &str| {
        read_trim(&d.join(n))
            .and_then(|v| u16::from_str_radix(&v, 16).ok())
            .unwrap_or(0)
    };
    [f("bustype"), f("vendor"), f("product"), f("version")]
}

/// Codes set in a kernel bitmap read as bytes (bit n in byte n/8).
pub(crate) fn codes(bitmap: &[u8]) -> Vec<u16> {
    let mut out = Vec::new();
    for (i, byte) in bitmap.iter().enumerate() {
        for b in 0..8 {
            if byte & (1 << b) != 0
                && let Ok(c) = u16::try_from(i * 8 + b)
            {
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// c3-a's `LGE M-RCU - Builtin [0]` key bitmap as printed by sysfs to a
    /// 32-bit reader (EXP-026 Part A), turned into the byte layout `EVIOCGBIT`
    /// returns.
    const C3A_KEY: &str = "7f0350 fc00 0 0 65f83fbf cdffffff 80bee01e f0030004 88000000 40 0 7fffff ffffffff 20206003 e9081e28 0 0 0 0 0 0 0 0 0 0 1000 100fc000 2001440 0 0 70000 20000 2018000 4180 10801 9e1680 0 0 10000ffc";

    fn bytes_from_32bit_words(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .rev()
            .flat_map(|w| {
                u32::from_str_radix(w, 16)
                    .unwrap_or_else(|e| panic!("{e}"))
                    .to_le_bytes()
            })
            .collect()
    }

    fn fake_node(root: &Path, event: &str, name: &str) {
        let d = root.join(event).join("device");
        fs::create_dir_all(d.join("id")).unwrap_or_else(|e| panic!("{e}"));
        fs::write(d.join("name"), format!("{name}\n")).unwrap_or_else(|e| panic!("{e}"));
        for (f, v) in [
            ("bustype", "0003"),
            ("vendor", "0000"),
            ("product", "0000"),
            ("version", "0004"),
        ] {
            fs::write(d.join("id").join(f), format!("{v}\n")).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("remote-proxy-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn the_c3a_bitmap_has_the_mic_key_and_lgs_private_codes() {
        let b = codes(&bytes_from_32bit_words(C3A_KEY));
        assert!(b.contains(&428));
        // Up, Enter, LG Back (412, EXP-022)
        assert!(b.contains(&103) && b.contains(&28) && b.contains(&412));
        for c in 1194..=1199 {
            assert!(b.contains(&c), "{c}");
        }
        assert_eq!(codes(&[0x03]), vec![0, 1]);
        assert_eq!(codes(&[0x00, 0x01]), vec![8]);
    }

    #[test]
    fn nodes_are_found_by_exact_name_and_duplicates_are_refused() {
        let root = tmp("find");
        fake_node(&root, "event2", "LGE M-RCU - Builtin [0]");
        fake_node(&root, "event4", "LGE M-RCU - Builtin [2]");
        fake_node(&root, "event10", "LGE M-RCU - Builtin [2]x");
        let n = find(&root, "LGE M-RCU - Builtin [2]").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(n.event, "event4");
        assert_eq!(n.dev, PathBuf::from("/dev/input/event4"));
        assert_eq!(read_id(&root, &n), [3, 0, 0, 4]);
        assert!(find(&root, "LGE M-RCU - Builtin [1]").is_err());
        fake_node(&root, "event5", "LGE M-RCU - Builtin [0]");
        assert!(find(&root, "LGE M-RCU - Builtin [0]").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn differing_capabilities_are_reported() {
        let a = Caps {
            ev: vec![0, 1, 2, 3],
            key: codes(&bytes_from_32bit_words(C3A_KEY)),
            rel: vec![8],
            abs: vec![0, 1],
            ..Caps::default()
        };
        let b = Caps {
            id: [9, 9, 9, 9],
            ..a.clone()
        };
        let c = Caps {
            key: vec![428],
            ..a.clone()
        };
        let d = Caps {
            abs: vec![0],
            ..a.clone()
        };
        assert_eq!(a.first_difference(&b), None);
        assert_eq!(a.first_difference(&c), Some("key"));
        assert_eq!(a.first_difference(&d), Some("abs"));
    }
}
