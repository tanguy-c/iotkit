//! A device's firmware in one call: [`start`] runs every iotkit service in the order they need,
//! and the app keeps only its own hardware and entities.
//!
//! ```ignore
//! static OTA_KEY: PublicKey = iotkit::public_key(include_str!("../keys/ota.pub"));
//! static LED: Light = Light::new("led", "LED");
//! static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
//!     .manufacturer("Me")
//!     .model("Desk lamp")
//!     .entities(&[&LED]);
//!
//! // in `main`:
//! iotkit::init!(p);
//! let app = iotkit::start(spawner, Board { wifi: p.WIFI, .. }, &CONFIG).await;
//! ```
//!
//! The full, compiled version: `apps/devkit/src/main.rs`.
//!
//! What [`start`] does, in order: takes the crash record and counts the boot; guards and handles
//! a pending update (trial boots, rollback); starts the watchdog; joins Wi-Fi (or opens the setup
//! window: access point, Improv over Bluetooth and serial); answers `<host>.local`; starts MQTT
//! (Home Assistant discovery) when a broker is set, and Matter with the `matter-wifi` feature;
//! serves OTA pushes; watches the BOOT button; runs the USB console. Once every health check
//! passes (the kit's and [`Config::checks`]), the boot is stable and an update is confirmed.

#![no_std]
#![warn(missing_docs)]
// The Thread stack's nested futures go deeper than the default limit.
#![recursion_limit = "256"]

#[cfg(all(
    feature = "wifi",
    not(any(feature = "esp32c6", feature = "esp32c3", feature = "esp32c5", feature = "esp32s3", feature = "esp32"))
))]
compile_error!(
    "iotkit: enable one chip feature: esp32c6, esp32c3, esp32c5, esp32s3 or esp32 (esp32h2: Thread only, without `wifi`)"
);
#[cfg(any(
    all(feature = "esp32c6", any(feature = "esp32c3", feature = "esp32c5", feature = "esp32s3", feature = "esp32h2")),
    all(feature = "esp32c3", any(feature = "esp32c5", feature = "esp32s3", feature = "esp32h2")),
    all(feature = "esp32c5", any(feature = "esp32s3", feature = "esp32h2")),
    all(feature = "esp32s3", feature = "esp32h2"),
    all(
        feature = "esp32",
        any(feature = "esp32c6", feature = "esp32c3", feature = "esp32c5", feature = "esp32s3", feature = "esp32h2")
    ),
))]
compile_error!("iotkit: enable only one chip feature");
#[cfg(all(feature = "esp32c5", feature = "matter-wifi"))]
compile_error!("iotkit: `matter-wifi` is not supported on the ESP32-C5 yet (no TRNG in esp-hal 1.2)");
// Matter keeps its sessions: on battery it stays reachable as an ICD (`matter-icd`) instead.
#[cfg(all(feature = "deep-sleep", feature = "matter-wifi"))]
compile_error!("iotkit: `deep-sleep` is not for Matter: `matter-icd` keeps a Matter device on battery");
#[cfg(all(feature = "matter-icd", not(any(feature = "matter-thread", feature = "matter-wifi"))))]
compile_error!("iotkit: `matter-icd` needs `matter-thread`");
// esp-radio 1.0.0-beta.1 stops the Wi-Fi power controller's clock at every doze on the C6, so
// the radio misses every beacon in power save and the device drops off the network (fixed on
// esp-hal `main`). Lift with the esp-radio release that has the fix (`net::esp::power_save`).
#[cfg(all(feature = "matter-icd", feature = "matter-wifi"))]
compile_error!(
    "iotkit: `matter-icd` over Wi-Fi waits for an esp-radio release that keeps the radio's power save working: use `matter-thread`, or deep sleep over MQTT or the ESPHome API"
);
// Matter over Wi-Fi needs about 21 KB more internal RAM than the ESP32-S3 has left (PSRAM
// support would fix it; not done yet).
// Not tried on the original ESP32: Matter (RAM, rs-matter support) and deep sleep (its code
// reads the USB-Serial-JTAG peripheral the ESP32 does not have).
#[cfg(all(feature = "esp32", any(feature = "matter-wifi", feature = "matter-thread")))]
compile_error!("iotkit: Matter is not supported on the original ESP32 (use MQTT, or a C6)");
// esp-radio 1.0.0-beta.1's ESP32 Bluetooth controller needs ROM symbols no linker script
// provides yet.
#[cfg(all(feature = "esp32", feature = "improv-ble"))]
compile_error!(
    "iotkit: `improv-ble` does not link on the original ESP32 yet: `default-features = false`, then the features to keep"
);
#[cfg(all(feature = "esp32", feature = "deep-sleep"))]
compile_error!("iotkit: `deep-sleep` is not supported on the original ESP32 yet");
// A wake-up is a restart: every state would be read and written to flash at each one.
#[cfg(all(feature = "remember", feature = "deep-sleep"))]
compile_error!("iotkit: `remember` is not for `deep-sleep` devices yet");
#[cfg(all(feature = "esp32s3", feature = "matter-wifi"))]
compile_error!("iotkit: `matter-wifi` does not fit in the ESP32-S3's internal RAM yet (use MQTT, or a C6)");
// Matter over Wi-Fi needs about 24 KB more static RAM than the ESP32-C3 has.
#[cfg(all(feature = "esp32c3", feature = "matter-wifi"))]
compile_error!("iotkit: `matter-wifi` does not fit in the ESP32-C3's RAM (use MQTT, or a C6/C5/S3)");

