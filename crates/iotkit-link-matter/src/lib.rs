//! The device model ([`iotkit_model`]) over Matter.
//!
//! Each entity with a Matter equivalent ([`matter_kind`]) becomes an endpoint, numbered from 1
//! in declaration order:
//!
//! | Entity | Matter device type | Clusters |
//! |---|---|---|
//! | `Light` | Extended Color Light | On/Off, Level Control, Color Control |
//! | `Switch` | On/Off Plug-in Unit | On/Off |
//! | `Sensor` with class `temperature` | Temperature Sensor | Temperature Measurement |
//! | `BinarySensor` | Contact Sensor | Boolean State |
//!
//! Buttons and other sensors are MQTT-only for now.
//!
//! `wifi`: Matter runs on `iotkit-net`'s network (joined with iotkit's provisioning), next to
//! MQTT and OTA, and is commissioned on that network.
//!
//! `thread`: Matter over Thread, commissioned over BLE ([`thread`]).

#![no_std]
// The Thread stack's nested futures go deeper than the default limit.
#![recursion_limit = "256"]

// esp-hal 1.2 has no true random number generator on the ESP32-C5, and Matter needs one.
#[cfg(feature = "esp32c5")]
compile_error!("iotkit-link-matter: Matter is not supported on the ESP32-C5 yet (no TRNG in esp-hal 1.2)");

#[cfg(feature = "_esp")]
extern crate alloc;

#[cfg(feature = "_esp")]
mod clusters;
#[cfg(feature = "_esp")]
mod common;
#[cfg(feature = "_esp")]
mod endpoints;
#[cfg(feature = "_esp")]
mod handler;
#[cfg(feature = "thread")]
pub mod thread;
// Links the C library functions OpenThread and mbedtls call.
#[cfg(feature = "thread")]
use tinyrlibc as _;
#[cfg(feature = "wifi")]
pub mod wifi;

use iotkit_config::{Setting, keys};
use iotkit_model::{Kind, Meta};

/// The Matter device an entity becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatterKind {
    ExtendedColorLight,
    OnOffPlug,
    TemperatureSensor,
    ContactSensor,
}

/// `None`: the entity has no Matter equivalent (it stays MQTT-only).
pub fn matter_kind(meta: &Meta) -> Option<MatterKind> {
    match meta.kind {
        Kind::Light => Some(MatterKind::ExtendedColorLight),
        Kind::Switch => Some(MatterKind::OnOffPlug),
        Kind::Sensor if meta.device_class == Some("temperature") => Some(MatterKind::TemperatureSensor),
        Kind::BinarySensor => Some(MatterKind::ContactSensor),
        _ => None,
    }
}

/// Matter's Level Control (1..=254) from a brightness (0..=255).
pub fn level_from_brightness(brightness: u8) -> u8 {
    ((u16::from(brightness) * 254 + 127) / 255).clamp(1, 254) as u8
}

/// A brightness (1..=255) from Matter's Level Control (0..=254; 0 counts as the minimum).
pub fn brightness_from_level(level: u8) -> u8 {
    ((u16::from(level.clamp(1, 254)) * 255 + 127) / 254) as u8
}

/// Temperature Measurement's `MeasuredValue`: hundredths of a degree Celsius. `unit`: the
/// sensor's (`°C` or `°F`; others are taken as °C).
pub fn temperature_hundredths(value: f32, unit: Option<&str>) -> Option<i16> {
    let celsius = if unit == Some("°F") { (value - 32.0) * 5.0 / 9.0 } else { value };
    let hundredths = celsius * 100.0;
    // MeasuredValue range: -273.15 .. 327.67 °C.
    (hundredths.is_finite() && (-27315.0..=32767.0).contains(&hundredths))
        .then(|| (hundredths + 0.5f32.copysign(hundredths)) as i16)
}
use serde::{Deserialize, Serialize};

