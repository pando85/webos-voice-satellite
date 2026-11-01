//! The entities this device exposes: one media player (whose ANNOUNCEMENT
//! format selects HA's TTS/announcement format) and a "Mute microphone"
//! switch.

use crate::config::Resolved;
use crate::proto::{
    EntityCategory, ListEntitiesMediaPlayerResponse, ListEntitiesSwitchResponse,
    MediaPlayerFormatPurpose, MediaPlayerState, MediaPlayerStateResponse,
    MediaPlayerSupportedFormat, SwitchStateResponse,
};
use crate::voice::MediaState;

/// Object id of the media player entity.
pub const MEDIA_PLAYER_OBJECT_ID: &str = "media_player";
/// Object id of the mute switch entity.
pub const MUTE_OBJECT_ID: &str = "mute_microphone";

/// `MediaPlayerEntityFeature.STOP` (aioesphomeapi 46.6.0). The only media
/// player feature offered; play_media/volume are not implemented.
pub const MEDIA_PLAYER_FEATURES: u32 = 1 << 12;

/// 32-bit FNV-1 hash of an object id, as ESPHome derives entity keys.
#[must_use]
pub fn entity_key(object_id: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in object_id.bytes() {
        hash = hash.wrapping_mul(0x0100_0193);
        hash ^= u32::from(b);
    }
    hash
}

pub(crate) fn media_player_key() -> u32 {
    entity_key(MEDIA_PLAYER_OBJECT_ID)
}

pub(crate) fn mute_key() -> u32 {
    entity_key(MUTE_OBJECT_ID)
}

pub(crate) fn list_media_player(cfg: &Resolved) -> ListEntitiesMediaPlayerResponse {
    let fmt = &cfg.announcement_format;
    ListEntitiesMediaPlayerResponse {
        object_id: MEDIA_PLAYER_OBJECT_ID.to_owned(),
        key: media_player_key(),
        // Empty name: HA names the entity after the device.
        name: String::new(),
        supported_formats: vec![MediaPlayerSupportedFormat {
            format: fmt.format.clone(),
            sample_rate: fmt.sample_rate,
            num_channels: fmt.num_channels,
            purpose: MediaPlayerFormatPurpose::Announcement as i32,
            sample_bytes: fmt.sample_bytes,
        }],
        feature_flags: MEDIA_PLAYER_FEATURES,
        ..Default::default()
    }
}

pub(crate) fn list_mute_switch() -> ListEntitiesSwitchResponse {
    ListEntitiesSwitchResponse {
        object_id: MUTE_OBJECT_ID.to_owned(),
        key: mute_key(),
        name: "Mute microphone".to_owned(),
        icon: "mdi:microphone-off".to_owned(),
        entity_category: EntityCategory::Config as i32,
        ..Default::default()
    }
}

pub(crate) fn media_player_state(state: MediaState) -> MediaPlayerStateResponse {
    MediaPlayerStateResponse {
        key: media_player_key(),
        state: match state {
            MediaState::Idle => MediaPlayerState::Idle,
            MediaState::Announcing => MediaPlayerState::Announcing,
        } as i32,
        volume: 1.0,
        muted: false,
        ..Default::default()
    }
}

pub(crate) fn mute_state(muted: bool) -> SwitchStateResponse {
    SwitchStateResponse {
        key: mute_key(),
        state: muted,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_fnv1_and_distinct() {
        // FNV-1 32-bit of "" is the offset basis; of "a" is 0x050c5d7e.
        assert_eq!(entity_key(""), 0x811c_9dc5);
        assert_eq!(entity_key("a"), 0x050c_5d7e);
        assert_ne!(media_player_key(), mute_key());
    }

    #[test]
    fn media_player_announces_format_with_sample_rate() {
        let cfg = crate::Config::default().resolve().unwrap();
        let mp = list_media_player(&cfg);
        let f = &mp.supported_formats[0];
        assert_eq!(f.purpose, MediaPlayerFormatPurpose::Announcement as i32);
        assert_eq!(
            (
                f.format.as_str(),
                f.sample_rate,
                f.num_channels,
                f.sample_bytes
            ),
            ("flac", 22_050, 1, 2)
        );
    }
}
