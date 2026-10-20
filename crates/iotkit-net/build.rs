//! `IOTKIT_HOSTNAME` (the host name prefix, `iotkit-aefc`) defaults to `iotkit`; a project sets it in
//! `.cargo/config.toml` `[env]`.

fn main() {
    println!("cargo:rerun-if-env-changed=IOTKIT_HOSTNAME");
    if std::env::var_os("IOTKIT_HOSTNAME").is_none() {
        println!("cargo:rustc-env=IOTKIT_HOSTNAME=iotkit");
    }
}
