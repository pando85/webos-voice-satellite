//! The entities this device exposes: one media player (whose ANNOUNCEMENT
//! format selects HA's TTS/announcement format and whose volume is the reply
//! volume), a "Mute microphone" switch and, optionally, a diagnostic "TV
//! power state" text sensor, plus the application-defined `select`, `number`,
//! `button` and `binary_sensor` entities from [`crate::Config`].
//!
//! Availability: the ESPHome API has no per-entity "unavailable". All entities
//! become unavailable in HA while the device is disconnected; a number or
//! binary sensor with no valid state is sent with `missing_state` (HA shows
//! "unknown").

use crate::config::{BinarySensorEntity, ButtonEntity, NumberEntity, Resolved, SelectEntity};
use crate::proto::{
    BinarySensorStateResponse, EntityCategory, ListEntitiesBinarySensorResponse,
    ListEntitiesButtonResponse, ListEntitiesMediaPlayerResponse, ListEntitiesNumberResponse,
    ListEntitiesSelectResponse, ListEntitiesSwitchResponse, ListEntitiesTextSensorResponse,
    MediaPlayerFormatPurpose, MediaPlayerState, MediaPlayerStateResponse,
    MediaPlayerSupportedFormat, NumberStateResponse, SelectStateResponse, SwitchStateResponse,
    TextSensorStateResponse,
};
use crate::voice::MediaState;

/// Object id of the media player entity.
pub const MEDIA_PLAYER_OBJECT_ID: &str = "media_player";
/// Object id of the mute switch entity.
pub const MUTE_OBJECT_ID: &str = "mute_microphone";
/// Object id of the TV power state text sensor.
pub const POWER_STATE_OBJECT_ID: &str = "tv_power_state";

/// `MediaPlayerEntityFeature.VOLUME_SET` (aioesphomeapi 46.6.0, the same
/// value as HA's flag): HA shows a volume slider and sends `has_volume`.
pub const FEATURE_VOLUME_SET: u32 = 1 << 2;
/// `MediaPlayerEntityFeature.VOLUME_STEP`: HA's volume up/down send the
/// `VOLUME_UP`/`VOLUME_DOWN` commands.
pub const FEATURE_VOLUME_STEP: u32 = 1 << 10;
/// `MediaPlayerEntityFeature.STOP`.
pub const FEATURE_STOP: u32 = 1 << 12;

/// The media player features offered: stop and the reply volume (set and
/// step). HA 2026.10.4 takes the entity's features from these flags
/// (`feature_flags_compat` for API 1.14). play_media and mute are not
/// implemented.
pub const MEDIA_PLAYER_FEATURES: u32 = FEATURE_STOP | FEATURE_VOLUME_SET | FEATURE_VOLUME_STEP;

/// Reply volume change for one `VOLUME_UP`/`VOLUME_DOWN`, in percent.
pub const VOLUME_STEP_PERCENT: u8 = 5;

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

pub(crate) fn power_state_key() -> u32 {
    entity_key(POWER_STATE_OBJECT_ID)
}

pub(crate) fn list_number(n: &NumberEntity) -> ListEntitiesNumberResponse {
    ListEntitiesNumberResponse {
        object_id: n.object_id.clone(),
        key: entity_key(&n.object_id),
        name: n.name.clone(),
        icon: n.icon.clone(),
        min_value: n.min_value,
        max_value: n.max_value,
        step: n.step,
        entity_category: n.entity_category as i32,
        unit_of_measurement: n.unit.clone(),
        mode: n.mode as i32,
        device_class: n.device_class.clone(),
        ..Default::default()
    }
}

/// `None`: no valid state (HA shows "unknown").
pub(crate) fn number_state(n: &NumberEntity, value: Option<f32>) -> NumberStateResponse {
    NumberStateResponse {
        key: entity_key(&n.object_id),
        state: value.unwrap_or_default(),
        missing_state: value.is_none(),
        ..Default::default()
    }
}

/// A command value the number accepts: finite and within its range.
pub(crate) fn number_accepts(n: &NumberEntity, value: f32) -> bool {
    value.is_finite() && value >= n.min_value && value <= n.max_value
}

pub(crate) fn list_button(b: &ButtonEntity) -> ListEntitiesButtonResponse {
    ListEntitiesButtonResponse {
        object_id: b.object_id.clone(),
        key: entity_key(&b.object_id),
        name: b.name.clone(),
        icon: b.icon.clone(),
        entity_category: b.entity_category as i32,
        device_class: b.device_class.clone(),
        ..Default::default()
    }
}