#[cfg(all(feature = "esphome-api", feature = "_esp", not(feature = "wifi")))]
compile_error!("iotkit: `esphome-api` needs `wifi`");
#[cfg(all(feature = "esp32h2", feature = "wifi"))]
compile_error!("iotkit: the ESP32-H2 has no Wi-Fi: `default-features = false` with `matter-thread`");
#[cfg(all(feature = "matter-ota", not(any(feature = "matter-wifi", feature = "matter-thread"))))]
compile_error!("iotkit: `matter-ota` needs `matter-wifi` or `matter-thread`");
// One esp-radio build: devkit's Wi-Fi and Thread's 802.15.4 do not mix.
#[cfg(all(feature = "matter-thread", feature = "wifi"))]
compile_error!("iotkit: `matter-thread` is without Wi-Fi: `default-features = false`");

// With no setup path, only a network given at build time can be joined.
#[cfg(all(feature = "wifi", not(any(feature = "improv-ble", feature = "softap", feature = "improv-serial"))))]
const _: () = assert!(
    option_env!("WIFI_SSID").is_some(),
    "iotkit: no setup path (improv-ble, softap, improv-serial): set WIFI_SSID at build time"
);

// Matter and the update entity allocate (the heap set up by `init!`).
extern crate alloc;

/// `core::sync::atomic`, or `portable-atomic` on chips without compare-and-swap (ESP32-C3).
#[cfg(all(feature = "_esp", not(feature = "portable-atomic")))]
pub(crate) use core::sync::atomic;
#[cfg(all(feature = "_esp", feature = "portable-atomic"))]
pub(crate) use portable_atomic as atomic;

pub use iotkit_core::{image, model, runtime, time};

#[cfg(esp_scheduled)]
pub mod clock;
pub mod components;
pub mod config;
pub mod diag;
#[cfg(feature = "esphome-api")]
pub mod esphome;
#[cfg(feature = "_esp")]
pub mod health;
#[cfg(feature = "_matter")]
pub mod matter;
pub mod mqtt;
pub mod net;
pub mod ota;
pub mod provision;
#[cfg(feature = "remember")]
pub mod remember;
pub mod status;

#[cfg(any(esp_wifi, all(feature = "_esp", feature = "matter-thread")))]
mod app;
#[cfg(any(esp_wifi, all(feature = "_esp", feature = "matter-thread")))]
pub use app::*;

pub use components::Active;
pub use config::Setting;
#[cfg(feature = "_esp")]
pub use config::esp::SharedStore;
pub use diag::{Boot, BootStats, TaskWatch};
pub use image::{PublicKey, public_key};
pub use model::*;
#[cfg(esp_wifi)]
pub use net::esp::Net;
pub use ota::Firmware;
pub use runtime::{Check, Health};

#[cfg(feature = "_esp")]
#[doc(hidden)]
pub mod __private {
    pub use {esp_alloc, esp_bootloader_esp_idf, esp_hal, esp_rtos};
}
