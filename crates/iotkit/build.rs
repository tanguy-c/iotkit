//! - `IOTKIT_HOSTNAME` (the host name prefix, `iotkit-aefc`) and `IOTKIT_TOPIC_PREFIX` (the MQTT
//!   topic prefix, `iotkit/<host>/...`) default to `iotkit`; a project sets them in
//!   `.cargo/config.toml` `[env]`.
//! - `IOTKIT_UPDATE_TRIAL_BOOTS` (3) and `IOTKIT_UPDATE_TIMEOUT` (1800 s): how long an update gets
//!   to prove itself before it is rolled back (`ota`), set the same way.
//! - `cfg(esp_wifi)`: a chip and the `wifi` feature, for the modules that run on the Wi-Fi side.

fn main() {
    for var in ["IOTKIT_HOSTNAME", "IOTKIT_TOPIC_PREFIX"] {
        println!("cargo:rerun-if-env-changed={var}");
        if std::env::var_os(var).is_none() {
            println!("cargo:rustc-env={var}=iotkit");
        }
    }
    for (var, default) in [("IOTKIT_UPDATE_TRIAL_BOOTS", "3"), ("IOTKIT_UPDATE_TIMEOUT", "1800")] {
        println!("cargo:rerun-if-env-changed={var}");
        if std::env::var_os(var).is_none() {
            println!("cargo:rustc-env={var}={default}");
        }
    }
    println!("cargo::rustc-check-cfg=cfg(esp_wifi)");
    if std::env::var_os("CARGO_FEATURE__ESP").is_some() && std::env::var_os("CARGO_FEATURE_WIFI").is_some() {
        println!("cargo::rustc-cfg=esp_wifi");
    }
}
