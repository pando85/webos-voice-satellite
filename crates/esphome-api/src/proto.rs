//! ESPHome native API messages generated from the vendored `proto/api.proto`
//! (ESPHome 2026.10.1) by `prost-build` + `protox` in `build.rs`.
//!
//! Every message that carries an `option (id)` in the proto implements
//! [`ApiMessage`], which gives its wire message type id.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]

include!(concat!(env!("OUT_DIR"), "/_.rs"));

/// A protobuf message with an ESPHome native API message type id.
pub trait ApiMessage: prost::Message + Default {
    /// Message type id (`option (id)` in `api.proto`).
    const ID: u16;
    /// Proto message name, for logs.
    const NAME: &'static str;
}

include!(concat!(env!("OUT_DIR"), "/message_ids.rs"));
