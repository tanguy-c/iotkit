//! - `IOTKIT_HOSTNAME` (the host name prefix, `iotkit-aefc`) and `IOTKIT_TOPIC_PREFIX` (the MQTT
//!   topic prefix, `iotkit/<host>/...`) default to `iotkit`; a project sets them in
//!   `.cargo/config.toml` `[env]`.
//! - `IOTKIT_UPDATE_TRIAL_BOOTS` (3) and `IOTKIT_UPDATE_TIMEOUT` (1800 s): how long an update gets
//!   to prove itself before it is rolled back (`ota`), set the same way.
//! - `IOTKIT_ICD_POLL_MS` (5000): how often a Matter device on battery listens (`matter-icd`).
//! - `IOTKIT_AUTO_UPDATE` (`on` or `off`): whether updates install without a click. Unset, as
//!   before: on where nobody can click (a deep-sleeping device over the ESPHome API; Matter's
//!   daily checks of its default providers), off on the other Wi-Fi devices. `on` gives those
//!   (`ha-update`) the *Auto-update* switch; `off` leaves Matter to updates its controller
//!   announces.
//! - `IOTKIT_UPDATE_TIME` (`02:00`): when a device over the ESPHome API installs updates by itself
//!   until set in Home Assistant: `HH:MM` on the half hour, or `wake` (at once).
//! - `cfg(esp_wifi)`: a chip and the `wifi` feature, for the modules that run on the Wi-Fi side.
//! - `cfg(esp_ota)`: a chip and a way to receive updates (`wifi` or `matter-ota`): `ota::esp`.
//! - `cfg(esp_auto)`: updates installed without a click over Wi-Fi (`ha-update`, and
//!   `IOTKIT_AUTO_UPDATE=on` or `esp_scheduled`).
//! - `cfg(esp_scheduled)`: at a set time, over the ESPHome API (Home Assistant gives the time):
//!   `esp_auto` and `esphome-api`. Always on a deep-sleeping device over the API, which nobody
//!   can click.
//! - `cfg(auto_update_off)`: `IOTKIT_AUTO_UPDATE=off`.
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
    println!("cargo::rustc-check-cfg=cfg(esp_auto)");
    println!("cargo::rustc-check-cfg=cfg(auto_update_off)");
    println!("cargo:rerun-if-env-changed=IOTKIT_AUTO_UPDATE");
    let auto = match std::env::var("IOTKIT_AUTO_UPDATE").as_deref() {
        Err(_) | Ok("") => None,
        Ok("on") => Some(true),
        Ok("off") => Some(false),
        Ok(other) => panic!("IOTKIT_AUTO_UPDATE: `on` or `off`, not `{other}`"),
    };
    if auto == Some(false) {
        println!("cargo::rustc-cfg=auto_update_off");
    }
    let updates = on("_ESP") && on("WIFI") && on("HA_UPDATE");
    // Nobody can click *Install* on a sleeping device over the API: its switch is always there.
    let asleep = on("DEEP_SLEEP") && on("ESPHOME_API");
    if updates && (auto == Some(true) || asleep) {
        println!("cargo::rustc-cfg=esp_auto");
    }
    let scheduled = updates && (auto == Some(true) || asleep) && on("ESPHOME_API");
    if scheduled {
        println!("cargo::rustc-cfg=esp_scheduled");
    }
    if on("_ESP") && (on("MATTER_OTA") || scheduled) {
        println!("cargo::rustc-cfg=esp_stage");
    }
}