pub(crate) fn list_binary_sensor(b: &BinarySensorEntity) -> ListEntitiesBinarySensorResponse {
    ListEntitiesBinarySensorResponse {
        object_id: b.object_id.clone(),
        key: entity_key(&b.object_id),
        name: b.name.clone(),
        icon: b.icon.clone(),
        device_class: b.device_class.clone(),
        entity_category: b.entity_category as i32,
        ..Default::default()
    }
}

/// `None`: no valid state (HA shows "unknown").
pub(crate) fn binary_sensor_state(
    b: &BinarySensorEntity,
    state: Option<bool>,
) -> BinarySensorStateResponse {
    BinarySensorStateResponse {
        key: entity_key(&b.object_id),
        state: state.unwrap_or_default(),
        missing_state: state.is_none(),
        ..Default::default()
    }
}

/// HA's media player volume (0.0 to 1.0) to percent, rounded and clamped.
/// NaN gives 0.
#[must_use]
pub fn volume_to_percent(volume: f32) -> u8 {
    if volume.is_nan() {
        return 0;
    }
    // Clamped to 0..=100 first, so the cast cannot truncate or wrap.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let p = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
    p
}

/// One volume step up or down from `percent`, clamped to 0..=100.
#[must_use]
pub fn volume_step(percent: u8, up: bool) -> u8 {
    if up {
        percent.saturating_add(VOLUME_STEP_PERCENT).min(100)
    } else {
        percent.min(100).saturating_sub(VOLUME_STEP_PERCENT)
    }
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

pub(crate) fn list_power_sensor() -> ListEntitiesTextSensorResponse {
    ListEntitiesTextSensorResponse {
        object_id: POWER_STATE_OBJECT_ID.to_owned(),
        key: power_state_key(),
        name: "TV power state".to_owned(),
        icon: "mdi:television".to_owned(),
        entity_category: EntityCategory::Diagnostic as i32,
        ..Default::default()
    }
}

pub(crate) fn list_select(sel: &SelectEntity) -> ListEntitiesSelectResponse {
    ListEntitiesSelectResponse {
        object_id: sel.object_id.clone(),
        key: entity_key(&sel.object_id),
        name: sel.name.clone(),
        icon: sel.icon.clone(),
        options: sel.options.clone(),
        entity_category: EntityCategory::Config as i32,
        ..Default::default()
    }
}

pub(crate) fn select_state(sel: &SelectEntity, state: Option<&str>) -> SelectStateResponse {
    SelectStateResponse {
        key: entity_key(&sel.object_id),
        state: state.unwrap_or_default().to_owned(),
        missing_state: state.is_none(),
        ..Default::default()
    }
}

/// `None`: no state seen yet (HA shows "unknown").
pub(crate) fn power_state(state: Option<&str>) -> TextSensorStateResponse {
    TextSensorStateResponse {
        key: power_state_key(),
        state: state.unwrap_or_default().to_owned(),
        missing_state: state.is_none(),
        ..Default::default()
    }
}

pub(crate) fn media_player_state(
    state: MediaState,
    volume_percent: u8,
) -> MediaPlayerStateResponse {
    MediaPlayerStateResponse {
        key: media_player_key(),
        state: match state {
            MediaState::Idle => MediaPlayerState::Idle,
            // HA 2026.10.4 maps only IDLE/PLAYING/PAUSED/OFF/ON
            // (`esphome/media_player.py` `_STATES`); ANNOUNCING raised a
            // KeyError in HA on every TTS reply (seen on c3-a).
            MediaState::Announcing => MediaPlayerState::Playing,
        } as i32,
        volume: f32::from(volume_percent.min(100)) / 100.0,
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
        assert_ne!(power_state_key(), mute_key());
        assert_ne!(power_state_key(), media_player_key());
    }

    #[test]
    fn media_player_offers_stop_and_volume() {
        assert_eq!(MEDIA_PLAYER_FEATURES, 4096 | 4 | 1024);
        let cfg = crate::Config::default().resolve().unwrap();
        assert_eq!(list_media_player(&cfg).feature_flags, MEDIA_PLAYER_FEATURES);
        let st = media_player_state(MediaState::Idle, 40);
        assert!((st.volume - 0.4).abs() < 1e-6);
        assert!((media_player_state(MediaState::Idle, 250).volume - 1.0).abs() < 1e-6);
    }

    #[test]
    fn volume_conversion_and_steps() {
        assert_eq!(volume_to_percent(0.0), 0);
        assert_eq!(volume_to_percent(0.404), 40);
        assert_eq!(volume_to_percent(0.405), 41);
        assert_eq!(volume_to_percent(1.0), 100);
        assert_eq!(volume_to_percent(7.0), 100);
        assert_eq!(volume_to_percent(-1.0), 0);
        assert_eq!(volume_to_percent(f32::NAN), 0);
        assert_eq!(volume_step(40, true), 45);
        assert_eq!(volume_step(98, true), 100);
        assert_eq!(volume_step(3, false), 0);
        assert_eq!(volume_step(100, false), 95);
    }

    #[test]
    fn power_sensor_is_diagnostic_and_reports_missing_state() {
        let e = list_power_sensor();
        assert_eq!(e.entity_category, EntityCategory::Diagnostic as i32);
        assert!(power_state(None).missing_state);
        let st = power_state(Some("Active Standby"));
        assert!(!st.missing_state);
        assert_eq!(st.state, "Active Standby");
    }

    fn number() -> NumberEntity {
        NumberEntity {
            object_id: "tv_volume".to_owned(),
            name: "TV volume".to_owned(),
            icon: "mdi:volume-high".to_owned(),
            min_value: 0.0,
            max_value: 100.0,
            step: 1.0,
            unit: "%".to_owned(),
            device_class: "volume".to_owned(),
            mode: crate::proto::NumberMode::Slider,
            entity_category: EntityCategory::None,
        }
    }

    #[test]
    fn number_listing_carries_range_unit_class_and_category() {
        let mut n = number();
        n.entity_category = EntityCategory::Config;
        let l = list_number(&n);
        assert_eq!(
            (l.object_id.as_str(), l.key),
            ("tv_volume", entity_key("tv_volume"))
        );
        assert_eq!((l.min_value, l.max_value, l.step), (0.0, 100.0, 1.0));
        assert_eq!(l.unit_of_measurement, "%");
        assert_eq!(l.device_class, "volume");
        assert_eq!(l.mode, crate::proto::NumberMode::Slider as i32);
        assert_eq!(l.entity_category, EntityCategory::Config as i32);
    }

    #[test]
    fn number_state_reports_missing_and_accepts_only_in_range_values() {
        let n = number();
        assert!(number_state(&n, None).missing_state);
        let st = number_state(&n, Some(42.5));
        assert!(!st.missing_state);
        assert!((st.state - 42.5).abs() < 1e-6);
        assert_eq!(st.key, entity_key("tv_volume"));
        assert!(number_accepts(&n, 0.0) && number_accepts(&n, 100.0) && number_accepts(&n, 33.3));
        assert!(!number_accepts(&n, -0.1) && !number_accepts(&n, 100.1));
        assert!(!number_accepts(&n, f32::NAN) && !number_accepts(&n, f32::INFINITY));
    }

    #[test]
    fn button_and_binary_sensor_listing_and_state() {
        let b = ButtonEntity {
            object_id: "tv_mute".to_owned(),
            name: "TV mute".to_owned(),
            icon: "mdi:volume-off".to_owned(),
            device_class: String::new(),
            entity_category: EntityCategory::Diagnostic,
        };
        let l = list_button(&b);
        assert_eq!(
            (l.key, l.entity_category),
            (entity_key("tv_mute"), EntityCategory::Diagnostic as i32)
        );
        assert_eq!(l.name, "TV mute");
        let s = BinarySensorEntity {
            object_id: "listening".to_owned(),
            name: "Listening".to_owned(),
            icon: "mdi:ear-hearing".to_owned(),
            device_class: "running".to_owned(),
            entity_category: EntityCategory::None,
        };
        let l = list_binary_sensor(&s);
        assert_eq!(
            (l.key, l.device_class.as_str()),
            (entity_key("listening"), "running")
        );
        assert_eq!(l.entity_category, EntityCategory::None as i32);
        assert!(binary_sensor_state(&s, None).missing_state);
        let on = binary_sensor_state(&s, Some(true));
        assert!(on.state && !on.missing_state);
        let off = binary_sensor_state(&s, Some(false));
        assert!(!off.state && !off.missing_state);
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
