//! Firmware updates: download a signed image into the spare OTA slot, boot it on trial, keep it
//! once the health check passes, roll back otherwise.
//!
//! ```text
//! manifest (signed) ─► verify, no downgrade ─► image ─► app header, SHA-256 ─► switch slot
//! ─► restart ─► trial boot 1..=MAX_TRIAL_BOOTS ─► healthy: confirm   (else: previous slot)
//! ```
//!
//! Rollback does not rely on the bootloader: [`Pending`] in flash counts the trial boots, and
//! the firmware switches back itself (so the image only has to reach `esp::boot`). A bootloader
//! built with rollback works too.

#![no_std]
#![warn(missing_docs)]

/// `core::sync::atomic`, or `portable-atomic` on chips without compare-and-swap (ESP32-C3).
#[cfg(all(feature = "_esp", not(feature = "portable-atomic")))]
pub(crate) use core::sync::atomic;
#[cfg(all(feature = "_esp", feature = "portable-atomic"))]
pub(crate) use portable_atomic as atomic;

#[cfg(feature = "_esp")]
pub mod esp;

use iotkit_config::{Setting, keys};
use iotkit_image::Version;
use serde::{Deserialize, Serialize};

/// Boots an update gets to pass its health check before the previous slot comes back.
pub const MAX_TRIAL_BOOTS: u8 = 3;

/// An installed update waiting to be confirmed (in flash).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// OTA slot number of the update (0 for `ota_0`).
    pub slot: u8,
    /// Slot to go back to.
    pub previous: u8,
    /// Version of the update.
    pub version: (u16, u16, u16),
    /// Trial boots so far.
    pub boots: u8,
}

impl Setting for Pending {
    const KEY: u16 = keys::OTA_PENDING;
    const VERSION: u8 = 1;
}

impl Pending {
    /// Version of the update.
    pub fn version(&self) -> Version {
        Version::new(self.version.0, self.version.1, self.version.2)
    }
}

/// What the last update did (in flash, for diagnostics).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastUpdate {
    /// What happened.
    pub outcome: Outcome,
    /// Version of the update.
    pub version: (u16, u16, u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// What became of an update.
pub enum Outcome {
    /// Written and selected; not booted yet.
    Installed,
    /// Booted and healthy: kept.
    Confirmed,
    /// Not healthy in time: the previous firmware is back.
    RolledBack,
}

impl Setting for LastUpdate {
    const KEY: u16 = keys::OTA_LAST;
    const VERSION: u8 = 1;
}

/// Where to check for updates: a manifest URL (`cargo iotkit ota publish`), kept in flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSource {
    /// `http://` URL of the manifest.
    pub manifest_url: heapless::String<200>,
}

impl Setting for UpdateSource {
    const KEY: u16 = keys::UPDATE_SOURCE;
    const VERSION: u8 = 1;
}

/// What to do at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootAction {
    /// No update in progress.
    Normal,
    /// A trial boot of the pending update: record it, then run.
    Trial {
        /// Trial boot number, from 1.
        boot: u8,
    },
    /// The update had its trial boots without being confirmed: select slot `to` and restart.
    RollBack {
        /// The slot to go back to.
        to: u8,
    },
    /// Not running the pending update (the bootloader went back by itself): forget it.
    Abandoned,
}

/// Decides from the pending update and the slot that was booted.
pub fn on_boot(pending: Option<&Pending>, booted_slot: Option<u8>) -> BootAction {
    match pending {
        None => BootAction::Normal,
        Some(p) if booted_slot != Some(p.slot) => BootAction::Abandoned,
        Some(p) if p.boots >= MAX_TRIAL_BOOTS => BootAction::RollBack { to: p.previous },
        Some(p) => BootAction::Trial { boot: p.boots + 1 },
    }
}

#[doc(hidden)]
pub const fn __version(s: &str) -> Option<Version> {
    // const-friendly `major.minor.patch` parser, for `firmware!`.
    let b = s.as_bytes();
    let mut parts = [0u32; 3];
    let (mut part, mut digits, mut i) = (0, 0, 0);
    while i < b.len() {
        match b[i] {
            b'0'..=b'9' => {
                parts[part] = parts[part] * 10 + (b[i] - b'0') as u32;
                digits += 1;
                if parts[part] > u16::MAX as u32 {
                    return None;
                }
            }
            b'.' if digits > 0 && part < 2 => {
                part += 1;
                digits = 0;
            }
            _ => return None,
        }
        i += 1;
    }
    if part != 2 || digits == 0 {
        return None;
    }
    Some(Version::new(parts[0] as u16, parts[1] as u16, parts[2] as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(boots: u8) -> Pending {
        Pending { slot: 1, previous: 0, version: (1, 2, 0), boots }
    }

    #[test]
    fn boot_policy() {
        assert_eq!(on_boot(None, Some(0)), BootAction::Normal);
        assert_eq!(on_boot(Some(&pending(0)), Some(1)), BootAction::Trial { boot: 1 });
        assert_eq!(on_boot(Some(&pending(2)), Some(1)), BootAction::Trial { boot: 3 });
        assert_eq!(on_boot(Some(&pending(3)), Some(1)), BootAction::RollBack { to: 0 });
        assert_eq!(on_boot(Some(&pending(200)), Some(1)), BootAction::RollBack { to: 0 });
        // The bootloader is running the previous slot (its own rollback, or a USB flash).
        assert_eq!(on_boot(Some(&pending(1)), Some(0)), BootAction::Abandoned);
        assert_eq!(on_boot(Some(&pending(1)), None), BootAction::Abandoned);
    }

    #[test]
    fn const_versions() {
        assert_eq!(__version("1.20.3"), Some(Version::new(1, 20, 3)));
        for bad in ["", "1.2", "1.2.3.4", "1..3", "1.2.x", "70000.0.0", "1.2.3-rc"] {
            assert_eq!(__version(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_crashing_update_rolls_back_after_its_trial_boots() {
        let mut p = pending(0);
        let mut boots = 0;
        loop {
            match on_boot(Some(&p), Some(p.slot)) {
                BootAction::Trial { boot } => {
                    p.boots = boot;
                    boots += 1;
                }
                BootAction::RollBack { to } => {
                    assert_eq!(to, 0);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(boots, MAX_TRIAL_BOOTS);
    }
}
