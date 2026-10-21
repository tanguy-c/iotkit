//! The device model over the ESPHome native API (`esphome-api` feature): Home Assistant's
//! ESPHome integration finds the device over mDNS and controls it directly, with no broker.
//!
//! The API is encrypted (`Noise_NNpsk0_25519_ChaChaPoly_SHA256`) with a key of the device's own,
//! made at first start; `api-key` on the console shows it, for Home Assistant to ask. One client
//! at a time, on TCP port 6053.
//!
//! [`proto`] (the protobuf subset), [`noise`] and [`api`] (the messages) are host-testable; the
//! `esp` module (chip feature) runs the server. The messages follow ESPHome's `api.proto` at the
//! version in [`api::VERSION`]: to move to a newer one, check its changelog for the messages
//! [`api`] writes and reads, then raise the version.

pub mod api;
#[cfg(esp_wifi)]
pub mod esp;
pub mod noise;
pub mod proto;

use crate::config::{Setting, keys};
use serde::{Deserialize, Serialize};

/// The API's encryption key, kept in flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKey(pub [u8; 32]);

impl Setting for ApiKey {
    const KEY: u16 = keys::ESPHOME_API_KEY;
    const VERSION: u8 = 1;
}

/// A key in base64, as Home Assistant and ESPHome show it.
pub type KeyText = heapless::String<44>;

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

impl ApiKey {
    /// In base64.
    pub fn to_text(&self) -> KeyText {
        let mut text = KeyText::new();
        for chunk in self.0.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
            for i in 0..4 {
                let c = if i <= chunk.len() { BASE64[(n >> (18 - 6 * i) & 63) as usize] } else { b'=' };
                let _ = text.push(char::from(c));
            }
        }
        text
    }

    /// From base64; `None` unless it is 32 bytes.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.as_bytes();
        if text.len() != 44 || text[43] != b'=' {
            return None;
        }
        let mut key = [0; 32];
        let mut bits = 0u32;
        for (i, &c) in text[..43].iter().enumerate() {
            bits = bits << 6 | BASE64.iter().position(|&b| b == c)? as u32;
            if i % 4 == 3 {
                key[i / 4 * 3..i / 4 * 3 + 3].copy_from_slice(&bits.to_be_bytes()[1..]);
            }
        }
        // The last group: 3 characters for 2 bytes, 2 unused bits that must be 0.
        if bits & 3 != 0 {
            return None;
        }
        key[30..].copy_from_slice(&((bits >> 2) as u16).to_be_bytes());
        Some(Self(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_text() {
        let key = ApiKey(core::array::from_fn(|i| i as u8));
        assert_eq!(key.to_text().as_str(), "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=");
        assert_eq!(ApiKey::parse(&key.to_text()), Some(key));
        let max = ApiKey([0xff; 32]);
        assert_eq!(ApiKey::parse(&max.to_text()), Some(max));
        for bad in [
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",   // no padding
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh!=",  // not base64
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9=",  // unused bits set
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=A", // too long
        ] {
            assert_eq!(ApiKey::parse(bad), None, "{bad}");
        }
    }
}
