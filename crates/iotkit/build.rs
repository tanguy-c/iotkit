//! - `IOTKIT_HOSTNAME` (the host name prefix, `iotkit-aefc`) and `IOTKIT_TOPIC_PREFIX` (the MQTT
//!   topic prefix, `iotkit/<host>/...`) default to `iotkit`; a project sets them in
//!   `.cargo/config.toml` `[env]`.
//! - `IOTKIT_UPDATE_TRIAL_BOOTS` (3) and `IOTKIT_UPDATE_TIMEOUT` (1800 s): how long an update gets
//!   to prove itself before it is rolled back (`ota`), set the same way.
//! - `IOTKIT_ICD_POLL_MS` (5000): how often a Matter device on battery listens (`matter-icd`).
//! - `IOTKIT_UPDATE_TIME` (`02:00`): when a deep-sleeping device over the ESPHome API installs
//!   updates by itself until set in Home Assistant: `HH:MM` on the half hour, or `wake`.
//! - `cfg(esp_wifi)`: a chip and the `wifi` feature, for the modules that run on the Wi-Fi side.
//! - `cfg(esp_ota)`: a chip and a way to receive updates (`wifi` or `matter-ota`): `ota::esp`.
//! - `cfg(esp_scheduled)`: updates installed at a set time, on a deep-sleeping device over the
//!   ESPHome API (`deep-sleep`, `esphome-api`, `ha-update`): Home Assistant gives the time.
//! - `cfg(esp_stage)`: images kept downloaded until applied (`matter-ota`, or `esp_scheduled`).

fn main() {
    for var in ["IOTKIT_HOSTNAME", "IOTKIT_TOPIC_PREFIX"] {
        println!("cargo:rerun-if-env-changed={var}");
        if std::env::var_os(var).is_none() {
            println!("cargo:rustc-env={var}=iotkit");
        }
    }
    for (var, default) in [
        ("IOTKIT_UPDATE_TRIAL_BOOTS", "3"),
        ("IOTKIT_UPDATE_TIMEOUT", "1800"),
        ("IOTKIT_ICD_POLL_MS", "5000"),
        ("IOTKIT_UPDATE_TIME", "02:00"),
    ] {
        println!("cargo:rerun-if-env-changed={var}");
        if std::env::var_os(var).is_none() {
            println!("cargo:rustc-env={var}={default}");
        }
    }
    println!("cargo::rustc-check-cfg=cfg(esp_wifi)");
    println!("cargo::rustc-check-cfg=cfg(esp_ota)");
    println!("cargo::rustc-check-cfg=cfg(esp_scheduled)");
    println!("cargo::rustc-check-cfg=cfg(esp_stage)");
    let on = |feature: &str| std::env::var_os(format!("CARGO_FEATURE_{feature}")).is_some();
    if on("_ESP") && on("WIFI") {
        println!("cargo::rustc-cfg=esp_wifi");
    }
    if on("_ESP") && (on("WIFI") || on("MATTER_OTA")) {
        println!("cargo::rustc-cfg=esp_ota");
    }
    let scheduled = on("_ESP") && on("WIFI") && on("DEEP_SLEEP") && on("ESPHOME_API") && on("HA_UPDATE");
    if scheduled {
        println!("cargo::rustc-cfg=esp_scheduled");
    }
    if on("_ESP") && (on("MATTER_OTA") || scheduled) {
        println!("cargo::rustc-cfg=esp_stage");
    }
}
