//! PCM discovery from the text of `/proc/asound/cards` and `/proc/asound/pcm`
//! (check C2).
//!
//! The caller reads the two files (read-only, nothing else under `/proc/asound`
//! or `/sys`) and passes the text here; this module does no I/O. The card is
//! found by **id**, never by index ([AGENTS.md]: no universal `hw:1,0`).

use crate::check::{CheckId, CheckResult, Reason};

/// One line of `/proc/asound/cards`: `" 1 [LGSIC          ]: LGSIC - LGSIC"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardInfo {
    /// Card index (not stable, never used to select a card).
    pub index: u32,
    /// Card id in the brackets.
    pub id: String,
}

/// One line of `/proc/asound/pcm`: `"01-00: id : name : playback 1 : capture 1"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcmInfo {
    /// Card index.
    pub card: u32,
    /// Device number on the card.
    pub device: u32,
    /// Stream id (can be empty).
    pub id: String,
    /// Stream name (can be empty).
    pub name: String,
    /// Offers a playback stream.
    pub playback: bool,
    /// Offers a capture stream.
    pub capture: bool,
}

/// Parsed cards and PCMs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PcmDiscovery {
    /// Cards.
    pub cards: Vec<CardInfo>,
    /// PCM devices.
    pub pcms: Vec<PcmInfo>,
}

impl PcmDiscovery {
    /// Parses the two files; unparsable lines are skipped.
    #[must_use]
    pub fn parse(cards_text: &str, pcm_text: &str) -> Self {
        Self {
            cards: parse_cards(cards_text),
            pcms: parse_pcms(pcm_text),
        }
    }

    /// Index of the card with this id.
    #[must_use]
    pub fn card_index(&self, id: &str) -> Option<u32> {
        self.cards.iter().find(|c| c.id == id).map(|c| c.index)
    }

    /// Capture device numbers of the card with this id, ascending.
    #[must_use]
    pub fn capture_devices(&self, card_id: &str) -> Vec<u32> {
        self.devices(card_id, |p| p.capture)
    }

    /// Playback device numbers of the card with this id, ascending.
    #[must_use]
    pub fn playback_devices(&self, card_id: &str) -> Vec<u32> {
        self.devices(card_id, |p| p.playback)
    }

    fn devices(&self, card_id: &str, pick: impl Fn(&PcmInfo) -> bool) -> Vec<u32> {
        let Some(idx) = self.card_index(card_id) else {
            return Vec::new();
        };
        let mut v: Vec<u32> = self
            .pcms
            .iter()
            .filter(|p| p.card == idx && pick(p))
            .map(|p| p.device)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

fn parse_cards(text: &str) -> Vec<CardInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            continue;
        }
        let Ok(index) = t[..digits].parse::<u32>() else {
            continue;
        };
        let rest = &t[digits..];
        let (Some(open), Some(close)) = (rest.find('['), rest.find(']')) else {
            continue;
        };
        if close < open || !rest[..open].trim().is_empty() {
            continue;
        }
        let id = rest[open + 1..close].trim();
        if id.is_empty() {
            continue;
        }
        out.push(CardInfo {
            index,
            id: id.to_string(),
        });
    }
    out
}

fn parse_pcms(text: &str) -> Vec<PcmInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split(':');
        let Some(head) = parts.next() else { continue };
        let Some((c, d)) = head.trim().split_once('-') else {
            continue;
        };
        let (Ok(card), Ok(device)) = (c.parse::<u32>(), d.parse::<u32>()) else {
            continue;
        };
        let id = parts.next().unwrap_or("").trim().to_string();
        let name = parts.next().unwrap_or("").trim().to_string();
        let (mut playback, mut capture) = (false, false);
        for flag in parts {
            let f = flag.trim();
            playback |= f.starts_with("playback");
            capture |= f.starts_with("capture");
        }
        out.push(PcmInfo {
            card,
            device,
            id,
            name,
            playback,
            capture,
        });
    }
    out
}

/// What the daemon wants to capture from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WantedPcm {
    /// Card id (for example the configured `LGSIC`).
    pub card_id: String,
    /// Configured device number, when a specific one is configured.
    pub device: Option<u32>,
}

/// Whether the device offers the configured rate and a supported format. This
/// is known only from the hardware-parameter negotiation of the normal open,
/// so before the first open it is [`CapsFit::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapsFit {
    /// Offers the configured rate and a supported format.
    Fits,
    /// Needs another format or rate than configured.
    NeedsOther,
    /// Not known (not negotiated yet).
    Unknown,
}

