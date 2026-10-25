//! Firmware updates: download a signed image into the spare OTA slot, boot it on trial, keep it
//! once the health check passes, roll back otherwise.
//!
//! ```text
//! manifest (signed) ─► verify, no downgrade ─► image ─► app header, SHA-256 ─► switch slot
//! ─► restart ─► trial boot 1..=MAX_TRIAL_BOOTS ─► healthy: confirm   (else: previous slot)
//! ```
//!
//! An update is rolled back when it crashes on each of its [`MAX_TRIAL_BOOTS`], or when a trial
//! boot is not healthy within [`UPDATE_TIMEOUT_S`] (a deep-sleeping device stays awake until
//! then). Rollback does not rely on the bootloader: [`Pending`] in flash counts the trial boots,
//! and the firmware switches back itself (so the image only has to reach `esp::boot`). A
//! bootloader built with rollback works too.

#[cfg(esp_ota)]
pub mod esp;

use crate::config::{Setting, keys};
use crate::image::{PublicKey, Version};
use serde::{Deserialize, Serialize};

/// The running firmware, as updates must match it.
#[derive(Clone, Copy)]
pub struct Firmware {
    /// Package name, also the manifest's `product` and the image's project name.
    pub product: &'static str,
    /// `major.minor.patch` from `Cargo.toml`.
    pub version: Version,
    /// [`version`](Self::version) as written in `Cargo.toml`.
    pub version_text: &'static str,
    /// Public key of `keys/ota.pub`.
    pub key: &'static PublicKey,
}

/// The [`Firmware`] of the calling crate: its package name and version, and `$key`. Fails to
/// build on a version that is not `major.minor.patch`.
#[macro_export]
macro_rules! firmware {
    ($key:expr) => {
        $crate::ota::Firmware {
            product: env!("CARGO_PKG_NAME"),
            version: match $crate::ota::__version(env!("CARGO_PKG_VERSION")) {
                Some(v) if $crate::ota::__fits(v) => v,
                Some(_) => panic!("CARGO_PKG_VERSION: Matter OTA takes a minor and patch up to 255"),
                None => panic!("CARGO_PKG_VERSION is not major.minor.patch"),
            },
            version_text: env!("CARGO_PKG_VERSION"),
            key: $key,
        }
    };
}

/// Boots an update gets to pass its health check before the previous slot comes back:
/// `IOTKIT_UPDATE_TRIAL_BOOTS` at build time (1 to 10), 3 by default.
pub const MAX_TRIAL_BOOTS: u8 = match number(env!("IOTKIT_UPDATE_TRIAL_BOOTS")) {
    Some(n @ 1..=10) => n as u8,
    _ => panic!("IOTKIT_UPDATE_TRIAL_BOOTS: 1 to 10"),
};

/// Seconds a trial boot gets to pass the health check before the previous slot comes back:
/// `IOTKIT_UPDATE_TIMEOUT` at build time (60 to 86400), 1800 by default.
pub const UPDATE_TIMEOUT_S: u32 = match number(env!("IOTKIT_UPDATE_TIMEOUT")) {
    Some(n @ 60..=86_400) => n,
    _ => panic!("IOTKIT_UPDATE_TIMEOUT: 60 to 86400 seconds"),
};

/// A decimal number, for the build-time settings.
pub(crate) const fn number(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 9 {
        return None;
    }
    let (mut n, mut i) = (0, 0);
    while i < b.len() {
        match b[i] {
            b'0'..=b'9' => n = n * 10 + (b[i] - b'0') as u32,
            _ => return None,
        }
        i += 1;
    }
    Some(n)
}

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

/// A downloaded and checked image, in the spare slot but not selected: Matter's provider has not
/// said to apply it yet. Other updates may overwrite the slot meanwhile, so it is hashed again
/// before it is selected; it survives restarts and deep sleep (no download again).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    /// OTA slot number holding it.
    pub slot: u8,
    /// Size of the image.
    pub size: u32,
    /// SHA-256 of the image.
    pub sha256: [u8; 32],
    /// Version of the image.
    pub version: (u16, u16, u16),
    /// SHA-256 of what was downloaded (a Matter OTA file's payload), to recognise it when offered
    /// again.
    pub source: [u8; 32],
}

impl Setting for Staged {
    const KEY: u16 = keys::OTA_STAGED;
    const VERSION: u8 = 1;
}

impl Staged {
    /// Version of the image.
    pub fn version(&self) -> Version {
        Version::new(self.version.0, self.version.1, self.version.2)
    }

