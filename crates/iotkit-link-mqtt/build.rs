//! `IOTKIT_TOPIC_PREFIX` (the MQTT topic prefix, `iotkit/<host>/...`) defaults to `iotkit`; a project sets it in
//! `.cargo/config.toml` `[env]`.

fn main() {
    println!("cargo:rerun-if-env-changed=IOTKIT_TOPIC_PREFIX");
    if std::env::var_os("IOTKIT_TOPIC_PREFIX").is_none() {
        println!("cargo:rustc-env=IOTKIT_TOPIC_PREFIX=iotkit");
    }
}
