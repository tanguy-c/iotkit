//! Cooperative shutdown before a planned restart: links say goodbye (MQTT `offline`) instead of
//! vanishing.
//!
//! ```
//! use iotkit_core::runtime::shutdown;
//!
//! let mut party = shutdown::register().unwrap();   // in the MQTT task
//! let mqtt = async {
//!     party.requested().await;  // then publish offline, disconnect
//!     party.done();
//! };
//! // Before `software_reset`, bounded with `embassy_time::with_timeout`:
//! let restart = shutdown::shutdown();
//! # embassy_futures::block_on(embassy_futures::join::join(mqtt, restart));
//! ```

use crate::runtime::atomic::{AtomicU8, Ordering};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal, watch::Watch};

/// Parties that can register.
pub const MAX_PARTIES: usize = 4;

static REQUEST: Watch<CriticalSectionRawMutex, (), MAX_PARTIES> = Watch::new();
static REGISTERED: AtomicU8 = AtomicU8::new(0);
static DONE: AtomicU8 = AtomicU8::new(0);
static FINISHED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Something that must finish its work before a restart.
pub struct Party {
    rx: embassy_sync::watch::Receiver<'static, CriticalSectionRawMutex, (), MAX_PARTIES>,
    finished: bool,
}

/// Registers a party; `None` if [`MAX_PARTIES`] already are.
pub fn register() -> Option<Party> {
    let rx = REQUEST.receiver()?;
    REGISTERED.fetch_add(1, Ordering::AcqRel);
    Some(Party { rx, finished: false })
}

impl Party {
    /// Waits until a shutdown is asked for.
    pub async fn requested(&mut self) {
        self.rx.changed().await;
    }

    /// Whether a shutdown was asked for, without waiting.
    pub fn is_requested(&mut self) -> bool {
        self.rx.try_get().is_some()
    }

    /// This party is ready for the restart.
    pub fn done(&mut self) {
        if !self.finished {
            self.finished = true;
            DONE.fetch_add(1, Ordering::AcqRel);
            FINISHED.signal(());
        }
    }
}

/// Asks every party to finish and waits until they all have. Bound it with a timeout.
pub async fn shutdown() {
    REQUEST.sender().send(());
    while DONE.load(Ordering::Acquire) < REGISTERED.load(Ordering::Acquire) {
        FINISHED.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::{block_on, join::join};

    use super::*;

    #[test]
    fn parties_finish_before_shutdown_returns() {
        let mut a = register().unwrap();
        let mut b = register().unwrap();
        assert!(!a.is_requested());
        let parties = async {
            a.requested().await;
            a.done();
            b.requested().await;
            b.done();
            b.done(); // twice: counted once
        };
        block_on(join(shutdown(), parties));
        assert!(a.is_requested());
        assert_eq!(DONE.load(Ordering::Acquire), 2);
    }
}
