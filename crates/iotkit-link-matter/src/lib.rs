//! The device model ([`iotkit_model`]) over Matter.
//!
//! Each entity with a Matter equivalent ([`matter_kind`]) becomes an endpoint, numbered from 1
//! in declaration order:
//!
//! | Entity | Matter device type | Main clusters |
//! |---|---|---|
//! | `Light` | Extended Color Light | On/Off, Level Control, Color Control |
//! | `Switch` | On/Off Plug-in Unit | On/Off |
//! | `Sensor` class `temperature`, `humidity`, `illuminance`, `pressure` | Temperature, Humidity, Light, Pressure Sensor | the Measurement cluster |
//! | `Sensor` class `carbon_dioxide`, `carbon_monoxide`, `nitrogen_dioxide`, `ozone`, `pm1`, `pm25`, `pm10`, `volatile_organic_compounds` | Air Quality Sensor | Air Quality, Concentration Measurement |
//! | `Sensor` class `battery` | Power Source | Power Source (battery) |
//! | `BinarySensor` class `motion`/`occupancy`, `moisture`, `smoke`, `carbon_monoxide`, others | Occupancy Sensor, Water Leak Detector, Smoke CO Alarm, Contact Sensor | |
//! | `Event` with button types (`press`, `long_press`, `double_press`, ...) | Generic Switch | Switch (events) |
//! | `Cover`, `Valve`, `Lock`, `Fan`, `Climate`, `Select` | Window Covering, Water Valve, Door Lock, Fan, Thermostat, Mode Select | the matching cluster |
//!
//! Every endpoint also has Identify (`wait_identify`) and a Fixed Label with the entity's name
//! (Home Assistant names the entity after it). Buttons, numbers and text sensors are MQTT-only,
//! like sensors in a unit Matter does not take.
//!
//! `wifi`: Matter runs on `iotkit-net`'s network (joined with iotkit's provisioning), next to
//! MQTT and OTA, and is commissioned on that network.
//!
//! `thread`: Matter over Thread, commissioned over BLE (the `thread` module).

#![no_std]
#![warn(missing_docs)]
// The Thread stack's nested futures go deeper than the default limit.
#![recursion_limit = "256"]

// esp-hal 1.2 has no true random number generator on the ESP32-C5, and Matter needs one.
#[cfg(feature = "esp32c5")]
compile_error!("iotkit-link-matter: Matter is not supported on the ESP32-C5 yet (no TRNG in esp-hal 1.2)");

#[cfg(feature = "_esp")]
extern crate alloc;

#[cfg(feature = "_esp")]
mod appliances;
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
pub mod map;
#[cfg(feature = "wifi")]
pub mod wifi;

use iotkit_config::{Setting, keys};
use iotkit_model::{DeviceClass, Entity, Kind, Spec, Unit};

/// The Matter device an entity becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatterKind {
    /// A light: on/off, level, color.
    ExtendedColorLight,
    /// A switch.
    OnOffPlug,
    /// Temperature.
    TemperatureSensor,
    /// Relative humidity.
    HumiditySensor,
    /// Illuminance.
    LightSensor,
    /// Pressure.
    PressureSensor,
    /// An open/closed reading (the default for binary sensors).
    ContactSensor,
    /// Motion or occupancy.
    OccupancySensor,
    /// Moisture.
    WaterLeakDetector,
    /// Smoke.
    SmokeAlarm,
    /// Carbon monoxide.
    CoAlarm,
    /// An air quality reading.
    AirQualitySensor(Pollutant),
    /// A battery level: the node's power source.
    Battery,
    /// A button's events.
    GenericSwitch,
    /// A cover.
    WindowCovering,
    /// A valve.
    WaterValve,
    /// A lock.
    DoorLock,
    /// A fan.
    Fan,
    /// A climate entity.
    Thermostat,
    /// A select.
    ModeSelect,
}

