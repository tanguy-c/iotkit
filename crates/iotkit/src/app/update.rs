//! Updates from Home Assistant (`ha-update`): a `firmware` update entity, release checks against
//! the manifest URL, and installs on request.

use alloc::{boxed::Box, vec::Vec};
use core::fmt::Write as _;

use crate::config::esp::SharedStore;
use crate::model::{Entity, Update};
use crate::net::esp::Net;
use crate::ota::{UpdateSource, esp::Firmware};
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_time::{Duration, Timer};
use heapless::String;
use log::{info, warn};

/// The firmware, as Home Assistant's update entity.
pub static FIRMWARE: Update = Update::new("firmware", "Firmware").config();

/// How often the manifest is checked.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

/// The app's entities and [`FIRMWARE`] (built once, at start).
pub(crate) fn with_update(entities: &'static [&'static dyn Entity]) -> &'static [&'static dyn Entity] {
    let mut all = Vec::with_capacity(entities.len() + 1);
    all.extend_from_slice(entities);
    all.push(&FIRMWARE as &dyn Entity);
    Box::leak(all.into_boxed_slice())
}

/// The manifest URL: the stored one; the first time, the one given at build time
/// (`OTA_MANIFEST_URL`), which is then stored.
pub(crate) async fn source(store: &SharedStore) -> Option<String<200>> {
    let mut store = store.lock().await;
    if let Ok(Some(s)) = store.load::<UpdateSource>().await {
        return Some(s.manifest_url).filter(|u| !u.is_empty());
    }
    let built_in = String::try_from(option_env!("OTA_MANIFEST_URL").filter(|u| !u.is_empty())?).ok()?;
    let _ = store.save(&UpdateSource { manifest_url: built_in.clone() }).await;
    Some(built_in)
}

/// Checks for releases (at start, then every [`CHECK_EVERY`], and when the URL changes) and
/// installs the latest one when Home Assistant asks.
#[embassy_executor::task]
pub(crate) async fn update_task(net: Net, store: &'static SharedStore, fw: Firmware) {
    FIRMWARE.set_installed(fw.version_text);
    net.wait_online().await;
    // Not right after a failed install: its error stays in the summary until the next check.
    let mut check_now = true;
    loop {
        let url = source(store).await;
        match &url {
            Some(url) if check_now => check(url, fw).await,
            Some(_) => {}
            None => info!("update: no manifest URL (set one with `update-url URL`)"),
        }
        check_now = true;
        match select3(Timer::after(CHECK_EVERY), FIRMWARE.wait_install(), SOURCE_CHANGED.wait()).await {
            Either3::Second(()) => match &url {
                Some(url) => {
                    install(url).await;
                    check_now = false;
                }
                None => warn!("update: install asked, but no manifest URL"),
            },
            Either3::First(()) | Either3::Third(()) => {}
        }
    }
}

/// The URL was changed (console): check again now.
pub(crate) static SOURCE_CHANGED: embassy_sync::signal::Signal<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    (),
> = embassy_sync::signal::Signal::new();

async fn check(url: &str, fw: Firmware) {
    match crate::ota::esp::request_check(url).await {
        Ok(Some(r)) => {
            let mut v: String<16> = String::new();
            let _ = write!(v, "{}", r.version);
            info!("update: {v} available");
            FIRMWARE.set_latest(&v, &r.summary, &r.release_url);
        }
        Ok(None) => FIRMWARE.set_latest(fw.version_text, "", ""),
        Err(e) => warn!("update: cannot check {url}: {e}"),
    }
}

async fn install(url: &str) {
    info!("update: installing from {url}");
    FIRMWARE.set_progress(true, None);
    let installing = crate::ota::esp::request_install(url);
    let progress = async {
        loop {
            FIRMWARE.set_progress(true, Some(crate::ota::esp::wait_progress().await));
        }
    };
    match select(installing, progress).await {
        Either::First(Ok(v)) => {
            info!("update: {v} installed, restarting");
            FIRMWARE.set_progress(true, Some(100));
            // Let Home Assistant see it before the restart says goodbye.
            Timer::after_millis(500).await;
            crate::diag::esp::restart().await;
        }
        Either::First(Err(e)) => {
            warn!("update: install failed: {e}");
            let mut summary: String<64> = String::new();
            let _ = write!(summary, "Update failed: {e}");
            let latest = FIRMWARE.update().map(|u| u.latest).unwrap_or_default();
            FIRMWARE.set_progress(false, None);
            FIRMWARE.set_latest(&latest, &summary, "");
        }
        Either::Second(()) => {}
    }
}
