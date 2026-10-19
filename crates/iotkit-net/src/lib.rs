//! Networking: Wi-Fi station with reconnect, DHCP, DNS, an mDNS responder for
//! `<hostname>.local`, and a small HTTP client.
//!
//! The protocol logic ([`backoff`], [`mdns`], [`http`]) is host-testable; the `esp` module (chip
//! feature) runs it on esp-radio and embassy-net.

#![no_std]

pub mod backoff;
#[cfg(feature = "_esp")]
pub mod esp;
pub mod http;
pub mod mdns;

use core::fmt::Write;

use heapless::String;
use iotkit_config::{Setting, keys};
use serde::{Deserialize, Serialize};

/// The Wi-Fi network to join, kept in flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiCredentials {
    pub ssid: String<32>,
    /// Empty for an open network.
    pub password: String<64>,
}

impl Setting for WifiCredentials {
    const KEY: u16 = keys::WIFI;
    const VERSION: u8 = 1;
}

impl WifiCredentials {
    /// `None` if the SSID is empty or a field is too long.
    pub fn new(ssid: &str, password: &str) -> Option<Self> {
        if ssid.is_empty() {
            return None;
        }
        Some(Self { ssid: ssid.try_into().ok()?, password: password.try_into().ok()? })
    }
}

/// `iotkit-` and the last two bytes of the MAC address, e.g. `iotkit-aefc`.
pub fn hostname(mac: [u8; 6]) -> String<16> {
    let mut name = String::new();
    let _ = write!(name, "iotkit-{:02x}{:02x}", mac[4], mac[5]);
    name
}

/// The IPv6 link-local address of a MAC address (modified EUI-64), as Matter expects.
pub fn link_local_ipv6(mac: [u8; 6]) -> core::net::Ipv6Addr {
    let w = |a: u8, b: u8| u16::from_be_bytes([a, b]);
    core::net::Ipv6Addr::new(
        0xfe80,
        0,
        0,
        0,
        w(mac[0] ^ 0x02, mac[1]),
        w(mac[2], 0xff),
        w(0xfe, mac[3]),
        w(mac[4], mac[5]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_local() {
        let ip = link_local_ipv6([0x98, 0xa3, 0x16, 0xb1, 0xae, 0xfc]);
        assert_eq!(ip, "fe80::9aa3:16ff:feb1:aefc".parse::<core::net::Ipv6Addr>().unwrap());
    }

    #[test]
    fn hostnames() {
        assert_eq!(hostname([0x98, 0xa3, 0x16, 0xb1, 0xae, 0xfc]), "iotkit-aefc");
    }

    #[test]
    fn credentials() {
        assert!(WifiCredentials::new("", "x").is_none());
        assert!(WifiCredentials::new(&"s".repeat(33), "").is_none());
        assert!(WifiCredentials::new("s", &"p".repeat(65)).is_none());
        let c = WifiCredentials::new("Pixel", "secret").unwrap();
        assert_eq!((c.ssid.as_str(), c.password.as_str()), ("Pixel", "secret"));
    }
}