/// What an air quality sensor measures (its Concentration Measurement cluster).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pollutant {
    /// Carbon dioxide.
    Co2,
    /// Carbon monoxide.
    Co,
    /// Nitrogen dioxide.
    No2,
    /// Ozone.
    O3,
    /// PM1 particles.
    Pm1,
    /// PM2.5 particles.
    Pm25,
    /// PM10 particles.
    Pm10,
    /// Volatile organic compounds.
    Tvoc,
}

/// `None`: the entity has no Matter equivalent (it stays MQTT-only): numbers, text sensors,
/// buttons, sensors of other classes or in a unit Matter does not take.
pub fn matter_kind(entity: &dyn Entity) -> Option<MatterKind> {
    let meta = entity.meta();
    let class = meta.device_class;
    match meta.kind {
        Kind::Light => Some(MatterKind::ExtendedColorLight),
        Kind::Switch => Some(MatterKind::OnOffPlug),
        Kind::Sensor => {
            let kind = match class? {
                DeviceClass::TEMPERATURE => MatterKind::TemperatureSensor,
                DeviceClass::HUMIDITY => MatterKind::HumiditySensor,
                DeviceClass::ILLUMINANCE => MatterKind::LightSensor,
                DeviceClass::PRESSURE => MatterKind::PressureSensor,
                DeviceClass::BATTERY => MatterKind::Battery,
                other => MatterKind::AirQualitySensor(map::pollutant(other)?),
            };
            // A temperature in an unknown unit is taken as °C, as before.
            if kind == MatterKind::TemperatureSensor {
                return Some(kind);
            }
            map::to_matter_unit(kind, meta.unit?, 0.0).map(|_| kind)
        }
        Kind::BinarySensor => Some(match class {
            Some(DeviceClass::MOTION | DeviceClass::OCCUPANCY | DeviceClass::PRESENCE) => MatterKind::OccupancySensor,
            Some(DeviceClass::MOISTURE) => MatterKind::WaterLeakDetector,
            Some(DeviceClass::SMOKE) => MatterKind::SmokeAlarm,
            Some(DeviceClass::CARBON_MONOXIDE | DeviceClass::GAS) => MatterKind::CoAlarm,
            _ => MatterKind::ContactSensor,
        }),
        Kind::Event => match entity.spec() {
            Spec::Event { types } if types.iter().any(|t| map::gesture(t).is_some()) => Some(MatterKind::GenericSwitch),
            _ => None,
        },
        Kind::Cover => Some(MatterKind::WindowCovering),
        Kind::Valve => Some(MatterKind::WaterValve),
        Kind::Lock => Some(MatterKind::DoorLock),
        Kind::Fan => Some(MatterKind::Fan),
        Kind::Climate => Some(MatterKind::Thermostat),
        Kind::Select => Some(MatterKind::ModeSelect),
        Kind::Button | Kind::Number | Kind::TextSensor | Kind::Update => None,
    }
}

/// Identify requests from a controller: show where the device is for this many seconds (0:
/// stop).
#[cfg(feature = "_esp")]
pub(crate) static IDENTIFY: embassy_sync::signal::Signal<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    u16,
> = embassy_sync::signal::Signal::new();

