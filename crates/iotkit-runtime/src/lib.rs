//! The health check: a firmware is good once each required [`Check`] has passed.
//!
//! Each subsystem passes its check when it works (Wi-Fi up, broker reached, ...). The runtime
//! waits for [`Health::healthy`]; then it marks the boot stable and, after an OTA update, the
//! image valid. If the checks never all pass, the image is not confirmed and the bootloader can
//! roll it back.
//!
//! ```ignore
//! const BOOTED: Check = Check(0);
//! const WIFI: Check = Check(1);
//! static HEALTH: Health = Health::new(&[BOOTED, WIFI]);
//!
//! HEALTH.pass(WIFI);             // in the Wi-Fi task
//! HEALTH.healthy().await;        // in the supervisor
//! ```

#![no_std]

pub mod shutdown;

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

/// One health check: a bit, 0 to 31.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check(pub u8);

impl Check {
    const fn bit(self) -> u32 {
        assert!(self.0 < 32, "Check is 0..32");
        1 << self.0
    }
}

/// The checks a firmware must pass, and those passed so far.
pub struct Health {
    required: u32,
    passed: AtomicU32,
    changed: Signal<CriticalSectionRawMutex, ()>,
}

impl Health {
    pub const fn new(required: &[Check]) -> Self {
        let mut mask = 0;
        let mut i = 0;
        while i < required.len() {
            mask |= required[i].bit();
            i += 1;
        }
        Self { required: mask, passed: AtomicU32::new(0), changed: Signal::new() }
    }

    /// Marks `check` as passed.
    pub fn pass(&self, check: Check) {
        let before = self.passed.fetch_or(check.bit(), Ordering::AcqRel);
        if before & check.bit() == 0 {
            self.changed.signal(());
        }
    }

    pub fn has_passed(&self, check: Check) -> bool {
        self.passed.load(Ordering::Acquire) & check.bit() != 0
    }

    pub fn is_healthy(&self) -> bool {
        self.missing() == 0
    }

    /// Bits of the required checks not passed yet.
    pub fn missing(&self) -> u32 {
        self.required & !self.passed.load(Ordering::Acquire)
    }

    /// Waits until every required check has passed. One waiter at a time.
    pub async fn healthy(&self) {
        while !self.is_healthy() {
            self.changed.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::{block_on, join::join, yield_now};

    use super::*;

    const A: Check = Check(0);
    const B: Check = Check(5);
    const EXTRA: Check = Check(31);

    #[test]
    fn healthy_once_all_required_passed() {
        let h = Health::new(&[A, B]);
        assert!(!h.is_healthy());
        assert_eq!(h.missing(), 1 | 1 << 5);
        h.pass(A);
        h.pass(EXTRA); // not required: changes nothing
        assert_eq!(h.missing(), 1 << 5);
        h.pass(B);
        assert!(h.is_healthy() && h.has_passed(EXTRA));
    }

    #[test]
    fn healthy_waits() {
        let h = Health::new(&[A, B]);
        block_on(join(h.healthy(), async {
            h.pass(A);
            yield_now().await;
            h.pass(A); // twice: harmless
            yield_now().await;
            h.pass(B);
        }));
        assert!(h.is_healthy());
    }

    #[test]
    fn no_checks_is_healthy() {
        assert!(Health::new(&[]).is_healthy());
        block_on(Health::new(&[]).healthy());
    }
}
