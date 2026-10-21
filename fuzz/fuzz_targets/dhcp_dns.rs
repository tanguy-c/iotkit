//! UDP from anyone in range of the setup access point (DHCP, captive DNS) or on the network (mDNS).
#![no_main]

use iotkit::{net::mdns, provision::{dhcp::Leases, dns}};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut leases = Leases::new();
    let mut out = [0; 576];
    if let Some(n) = leases.respond(data, &mut out) {
        assert!(n <= out.len());
    }
    let mut out = [0; 512];
    if let Some(n) = dns::answer(data, [192, 168, 4, 1], &mut out) {
        assert!(n <= out.len());
    }
    let mut out = [0; 512];
    let _ = mdns::answer(data, "iotkit-aefc", [192, 168, 1, 2], &mut out);
});