/// Waits until a controller asks the device to identify itself; the seconds to do so (blink a
/// LED, beep), 0 to stop.
#[cfg(feature = "_esp")]
pub async fn wait_identify() -> u16 {
    IDENTIFY.wait().await
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
pub fn temperature_hundredths(value: f32, unit: Option<Unit>) -> Option<i16> {
    let celsius = if unit == Some(Unit::FAHRENHEIT) { (value - 32.0) * 5.0 / 9.0 } else { value };
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

/// A passcode Matter accepts: 1 to 99999998, not in [`INVALID_PASSCODES`].
pub fn valid_passcode(passcode: u32) -> bool {
    (1..=99_999_998).contains(&passcode) && !INVALID_PASSCODES.contains(&passcode)
}

impl MatterSetup {
    /// A setup code from random numbers; `None` if they give an invalid passcode (draw again).
    pub fn from_random(a: u32, b: u32) -> Option<Self> {
        let passcode = a % 99_999_998 + 1;
        valid_passcode(passcode).then_some(Self { passcode, discriminator: (b & 0xfff) as u16 })
    }

    /// A setup code given as text (`MATTER_PASSCODE`, `MATTER_DISCRIMINATOR`); `None` if invalid.
    pub fn parse(passcode: &str, discriminator: &str) -> Option<Self> {
        let passcode = passcode.trim().parse().ok().filter(|p| valid_passcode(*p))?;
        let discriminator = discriminator.trim().parse().ok().filter(|d: &u16| *d < 4096)?;
        Some(Self { passcode, discriminator })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use iotkit_model::{BinarySensor, Button, Event, Light, Number, Sensor, Switch};

    #[test]
    fn kinds() {
        static L: Light = Light::new("l", "L");
        static S: Switch = Switch::new("s", "S");
        static T: Sensor = Sensor::new("t", "T").device_class(DeviceClass::TEMPERATURE);
        static U: Sensor = Sensor::new("u", "U").unit(Unit::SECOND);
        static B: BinarySensor = BinarySensor::new("b", "B");
        static P: Button = Button::new("p", "P");
        assert_eq!(matter_kind(&L), Some(MatterKind::ExtendedColorLight));
        assert_eq!(matter_kind(&S), Some(MatterKind::OnOffPlug));
        assert_eq!(matter_kind(&T), Some(MatterKind::TemperatureSensor));
        assert_eq!(matter_kind(&U), None);
        assert_eq!(matter_kind(&B), Some(MatterKind::ContactSensor));
        assert_eq!(matter_kind(&P), None);
        static H: Sensor = Sensor::new("h", "H").device_class(DeviceClass::HUMIDITY).unit(Unit::PERCENT);
        static HX: Sensor =
            Sensor::new("hx", "HX").device_class(DeviceClass::HUMIDITY).unit(Unit::GRAMS_PER_CUBIC_METER);
        static C: Sensor = Sensor::new("c", "C").device_class(DeviceClass::CARBON_DIOXIDE).unit(Unit::PPM);
        static M: BinarySensor = BinarySensor::new("m", "M").device_class(DeviceClass::MOTION);
        static W: BinarySensor = BinarySensor::new("w", "W").device_class(DeviceClass::MOISTURE);
        static E: Event = Event::new("e", "E", &["press", "long_press"]);
        static X: Event = Event::new("x", "X", &["spin"]);
        static N: Number = Number::new("n", "N", 0.0, 1.0, 0.1);
        assert_eq!(matter_kind(&H), Some(MatterKind::HumiditySensor));
        assert_eq!(matter_kind(&HX), None, "a unit Matter does not take");
        assert_eq!(matter_kind(&C), Some(MatterKind::AirQualitySensor(Pollutant::Co2)));
        assert_eq!(matter_kind(&M), Some(MatterKind::OccupancySensor));
        assert_eq!(matter_kind(&W), Some(MatterKind::WaterLeakDetector));
        assert_eq!(matter_kind(&E), Some(MatterKind::GenericSwitch));
        assert_eq!(matter_kind(&X), None, "no switch gesture");
        assert_eq!(matter_kind(&N), None);
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
        assert_eq!(temperature_hundredths(21.46, Some(Unit::CELSIUS)), Some(2146));
        assert_eq!(temperature_hundredths(-5.5, None), Some(-550));
        assert_eq!(temperature_hundredths(212.0, Some(Unit::FAHRENHEIT)), Some(10000));
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

    #[test]
    fn built_in_codes() {
        assert_eq!(
            MatterSetup::parse(" 79771663", "985 "),
            Some(MatterSetup { passcode: 79771663, discriminator: 985 })
        );
        assert_eq!(MatterSetup::parse("12345678", "985"), None, "forbidden passcode");
        assert_eq!(MatterSetup::parse("79771663", "4096"), None, "12 bits");
        assert_eq!(MatterSetup::parse("79771663", ""), None);
        assert_eq!(MatterSetup::parse("abc", "1"), None);
    }
}
