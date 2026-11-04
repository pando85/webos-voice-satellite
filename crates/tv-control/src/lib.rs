//! TV control for the webOS voice satellite: screen, power, volume, mute,
//! input and app launch as Home Assistant entities
//! ([design](../../../docs/design/tv-control.md), [ADR 0005](../../../docs/decisions/0005-tv-control-entities.md)).
//!
//! The crate is sans-IO and knows nothing about capture, voice, playback or
//! ESPHome. [`Controller`] takes power-state updates, `luna-send` replies,
//! subscription lines, Home Assistant commands and the time, and returns
//! [`Effect`]s: luna calls to make, entity states to send and log lines. The
//! daemon (`crates/satellite/src/tv_control.rs`) runs the calls with
//! `luna-send` and maps the entities onto the ESPHome API crate.
//!
//! - [`luna`]: reply parsers (JSON, tolerant of extra fields, strict about
//!   the fields the design uses; error values never carry a reply body).
//! - [`lists`]: the input and app select options.
//! - [`limits`]: rate limits of the setters.
//! - [`setters`]: luna methods, payloads and the declaration check.
//! - [`entities`]: the entity table as plain data.
//! - [`controller`]: the state machine.
//!
//! Evidence labels: the getters, the power path and (EXP-010 Part A,
//! 2026-11-04) the volume, mute, input and app setters are `verified-on-target`
//! on c3-b (`03.30.73`) only; c3-a is untested. Every control is still
//! checked per TV at startup and a failing one is switched off.

#![cfg_attr(test, allow(clippy::assert_is_empty))]

pub mod controller;
pub mod entities;
pub mod limits;
pub mod lists;
pub mod luna;
pub mod setters;

pub use controller::{
    Action, Command, Config, Controller, Effect, Level, PowerOffEnv, PowerOffRefusal, PowerState,
    Purpose, Topic,
};
pub use entities::{Category, EntityDef, Kind, StateUpdate, StateValue};
pub use limits::Control;
pub use setters::{Declared, Setter};
