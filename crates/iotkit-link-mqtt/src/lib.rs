//! The device model ([`iotkit_model`]) over MQTT, discovered by Home Assistant.
//!
//! [`codec`] (MQTT 3.1.1) and [`ha`] (discovery, states, commands) are host-testable; the `esp`
//! module (chip feature) runs the client.

#![no_std]

/// `core::sync::atomic`, or `portable-atomic` on chips without compare-and-swap (ESP32-C3).
#[cfg(all(feature = "_esp", not(feature = "portable-atomic")))]
pub(crate) use core::sync::atomic;
#[cfg(all(feature = "_esp", feature = "portable-atomic"))]
pub(crate) use portable_atomic as atomic;

pub mod codec;
#[cfg(feature = "_esp")]
pub mod esp;
pub mod ha;

use heapless::String;
use iotkit_config::{Setting, keys};
use serde::{Deserialize, Serialize};

/// The broker, kept in flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Broker {
    /// Host name or IPv4 address.
    pub host: String<64>,
    pub port: u16,
    pub user: String<32>,
    pub password: String<64>,
}

impl Setting for Broker {
    const KEY: u16 = keys::MQTT_BROKER;
    const VERSION: u8 = 1;
}

impl Broker {
    /// `host` or `host:port` (1883 by default); `None` if too long or the port is invalid.
    pub fn parse(address: &str, user: &str, password: &str) -> Option<Self> {
        let (host, port) = match address.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()?),
            None => (address, 1883),
        };
        if host.is_empty() {
            return None;
        }
        Some(Self {
            host: host.try_into().ok()?,
            port,
            user: user.try_into().ok()?,
            password: password.try_into().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_addresses() {
        let b = Broker::parse("10.0.0.2", "", "").unwrap();
        assert_eq!((b.host.as_str(), b.port), ("10.0.0.2", 1883));
        let b = Broker::parse("broker.lan:8883", "u", "p").unwrap();
        assert_eq!((b.host.as_str(), b.port, b.user.as_str()), ("broker.lan", 8883, "u"));
        assert!(Broker::parse(":1883", "", "").is_none());
        assert!(Broker::parse("h:x", "", "").is_none());
    }
}
