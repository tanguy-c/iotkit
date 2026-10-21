//! HTTP: responses from an update server, requests to the device (OTA endpoint, setup page).
#![no_main]

use iotkit::{net::http, provision::portal};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = http::parse_response_head(data);
    let _ = portal::parse_form(data);
    if let Ok(text) = core::str::from_utf8(data) {
        if let Ok(url) = http::Url::parse(text) {
            let mut out = [0; 512];
            let _ = http::write_get(&url, 0, &mut out);
        }
        if let Some((method, target)) = http::parse_request_line(text) {
            let _ = portal::route(method, http::target_path(target));
            let _ = http::query_param(target, "manifest");
        }
        let _ = portal::content_length(text);
    }
});
