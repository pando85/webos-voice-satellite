//! Parsing of `/proc/asound` for diagnostics and device selection.

use std::fs;
use std::path::Path;

use crate::CaptureError;

/// One PCM device as listed under `/proc/asound`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmInfo {
    /// Card index (from `/proc/asound/cards`). Do not hard-code it.
    pub card: u32,
    /// Card id, for example `LGSIC` on the LG C3.
    pub card_id: String,
    /// PCM device number on the card.
    pub device: u32,
    /// `true` for a capture stream, `false` for playback.
    pub capture: bool,
    /// The PCM `id` line.
    pub id: String,
    /// The PCM `name` line.
    pub name: String,
    /// Number of subdevices (1 means exclusive access).
    pub subdevices: u32,
}

impl PcmInfo {
    /// Whether `pattern` is contained in the PCM id or name.
    #[must_use]
    pub fn matches_name(&self, pattern: &str) -> bool {
        self.name.contains(pattern) || self.id.contains(pattern)
    }
}

/// Parses `/proc/asound/cards` into `(index, id)` pairs.
///
/// Lines look like ` 1 [LGSIC         ]: LG_SIC - LGSIC`; the continuation lines
/// are skipped.
pub(crate) fn parse_cards(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (num, rest) = line.split_once(char::is_whitespace)?;
            let index = num.parse::<u32>().ok()?;
            let rest = rest.trim_start().strip_prefix('[')?;
            let (id, _) = rest.split_once(']')?;
            Some((index, id.trim().to_owned()))
        })
        .collect()
}

/// Looks up `key:` in an ALSA proc `info` file.
fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

/// Parses `/proc/asound/cardN/pcmDx/info`.
pub(crate) fn parse_pcm_info(text: &str, card: u32, card_id: &str) -> Option<PcmInfo> {
    let device = field(text, "device")?.parse().ok()?;
    let capture = match field(text, "stream")? {
        "CAPTURE" => true,
        "PLAYBACK" => false,
        _ => return None,
    };
    Some(PcmInfo {
        card,
        card_id: card_id.to_owned(),
        device,
        capture,
        id: field(text, "id").unwrap_or_default().to_owned(),
        name: field(text, "name").unwrap_or_default().to_owned(),
        subdevices: field(text, "subdevices_count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    })
}

/// Lists PCMs below `root` (normally `/proc/asound`), sorted by card, device,
/// capture-first.
pub(crate) fn list_pcms_in(root: &Path) -> Result<Vec<PcmInfo>, CaptureError> {
    let cards = parse_cards(&fs::read_to_string(root.join("cards"))?);
    let mut out = Vec::new();
    for (index, id) in cards {
        let Ok(entries) = fs::read_dir(root.join(format!("card{index}"))) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let is_pcm_dir = name.strip_prefix("pcm").is_some_and(|r| {
                r.ends_with(['c', 'p']) && r[..r.len() - 1].parse::<u32>().is_ok()
            });
            if !is_pcm_dir {
                continue;
            }
            let Ok(text) = fs::read_to_string(entry.path().join("info")) else {
                continue;
            };
            if let Some(info) = parse_pcm_info(&text, index, &id) {
                out.push(info);
            }
        }
    }
    out.sort_by_key(|p| (p.card, p.device, !p.capture));
    Ok(out)
}

/// Lists all PCM devices known to the kernel, by parsing `/proc/asound`.
/// Meant for diagnostics and for resolving a card by id.
///
/// # Errors
/// [`CaptureError::Io`] if `/proc/asound/cards` cannot be read (for example no
/// sound card, or not Linux).
pub fn list_pcms() -> Result<Vec<PcmInfo>, CaptureError> {
    list_pcms_in(Path::new("/proc/asound"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: &str = " 0 [lg115x         ]: lg115x - lg115x\n                      lg115x\n 1 [LGSIC          ]: LG_SIC - LGSIC\n                      LGSIC\n";

    const INFO: &str = "card: 1\ndevice: 0\nsubdevice: 0\nstream: CAPTURE\nid: WoV PDM Mic snd-soc-dummy-dai-0\nname: WoV PDM Mic\nsubname: subdevice #0\nclass: 0\nsubclass: 0\nsubdevices_count: 1\nsubdevices_avail: 1\n";

    #[test]
    fn parses_cards() {
        assert_eq!(
            parse_cards(CARDS),
            [(0, "lg115x".to_owned()), (1, "LGSIC".to_owned())]
        );
        assert_eq!(parse_cards("--- no soundcards ---\n").len(), 0);
    }

    #[test]
    fn parses_pcm_info() {
        let p = parse_pcm_info(INFO, 1, "LGSIC").unwrap_or_else(|| panic!("no parse"));
        assert_eq!((p.card, p.device, p.capture, p.subdevices), (1, 0, true, 1));
        assert!(p.matches_name("WoV PDM Mic"));
        assert!(!p.matches_name("SpeakerFeedback"));
        assert!(parse_pcm_info("garbage", 1, "x").is_none());
    }

    #[test]
    fn lists_fake_proc_tree() {
        let root = std::env::temp_dir().join(format!("webos-capture-proc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (dir, text) in [
            ("card1/pcm0c", INFO.to_owned()),
            (
                "card1/pcm1c",
                INFO.replace("device: 0", "device: 1")
                    .replace("WoV PDM Mic", "SpeakerFeedback"),
            ),
            ("card1/pcm0p", INFO.replace("CAPTURE", "PLAYBACK")),
            ("card1/pcm9x", INFO.to_owned()),
        ] {
            fs::create_dir_all(root.join(dir)).unwrap_or_else(|e| panic!("{e}"));
            fs::write(root.join(dir).join("info"), text).unwrap_or_else(|e| panic!("{e}"));
        }
        fs::write(root.join("cards"), CARDS).unwrap_or_else(|e| panic!("{e}"));
        let pcms = list_pcms_in(&root).unwrap_or_else(|e| panic!("{e}"));
        let _ = fs::remove_dir_all(&root);
        let summary: Vec<_> = pcms.iter().map(|p| (p.card, p.device, p.capture)).collect();
        assert_eq!(summary, [(1, 0, true), (1, 0, false), (1, 1, true)]);
        assert_eq!(pcms[0].card_id, "LGSIC");
    }

    #[test]
    fn missing_proc_is_io_error() {
        assert!(matches!(
            list_pcms_in(Path::new("/nonexistent/asound")),
            Err(CaptureError::Io(_))
        ));
    }
}
