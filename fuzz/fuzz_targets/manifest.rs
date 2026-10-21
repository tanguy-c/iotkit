//! Update manifests and image headers: parsed before the signature is checked.
#![no_main]

use iotkit_core::image::{Manifest, Version, app};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(manifest) = Manifest::parse(data) {
        let _ = manifest.verify(&[7; 32]);
        let _ = manifest.check_app(data, app::ESP32C6);
    }
    if let Ok(text) = core::str::from_utf8(data) {
        let _ = Version::parse(text);
    }
});
