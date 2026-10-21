//! MQTT packets from the broker (a stream: several packets, or part of one).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut rest = data;
    while let Ok(Some((_, used))) = iotkit::mqtt::codec::parse(rest) {
        assert!(used > 0 && used <= rest.len());
        rest = &rest[used..];
    }
});
