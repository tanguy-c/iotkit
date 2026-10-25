//! Matter on battery (`matter-icd`): an Intermittently Connected Device with a short idle time
//! (SIT). Controllers read its intervals in DNS-SD (`SII`, `SAI`) and wait for it accordingly.
//!
//! Over Thread the device is a sleepy end device: its radio is off except when it polls its
//! parent for queued frames, every [`POLL_MS`] (every [`ACTIVE_POLL_MS`] for a while after data
//! went either way), and the CPU light-sleeps whenever nothing runs ([`init!`](crate::init)).
//! Over Wi-Fi (the radio's power save, `net::esp::power_save`) it waits for an esp-radio release.

#[cfg(feature = "matter-thread")]
mod thread;
#[cfg(feature = "matter-thread")]
pub(crate) use thread::{SleepyThread, Stack};

/// The idle poll period (the advertised `SII`): a command waits up to this long. Set with
/// `IOTKIT_ICD_POLL_MS` at build time (500 to 15000: a short-idle-time ICD), 5000 by default.
pub const POLL_MS: u32 = match crate::ota::number(env!("IOTKIT_ICD_POLL_MS")) {
    Some(n @ 500..=15_000) => n,
    _ => panic!("IOTKIT_ICD_POLL_MS: 500 to 15000 milliseconds"),
};

/// The poll period while active (the advertised `SAI`).
pub const ACTIVE_POLL_MS: u32 = 300;
