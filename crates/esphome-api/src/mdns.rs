//! Optional `_esphomelib._tcp` mDNS advertisement (fails soft).
//!
//! TXT keys follow what HA 2026.10.4's ESPHome config flow reads (`mac`,
//! required; `friendly_name`; `api_encryption` marks a Noise device) plus the
//! informational keys ESPHome sends (`version`, `platform`, `board`, `network`).
//! HA can always add the device by IP instead; mDNS only speeds up discovery
//! and reconnects. Whether UDP 5353 is free on the TV is still unverified.
//!
//! Pairing: `api_encryption` follows the runtime key state, so an unpaired
//! device is offered to HA as keyless (HA then tries plaintext and provisions
//! a key) and a paired one as encrypted. While unpaired with the pairing
//! window closed the server does not advertise at all.

use std::net::IpAddr;

use crate::config::Resolved;

/// ESPHome mDNS service type.
#[cfg(feature = "mdns")]
pub(crate) const SERVICE_TYPE: &str = "_esphomelib._tcp.local.";

/// TXT records for the advertisement.
#[cfg(any(feature = "mdns", test))]
pub(crate) fn txt_records(cfg: &Resolved, encrypted: bool) -> Vec<(&'static str, String)> {
    let mut txt = vec![
        ("friendly_name", cfg.friendly_name.clone()),
        ("version", crate::ESPHOME_VERSION.to_owned()),
        ("mac", cfg.mac.compact()),
        ("platform", "webOS".to_owned()),
        ("board", cfg.model.clone()),
        ("network", "ethernet".to_owned()),
    ];
    if encrypted {
        txt.push(("api_encryption", crate::frame::NOISE_PROTOCOL.to_owned()));
    }
    txt
}

/// Running advertisement; stops when dropped.
pub(crate) struct Advertiser {
    #[cfg(feature = "mdns")]
    daemon: mdns_sd::ServiceDaemon,
    #[cfg(feature = "mdns")]
    info: mdns_sd::ServiceInfo,
}

impl Advertiser {
    /// Registers the service again so it is announced again (after a system
    /// resume). aioesphomeapi's reconnect logic connects at once when it sees
    /// the device's record while disconnected (`ZeroconfWake`). Fails soft.
    pub(crate) fn reannounce(&self) {
        #[cfg(feature = "mdns")]
        match self.daemon.register(self.info.clone()) {
            Ok(()) => log::info!("mdns: announced again"),
            Err(err) => log::warn!("mdns: re-announcement failed: {err}"),
        }
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        #[cfg(feature = "mdns")]
        if let Err(err) = self.daemon.shutdown() {
            log::debug!("mdns: shutdown: {err}");
        }
    }
}

/// Starts advertising; logs and returns `None` on any failure.
#[cfg(feature = "mdns")]
pub(crate) fn advertise(
    cfg: &Resolved,
    bind: IpAddr,
    port: u16,
    encrypted: bool,
) -> Option<Advertiser> {
    let txt = txt_records(cfg, encrypted);
    let props: Vec<(&str, &str)> = txt.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let host = format!("{}.local.", cfg.name);
    let info = if bind.is_unspecified() {
        mdns_sd::ServiceInfo::new(SERVICE_TYPE, &cfg.name, &host, (), port, props.as_slice())
            .map(mdns_sd::ServiceInfo::enable_addr_auto)
    } else {
        mdns_sd::ServiceInfo::new(SERVICE_TYPE, &cfg.name, &host, bind, port, props.as_slice())
    };
    let result = mdns_sd::ServiceDaemon::new().and_then(|daemon| {
        let info = info?;
        daemon.register(info.clone())?;
        Ok((daemon, info))
    });
    match result {
        Ok((daemon, info)) => {
            log::info!(
                "mdns: advertising {} on port {port} ({})",
                cfg.name,
                if encrypted { "encrypted" } else { "no key" }
            );
            Some(Advertiser { daemon, info })
        }
        Err(err) => {
            log::warn!("mdns: advertisement disabled: {err}");
            None
        }
    }
}

/// Built without the `mdns` feature: logs and returns `None`.
#[cfg(not(feature = "mdns"))]
pub(crate) fn advertise(
    cfg: &Resolved,
    _bind: IpAddr,
    _port: u16,
    _encrypted: bool,
) -> Option<Advertiser> {
    log::warn!(
        "mdns: requested for {} but built without the `mdns` feature",
        cfg.name
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_has_mac_and_encryption_marker() {
        let mut cfg = crate::Config::default();
        cfg.mac = Some("02:AB:CD:EF:01:23".to_owned());
        let resolved = cfg.resolve().unwrap();
        let txt = txt_records(&resolved, false);
        assert!(txt.contains(&("mac", "02abcdef0123".to_owned())));
        assert!(!txt.iter().any(|(k, _)| *k == "api_encryption"));
        let txt = txt_records(&resolved, true);
        assert!(txt.iter().any(|(k, _)| *k == "api_encryption"));
        // The key itself is never in the TXT records.
        cfg.noise_psk = Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned());
        let txt = txt_records(&cfg.resolve().unwrap(), true);
        assert!(!txt.iter().any(|(_, v)| v.contains("AAAA")));
    }
}
