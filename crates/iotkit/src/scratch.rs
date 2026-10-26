//! The work area: one buffer for the jobs that rarely run at once (a download, an update's
//! install, the setup access point), so their RAM is reserved once, at the size of the largest,
//! instead of in each task's future for good. A job holds it while it runs, then gives it back.
//!
//! An update asked for while the setup access point holds it is refused
//! ([`OtaError::InUse`](crate::ota::esp::OtaError::InUse)); a setup window opened during an
//! update waits for it (an update ends with a restart anyway).

use core::mem::ManuallyDrop;

use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    mutex::{Mutex, MutexGuard},
};

/// What the jobs work in, one at a time. Every field is plain bytes: any content is valid for
/// each, so reading one after another wrote is sound.
pub(crate) union Area {
    download: ManuallyDrop<crate::net::esp::GetBuffers>,
    #[cfg(any(feature = "ota-push", feature = "ha-update"))]
    update: ManuallyDrop<crate::ota::esp::UpdateBuffers>,
    #[cfg(feature = "softap")]
    access_point: ManuallyDrop<crate::provision::esp::ApBuffers>,
}

impl Area {
    /// For a GET (`net::esp::get`).
    pub(crate) fn download(&mut self) -> &mut crate::net::esp::GetBuffers {
        // SAFETY: plain bytes (see `Area`).
        unsafe { &mut self.download }
    }

    /// For an update's install or check.
    #[cfg(any(feature = "ota-push", feature = "ha-update"))]
    pub(crate) fn update(&mut self) -> &mut crate::ota::esp::UpdateBuffers {
        // SAFETY: plain bytes (see `Area`).
        unsafe { &mut self.update }
    }

    /// For the setup access point's servers.
    #[cfg(feature = "softap")]
    pub(crate) fn access_point(&mut self) -> &mut crate::provision::esp::ApBuffers {
        // SAFETY: plain bytes (see `Area`).
        unsafe { &mut self.access_point }
    }
}

// SAFETY: all zeros is valid bytes. In `.bss`; unlinked when no job is.
static AREA: Mutex<CriticalSectionRawMutex, Area> = Mutex::new(unsafe { core::mem::zeroed() });

/// The area, held until dropped.
pub(crate) type Lease = MutexGuard<'static, CriticalSectionRawMutex, Area>;

/// The area, once free.
pub(crate) async fn lease() -> Lease {
    AREA.lock().await
}

/// The area if free now.
#[cfg(any(feature = "ota-push", feature = "ha-update", feature = "softap"))]
pub(crate) fn try_lease() -> Option<Lease> {
    AREA.try_lock().ok()
}