/// The device's Matter setup code, drawn at its first boot and kept in flash, so the code on
/// a label stays valid across restarts, updates and pairing resets (not a factory reset).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterSetup {
    /// 1..=99_999_998, not one of [`INVALID_PASSCODES`].
    pub passcode: u32,
    /// 12 bits.
    pub discriminator: u16,
}

impl Setting for MatterSetup {
    const KEY: u16 = keys::MATTER_SETUP;
    const VERSION: u8 = 1;
}

/// Passcodes the Matter specification forbids (5.1.7.1).
pub const INVALID_PASSCODES: [u32; 12] =
    [0, 11111111, 22222222, 33333333, 44444444, 55555555, 66666666, 77777777, 88888888, 99999999, 12345678, 87654321];

pub fn valid_passcode(passcode: u32) -> bool {
    (1..=99_999_998).contains(&passcode) && !INVALID_PASSCODES.contains(&passcode)
}

impl MatterSetup {
    /// A setup code from random numbers; `None` if they give an invalid passcode (draw again).
    pub fn from_random(a: u32, b: u32) -> Option<Self> {
        let passcode = a % 99_999_998 + 1;
        valid_passcode(passcode).then_some(Self { passcode, discriminator: (b & 0xfff) as u16 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use iotkit_model::{BinarySensor, Button, Entity, Light, Sensor, Switch};

    #[test]
    fn kinds() {
        static L: Light = Light::new("l", "L");
        static S: Switch = Switch::new("s", "S");
        static T: Sensor = Sensor::new("t", "T").device_class("temperature");
        static U: Sensor = Sensor::new("u", "U").unit("s");
        static B: BinarySensor = BinarySensor::new("b", "B");
        static P: Button = Button::new("p", "P");
        assert_eq!(matter_kind(L.meta()), Some(MatterKind::ExtendedColorLight));
        assert_eq!(matter_kind(S.meta()), Some(MatterKind::OnOffPlug));
        assert_eq!(matter_kind(T.meta()), Some(MatterKind::TemperatureSensor));
        assert_eq!(matter_kind(U.meta()), None);
        assert_eq!(matter_kind(B.meta()), Some(MatterKind::ContactSensor));
        assert_eq!(matter_kind(P.meta()), None);
    }

    #[test]
    fn levels() {
        assert_eq!(level_from_brightness(255), 254);
        assert_eq!(level_from_brightness(0), 1, "MinLevel");
        assert_eq!(brightness_from_level(254), 255);
        assert_eq!(brightness_from_level(0), 1);
        for b in 1..=255u8 {
            let back = brightness_from_level(level_from_brightness(b));
            assert!(back.abs_diff(b) <= 1, "{b} -> {back}");
        }
    }

    #[test]
    fn temperatures() {
        assert_eq!(temperature_hundredths(21.46, Some("°C")), Some(2146));
        assert_eq!(temperature_hundredths(-5.5, None), Some(-550));
        assert_eq!(temperature_hundredths(212.0, Some("°F")), Some(10000));
        assert_eq!(temperature_hundredths(f32::NAN, None), None);
        assert_eq!(temperature_hundredths(400.0, None), None);
    }

    #[test]
    fn passcodes() {
        assert!(valid_passcode(20202021));
        for bad in [0, 11111111, 12345678, 87654321, 99_999_999, 100_000_000] {
            assert!(!valid_passcode(bad), "{bad}");
        }
        let s = MatterSetup::from_random(u32::MAX, 0xffff_ffff).unwrap();
        assert!(valid_passcode(s.passcode) && s.discriminator == 0xfff);
        assert_eq!(MatterSetup::from_random(11111110, 0), None, "lands on 11111111");
        for i in 0..1000u32 {
            if let Some(s) = MatterSetup::from_random(i.wrapping_mul(2_654_435_761), i) {
                assert!(valid_passcode(s.passcode) && s.discriminator < 4096);
            }
        }
    }
}