    /// Whether it may still be applied: newer than `running`, in `target` (the slot an update
    /// goes to now), and not on trial (the spare slot then holds the firmware to go back to).
    pub fn usable(&self, target: u8, running: Version, on_trial: bool) -> bool {
        !on_trial && self.slot == target && self.version() > running
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

/// Boots of an unconfirmed update the early guard allows: [`on_boot`] rolls back on boot
/// `MAX_TRIAL_BOOTS + 1`, so past this the update never reached it.
#[cfg(any(esp_ota, test))]
pub(crate) const EARLY_LIMIT: u32 = 2 * MAX_TRIAL_BOOTS as u32;
/// Marks [`early_check`]'s counter as set (RTC memory holds garbage after a power cut).
#[cfg(any(esp_ota, test))]
pub(crate) const EARLY_MAGIC: u32 = 0x07a0_ea71;

/// The early guard's decision, from whether the running firmware is unconfirmed and the boot
/// counter in RTC memory (magic, count): the counter to keep, and whether to roll back.
#[cfg(any(esp_ota, test))]
pub(crate) fn early_check(unconfirmed: bool, counter: [u32; 2]) -> ([u32; 2], bool) {
    if !unconfirmed {
        return ([0; 2], false);
    }
    let boots = if counter[0] == EARLY_MAGIC { counter[1].saturating_add(1) } else { 1 };
    ([EARLY_MAGIC, boots], boots > EARLY_LIMIT)
}

/// `v` can run: under `matter-ota`, it must be a Matter `SoftwareVersion`, or updates would
/// compare against 0 (downgrades) and never confirm.
#[doc(hidden)]
pub const fn __fits(v: Version) -> bool {
    !cfg!(feature = "matter-ota") || crate::image::matter::version(v).is_some()
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
    fn staged_policy() {
        let s = Staged { slot: 1, size: 10, sha256: [0; 32], version: (1, 2, 0), source: [0; 32] };
        assert!(s.usable(1, Version::new(1, 1, 0), false));
        assert!(!s.usable(1, Version::new(1, 1, 0), true), "on trial");
        assert!(!s.usable(0, Version::new(1, 1, 0), false), "another slot is the target now");
        assert!(!s.usable(1, Version::new(1, 2, 0), false), "not newer");
    }

    #[test]
    fn boot_policy() {
        assert_eq!(on_boot(None, Some(0)), BootAction::Normal);
        assert_eq!(on_boot(Some(&pending(0)), Some(1)), BootAction::Trial { boot: 1 });
        let last = MAX_TRIAL_BOOTS;
        assert_eq!(on_boot(Some(&pending(last - 1)), Some(1)), BootAction::Trial { boot: last });
        assert_eq!(on_boot(Some(&pending(last)), Some(1)), BootAction::RollBack { to: 0 });
        assert_eq!(on_boot(Some(&pending(200)), Some(1)), BootAction::RollBack { to: 0 });
        // The bootloader is running the previous slot (its own rollback, or a USB flash).
        assert_eq!(on_boot(Some(&pending(1)), Some(0)), BootAction::Abandoned);
        assert_eq!(on_boot(Some(&pending(1)), None), BootAction::Abandoned);
    }

    #[test]
    fn build_settings() {
        assert_eq!(number("1800"), Some(1800));
        for bad in ["", "-1", "1.5", "30m", "1234567890"] {
            assert_eq!(number(bad), None, "{bad}");
        }
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

    #[test]
    fn early_guard_counts_only_unconfirmed_boots() {
        assert_eq!(early_check(false, [EARLY_MAGIC, 5]), ([0; 2], false));
        // After a power cut the counter is garbage: start over.
        assert_eq!(early_check(true, [0xdead_beef, 5]), ([EARLY_MAGIC, 1], false));
        assert_eq!(early_check(true, [EARLY_MAGIC, u32::MAX]), ([EARLY_MAGIC, u32::MAX], true));
    }

    #[test]
    fn an_update_crashing_before_its_trial_boots_rolls_back() {
        let mut counter = [0; 2];
        let mut rolled_back_at = None;
        for boot in 1..=EARLY_LIMIT + 1 {
            let (next, roll_back) = early_check(true, counter);
            counter = next;
            if roll_back {
                rolled_back_at = Some(boot);
                break;
            }
        }
        assert_eq!(rolled_back_at, Some(EARLY_LIMIT + 1));
    }
}

#[cfg(esp_ota)]
pub(crate) use crate::atomic;