/// C2: capture PCM found by card id and capabilities.
///
/// FAIL: no such card, no capture PCM on it, or the configured device is
/// missing. WARN: present but needs another format or rate.
#[must_use]
pub fn check_pcm(discovery: &PcmDiscovery, wanted: &WantedPcm, fit: CapsFit) -> CheckResult {
    let id = CheckId::C2;
    if discovery.card_index(&wanted.card_id).is_none() {
        return CheckResult::fail(id, Reason::NoCard);
    }
    let capture = discovery.capture_devices(&wanted.card_id);
    if capture.is_empty() {
        return CheckResult::fail(id, Reason::NoCapturePcm);
    }
    if wanted.device.is_some_and(|d| !capture.contains(&d)) {
        return CheckResult::fail(id, Reason::DeviceMissing);
    }
    match fit {
        CapsFit::NeedsOther => CheckResult::warn(id, Reason::FormatDiffers),
        CapsFit::Fits | CapsFit::Unknown => CheckResult::pass(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Grade;

    // Shaped like the c3 files (names and ids as in the discovery records;
    // c3-a has an empty name and the id `WoV PDM Mic ...`).
    const CARDS: &str = " 0 [lg115x         ]: lg115x - lg115x\n                      lg115x\n 1 [LGSIC          ]: LGSIC - LGSIC\n                      LGSIC\n";
    const PCM: &str = "\
00-00: SoundEngine : : playback 1
00-10: MixerCapture : : capture 1
00-11: Bt : : capture 1
01-00: WoV PDM Mic snd-soc-dummy-dai-0 : : capture 1
01-01: SpeakerFeedback : : capture 1
01-02: Out : : playback 1 : capture 1
garbage line
";

    fn disc() -> PcmDiscovery {
        PcmDiscovery::parse(CARDS, PCM)
    }

    #[test]
    fn parses_cards_by_id_and_skips_continuation_lines() {
        let d = disc();
        assert_eq!(d.cards.len(), 2);
        assert_eq!(d.card_index("LGSIC"), Some(1));
        assert_eq!(d.card_index("lg115x"), Some(0));
        assert_eq!(d.card_index("nope"), None);
    }

    #[test]
    fn parses_pcms() {
        let d = disc();
        assert_eq!(d.pcms.len(), 6);
        assert_eq!(d.capture_devices("LGSIC"), vec![0, 1, 2]);
        assert_eq!(d.playback_devices("LGSIC"), vec![2]);
        assert_eq!(d.capture_devices("lg115x"), vec![10, 11]);
        assert_eq!(d.pcms[3].id, "WoV PDM Mic snd-soc-dummy-dai-0");
        assert_eq!(d.pcms[3].name, "");
    }

    #[test]
    fn found_by_id_not_by_index() {
        // Same cards with swapped indexes: the answer must not change.
        let cards = " 0 [LGSIC          ]: LGSIC - LGSIC\n 1 [lg115x         ]: lg115x - lg115x\n";
        let pcm = "00-00: Mic : : capture 1\n01-10: Cap : : capture 1\n";
        let d = PcmDiscovery::parse(cards, pcm);
        let w = WantedPcm {
            card_id: "LGSIC".into(),
            device: Some(0),
        };
        assert_eq!(check_pcm(&d, &w, CapsFit::Fits).grade, Grade::Pass);
    }

    #[test]
    fn c2_table() {
        let d = disc();
        let want = |c: &str, dev| WantedPcm {
            card_id: c.into(),
            device: dev,
        };
        assert_eq!(
            check_pcm(&d, &want("LGSIC", Some(0)), CapsFit::Fits).grade,
            Grade::Pass
        );
        assert_eq!(
            check_pcm(&d, &want("LGSIC", None), CapsFit::Unknown).grade,
            Grade::Pass
        );
        let w = check_pcm(&d, &want("LGSIC", Some(0)), CapsFit::NeedsOther);
        assert_eq!(
            (w.grade, w.reason),
            (Grade::Warn, Some(Reason::FormatDiffers))
        );
        let f = check_pcm(&d, &want("LGSIC", Some(7)), CapsFit::Fits);
        assert_eq!(
            (f.grade, f.reason),
            (Grade::Fail, Some(Reason::DeviceMissing))
        );
        let f = check_pcm(&d, &want("other", None), CapsFit::Fits);
        assert_eq!((f.grade, f.reason), (Grade::Fail, Some(Reason::NoCard)));
    }

    #[test]
    fn card_without_capture_pcm_fails() {
        let d = PcmDiscovery::parse(
            " 0 [Only          ]: Only - Only\n",
            "00-00: Out : : playback 1\n",
        );
        let w = WantedPcm {
            card_id: "Only".into(),
            device: None,
        };
        assert_eq!(
            check_pcm(&d, &w, CapsFit::Fits).reason,
            Some(Reason::NoCapturePcm)
        );
    }

    #[test]
    fn empty_input_is_no_card() {
        let d = PcmDiscovery::parse("", "");
        let w = WantedPcm {
            card_id: "LGSIC".into(),
            device: None,
        };
        assert_eq!(
            check_pcm(&d, &w, CapsFit::Fits).reason,
            Some(Reason::NoCard)
        );
    }
}
