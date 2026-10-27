//! Updates from Home Assistant (`ha-update`): a `firmware` update entity, release checks against
//! the manifest URL, and installs on request.
//!
//! With *Auto-update* (`cfg(esp_auto)`: `IOTKIT_AUTO_UPDATE=on`) a release installs without a
//! click: over MQTT once found, over the ESPHome API (`cfg(esp_scheduled)`) at *Update time*
//! (02:00 by default, or at once). For a set time it downloads the release at once and installs
//! it then (Home Assistant gives the time and zone). A deep-sleeping device over the API cannot get
//! *Install* while it sleeps (nothing holds it), so it always has the switch, and wakes up at the
//! time.

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

/// How often the manifest is checked (across deep sleeps too).
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Longest check: a deep-sleeping device stays awake for it.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// After a failed check or download, the next one (not at every wake-up: a server down would
/// keep each awake).
#[cfg(feature = "deep-sleep")]
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// A check is running.
static CHECKING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether a release check is running (a deep-sleeping device does not sleep then).
#[cfg(feature = "deep-sleep")]
pub(crate) fn checking() -> bool {
    CHECKING.load(core::sync::atomic::Ordering::Relaxed)
}

/// The app's entities and [`FIRMWARE`] (built once, at start).
pub(crate) fn with_update(entities: &'static [&'static dyn Entity]) -> &'static [&'static dyn Entity] {
    let mut all = Vec::with_capacity(entities.len() + 3);
    all.extend_from_slice(entities);
    all.push(&FIRMWARE as &dyn Entity);
    #[cfg(esp_auto)]
    all.push(&auto::AUTO_UPDATE as &dyn Entity);
    #[cfg(esp_scheduled)]
    all.push(&auto::UPDATE_TIME as &dyn Entity);
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
    #[cfg(feature = "deep-sleep")]
    known::restore(store, fw).await;
    // Not waiting for the network: the image is in flash already.
    #[cfg(esp_auto)]
    auto::start(store, fw).await;
    net.wait_online().await;
    // A deep-sleeping device checks every `CHECK_EVERY` across its sleeps, and at its update
    // time (a release out since is installed then).
    #[cfg(feature = "deep-sleep")]
    #[cfg_attr(not(esp_auto), allow(unused_variables))]
    let (mut check_now, at_slot) = known::due();
    // Not right after a failed install: its error stays in the summary until the next check.
    #[cfg(not(feature = "deep-sleep"))]
    let mut check_now = true;
    #[cfg(all(esp_auto, not(feature = "deep-sleep")))]
    let at_slot = false;
    loop {
        let url = source(store).await;
        match &url {
            Some(url) if check_now => {
                CHECKING.store(true, core::sync::atomic::Ordering::Relaxed);
                let checked = embassy_time::with_timeout(CHECK_TIMEOUT, check(url, fw)).await.unwrap_or_else(|_| {
                    warn!("update: no answer from {url} in {} s", CHECK_TIMEOUT.as_secs());
                    Err(())
                });
                #[cfg_attr(not(esp_auto), allow(unused_mut))]
                let mut ok = checked.is_ok();
                #[cfg(esp_auto)]
                if let Ok(Some(version)) = checked {
                    ok = auto::released(url, store, fw, version, at_slot).await;
                }
                #[cfg(feature = "deep-sleep")]
                crate::app::sleep::checked(match ok {
                    true => 0,
                    false => (CHECK_EVERY - RETRY_AFTER).as_secs() as u32,
                });
                #[cfg(not(feature = "deep-sleep"))]
                let _ = ok;
                #[cfg(feature = "deep-sleep")]
                known::save(store, fw).await;
                CHECKING.store(false, core::sync::atomic::Ordering::Relaxed);
            }
            Some(_) => {}
            None => info!("update: no manifest URL (set one with `update-url URL`)"),
        }
        check_now = true;
        #[cfg(esp_auto)]
        let wait = select3(
            Timer::after(CHECK_EVERY),
            FIRMWARE.wait_install(),
            select(SOURCE_CHANGED.wait(), auto::run(store, fw)),
        );
        #[cfg(not(esp_auto))]
        let wait = select3(Timer::after(CHECK_EVERY), FIRMWARE.wait_install(), SOURCE_CHANGED.wait());
        match wait.await {
            Either3::Second(()) => match &url {
                Some(url) => {
                    install(url).await;
                    check_now = false;
                }
                None => warn!("update: install asked, but no manifest URL"),
            },
            // The timer, a new URL, or *Auto-update* turned on: check now.
            Either3::First(()) | Either3::Third(_) => {}
        }
    }
}

/// The URL was changed (console): check again now.
pub(crate) static SOURCE_CHANGED: embassy_sync::signal::Signal<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    (),
> = embassy_sync::signal::Signal::new();

/// Checks the manifest at `url`: the release's version if newer.
async fn check(url: &str, fw: Firmware) -> Result<Option<crate::image::Version>, ()> {
    info!("update: checking {url}");
    match crate::ota::esp::request_check(url).await {
        Ok(Some(r)) => {
            let mut v: String<16> = String::new();
            let _ = write!(v, "{}", r.version);
            info!("update: {v} available");
            FIRMWARE.set_latest(&v, &r.summary, &r.release_url);
            Ok(Some(r.version))
        }
        Ok(None) => {
            FIRMWARE.set_latest(fw.version_text, "", "");
            Ok(None)
        }
        Err(e) => {
            warn!("update: cannot check {url}: {e}");
            Err(())
        }
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

/// Installs without a click, at a set time over the ESPHome API ([module](self) docs).
#[cfg(esp_auto)]
mod auto {
    #[cfg(esp_scheduled)]
    use core::fmt::Write as _;

    #[cfg(esp_scheduled)]
    use embassy_futures::select::{Either, select};
    #[cfg(esp_scheduled)]
    use embassy_time::Timer;
    #[cfg(esp_scheduled)]
    use heapless::String;
    use log::info;
    #[cfg(esp_scheduled)]
    use log::warn;
    use serde::{Deserialize, Serialize};

    #[cfg(esp_scheduled)]
    use super::FIRMWARE;
    #[cfg(esp_scheduled)]
    use crate::atomic::{AtomicU32, Ordering};
    use crate::config::{Setting, esp::SharedStore, keys};
    use crate::image::Version;
    #[cfg(esp_scheduled)]
    use crate::model::Entity;
    #[cfg(esp_scheduled)]
    use crate::model::Select;
    use crate::model::Switch;
    use crate::ota::esp::Firmware;

    /// Updates install without a click.
    pub static AUTO_UPDATE: Switch = Switch::new("auto_update", "Auto-update").icon("mdi:autorenew").config();
    /// When: at the wake-up (or the check) that finds the release, or every half hour of the day.
    #[cfg(esp_scheduled)]
    pub static UPDATE_TIME: Select =
        Select::new("update_time", "Update time", &TIMES).icon("mdi:clock-outline").config();

    /// `IOTKIT_AUTO_UPDATE`: on unless `off`.
    const DEFAULT_ON: bool = !cfg!(auto_update_off);

    #[cfg(esp_scheduled)]
    const TIMES: [&str; 49] = [
        "At wake-up",
        "00:00",
        "00:30",
        "01:00",
        "01:30",
        "02:00",
        "02:30",
        "03:00",
        "03:30",
        "04:00",
        "04:30",
        "05:00",
        "05:30",
        "06:00",
        "06:30",
        "07:00",
        "07:30",
        "08:00",
        "08:30",
        "09:00",
        "09:30",
        "10:00",
        "10:30",
        "11:00",
        "11:30",
        "12:00",
        "12:30",
        "13:00",
        "13:30",
        "14:00",
        "14:30",
        "15:00",
        "15:30",
        "16:00",
        "16:30",
        "17:00",
        "17:30",
        "18:00",
        "18:30",
        "19:00",
        "19:30",
        "20:00",
        "20:30",
        "21:00",
        "21:30",
        "22:00",
        "22:30",
        "23:00",
        "23:30",
    ];
    /// `IOTKIT_UPDATE_TIME` at build time, 02:00 by default: the device rarely does anything then.
    const DEFAULT_TIME: u8 = match time_index(env!("IOTKIT_UPDATE_TIME").as_bytes()) {
        Some(i) => i,
        None => panic!("IOTKIT_UPDATE_TIME: `HH:MM` on the half hour (02:00, 13:30), or `wake`"),
    };

    /// The index in [`TIMES`] of `HH:MM`, or of `wake`.
    const fn time_index(s: &[u8]) -> Option<u8> {
        if let [b'w', b'a', b'k', b'e'] = s {
            return Some(0);
        }
        let [h1 @ b'0'..=b'2', h2 @ b'0'..=b'9', b':', m1 @ (b'0' | b'3'), b'0'] = *s else { return None };
        let hour = (h1 - b'0') * 10 + (h2 - b'0');
        if hour > 23 {
            return None;
        }
        Some(1 + hour * 2 + (m1 == b'3') as u8)
    }

    #[derive(Clone, Copy, Serialize, Deserialize)]
    struct Settings {
        on: bool,
        at: u8,
    }

    impl Setting for Settings {
        const KEY: u16 = keys::OTA_AUTO;
        const VERSION: u8 = 1;
    }

    /// A release downloaded for later (`ota::Staged`), and when to install it.
    #[cfg(esp_scheduled)]
    #[derive(Clone, Copy, Serialize, Deserialize)]
    struct Scheduled {
        version: (u16, u16, u16),
        /// Unix seconds; 0 until the clock is known.
        due: u32,
    }

    #[cfg(esp_scheduled)]
    impl Setting for Scheduled {
        const KEY: u16 = keys::OTA_SCHEDULED;
        const VERSION: u8 = 1;
    }

    /// When the scheduled install is due (Unix seconds, 0: none), for the sleep: it wakes up then.
    #[cfg(esp_scheduled)]
    static DUE: AtomicU32 = AtomicU32::new(0);

    /// Seconds until the scheduled install, for the sleep (`None`: nothing scheduled, or no clock).
    #[cfg(esp_scheduled)]
    pub(crate) fn seconds_to_due() -> Option<u64> {
        let now = crate::clock::now()?;
        let due = i64::from(DUE.load(Ordering::Relaxed));
        let staged = (due > 0).then(|| (due - now).max(0));
        // The update time, a few seconds past (the RTC drifts): a release out since is installed.
        let at = UPDATE_TIME.get().unwrap_or(DEFAULT_TIME);
        let slot =
            (AUTO_UPDATE.get() == Some(true) && at != 0).then(|| crate::clock::zone().next(now, minute(at)) - now + 5);
        [staged, slot].into_iter().flatten().min().map(|s| s as u64)
    }

    #[cfg(esp_scheduled)]
    fn minute(at: u8) -> u16 {
        (u16::from(at) - 1) * 30
    }

    /// The settings shown; the update scheduled installed if due.
    pub(crate) async fn start(store: &'static SharedStore, fw: Firmware) {
        let settings = store.lock().await.load::<Settings>().await.ok().flatten();
        let settings = settings.unwrap_or(Settings { on: DEFAULT_ON, at: DEFAULT_TIME });
        AUTO_UPDATE.set(settings.on);
        #[cfg(esp_scheduled)]
        {
            UPDATE_TIME.set(settings.at);
            install_if_due(store, fw).await;
        }
        #[cfg(not(esp_scheduled))]
        let _ = fw;
    }

    /// Whether the update time came within the last `seconds` (the check before it then).
    #[cfg(all(esp_scheduled, feature = "deep-sleep"))]
    pub(crate) fn slot_within(seconds: u32) -> bool {
        let at = UPDATE_TIME.get().unwrap_or(DEFAULT_TIME);
        let Some(now) = crate::clock::now() else { return false };
        if AUTO_UPDATE.get() != Some(true) || at == 0 {
            return false;
        }
        let last = crate::clock::zone().next(now - 86_400, minute(at));
        last <= now && now - last < i64::from(seconds)
    }

    /// A newer release is out (`version`): installed now (without a clock, at once).
    #[cfg(not(esp_scheduled))]
    pub(crate) async fn released(
        url: &str,
        _store: &'static SharedStore,
        _fw: Firmware,
        version: Version,
        _at_slot: bool,
    ) -> bool {
        if AUTO_UPDATE.get() != Some(true) {
            return true;
        }
        info!("update: auto-update, installing {version}");
        // Restarts once installed.
        super::install(url).await;
        false
    }

    /// Saves the switch changed in Home Assistant; returns when it is turned on, so that a release
    /// found already installs now, not at the next check.
    #[cfg(not(esp_scheduled))]
    pub(crate) async fn run(store: &'static SharedStore, _fw: Firmware) {
        loop {
            let on = AUTO_UPDATE.wait_command().await;
            let turned_on = on && AUTO_UPDATE.get() != Some(true);
            AUTO_UPDATE.set(on);
            let _ = store.lock().await.save(&Settings { on, at: DEFAULT_TIME }).await;
            if turned_on {
                return;
            }
        }
    }

    /// A newer release is out (`version`): installed now (at the wake-up that found it, or at the
    /// update time: `at_slot`), or downloaded for its time. `false` if that failed.
    #[cfg(esp_scheduled)]
    pub(crate) async fn released(
        url: &str,
        store: &'static SharedStore,
        fw: Firmware,
        version: Version,
        at_slot: bool,
    ) -> bool {
        if AUTO_UPDATE.get() != Some(true) {
            return true;
        }
        let at = UPDATE_TIME.get().unwrap_or(DEFAULT_TIME);
        if at == 0 || at_slot {
            info!("update: auto-update, installing {version}");
            // Restarts once installed.
            super::install(url).await;
            return false;
        }
        let triple = (version.major, version.minor, version.patch);
        let scheduled = store.lock().await.load::<Scheduled>().await.ok().flatten();
        let staged = crate::ota::esp::staged(store, fw.version).await;
        let ready = scheduled.is_some_and(|s| s.version == triple) && staged.is_some_and(|s| s.version == triple);
        if !ready {
            info!("update: auto-update, downloading {version} for {}", TIMES[usize::from(at)]);
            if let Err(e) = crate::ota::esp::request_stage(url).await {
                warn!("update: download failed: {e}");
                return false;
            }
            let due = crate::clock::now().map_or(0, |now| crate::clock::zone().next(now, minute(at)) as u32);
            let _ = store.lock().await.save(&Scheduled { version: triple, due }).await;
            DUE.store(due, Ordering::Relaxed);
        } else if DUE.load(Ordering::Relaxed) == 0 {
            // Downloaded before the clock was known (an always-on device): due from now on.
            install_if_due(store, fw).await;
        }
        let mut summary: String<64> = String::new();
        let _ = write!(summary, "Installs at {}", TIMES[usize::from(at)]);
        let latest = FIRMWARE.update().map(|u| (u.latest, u.release_url)).unwrap_or_default();
        FIRMWARE.set_latest(&latest.0, &summary, &latest.1);
        true
    }

    /// Installs the scheduled update if its time has come (restarts into it).
    #[cfg(esp_scheduled)]
    async fn install_if_due(store: &'static SharedStore, fw: Firmware) {
        let Some(mut s) = store.lock().await.load::<Scheduled>().await.ok().flatten() else { return };
        let version = Version::new(s.version.0, s.version.1, s.version.2);
        if version <= fw.version {
            let _ = store.lock().await.remove::<Scheduled>().await;
            return;
        }
        let Some(now) = crate::clock::now() else { return };
        if s.due == 0 {
            s.due = match UPDATE_TIME.get().unwrap_or(DEFAULT_TIME) {
                0 => now as u32,
                at => crate::clock::zone().next(now, minute(at)) as u32,
            };
            let _ = store.lock().await.save(&s).await;
        }
        DUE.store(s.due, Ordering::Relaxed);
        if now < i64::from(s.due) {
            return;
        }
        let staged = crate::ota::esp::staged(store, fw.version).await.filter(|st| st.version == s.version);
        let _ = store.lock().await.remove::<Scheduled>().await;
        DUE.store(0, Ordering::Relaxed);
        let Some(staged) = staged else {
            warn!("update: the downloaded {version} is gone, downloading it again at the next check");
            return;
        };
        info!("update: installing {version}, as scheduled");
        FIRMWARE.set_progress(true, None);
        let mut sector = [0xff; core::mem::size_of::<crate::ota::esp::Sector>()];
        match crate::ota::esp::apply_staged(store, &staged, &mut sector).await {
            Ok(_) => crate::diag::esp::restart().await,
            Err(e) => {
                warn!("update: scheduled install failed: {e}");
                FIRMWARE.set_progress(false, None);
            }
        }
    }

    /// Saves the settings changed in Home Assistant, and installs when the time comes while awake.
    /// Returns when *Auto-update* is turned on, or *Update time* changed while it is: the check
    /// that follows schedules a release found already, and shows its new time.
    #[cfg(esp_scheduled)]
    pub(crate) async fn run(store: &'static SharedStore, fw: Firmware) {
        loop {
            let due = async {
                match seconds_to_due() {
                    Some(s) => Timer::after_secs(s).await,
                    // No clock yet: once Home Assistant sends it, a download waiting for it is due.
                    None if crate::clock::now().is_none() => crate::clock::KNOWN.wait().await,
                    None => core::future::pending().await,
                }
            };
            match select(select(AUTO_UPDATE.wait_command(), UPDATE_TIME.wait_command()), due).await {
                Either::First(changed) => {
                    let check = match changed {
                        Either::First(on) => on && AUTO_UPDATE.get() != Some(true),
                        Either::Second(_) => AUTO_UPDATE.get() == Some(true),
                    };
                    match changed {
                        Either::First(on) => AUTO_UPDATE.set(on),
                        Either::Second(at) => UPDATE_TIME.set(at),
                    }
                    let settings =
                        Settings { on: AUTO_UPDATE.get() == Some(true), at: UPDATE_TIME.get().unwrap_or(DEFAULT_TIME) };
                    let mut store_ = store.lock().await;
                    let _ = store_.save(&settings).await;
                    // Off: the update downloaded waits for a click; a new time: it is due then.
                    DUE.store(0, Ordering::Relaxed);
                    if let Some(mut s) = store_.load::<Scheduled>().await.ok().flatten() {
                        if settings.on {
                            s.due = 0;
                            let _ = store_.save(&s).await;
                        } else {
                            let _ = store_.remove::<Scheduled>().await;
                        }
                    }
                    drop(store_);
                    install_if_due(store, fw).await;
                    if check {
                        return;
                    }
                }
                Either::Second(()) => install_if_due(store, fw).await,
            }
        }
    }
}

#[cfg(all(esp_scheduled, feature = "deep-sleep"))]
pub(crate) use auto::seconds_to_due;

/// The release last found, kept in flash: a deep-sleeping device checks only now and then, and
/// shows it in between.
#[cfg(feature = "deep-sleep")]
mod known {
    use heapless::String;
    use serde::{Deserialize, Serialize};

    use super::{CHECK_EVERY, FIRMWARE};
    use crate::config::{Setting, esp::SharedStore, keys};
    use crate::model::Entity;
    use crate::ota::esp::Firmware;

    #[derive(PartialEq, Eq, Serialize, Deserialize)]
    struct Known {
        latest: String<16>,
        summary: String<64>,
        release_url: String<96>,
    }

    impl Setting for Known {
        const KEY: u16 = keys::OTA_KNOWN;
        const VERSION: u8 = 1;
    }

    /// Whether to check now, and whether it is the update time (`esp_scheduled`).
    pub(super) fn due() -> (bool, bool) {
        let Some(since) = crate::app::sleep::since_check() else { return (true, false) };
        #[cfg(esp_scheduled)]
        if super::auto::slot_within(since) {
            return (true, true);
        }
        (since >= CHECK_EVERY.as_secs() as u32, false)
    }

    /// Shows the release last found, if it is newer than this firmware.
    pub(super) async fn restore(store: &SharedStore, fw: Firmware) {
        let Ok(Some(k)) = store.lock().await.load::<Known>().await else { return };
        if crate::image::Version::parse(&k.latest).is_ok_and(|v| v > fw.version) {
            FIRMWARE.set_latest(&k.latest, &k.summary, &k.release_url);
        }
    }

    /// Keeps what was found (only when it changed: flash writes are rare).
    pub(super) async fn save(store: &SharedStore, fw: Firmware) {
        let Some(u) = FIRMWARE.update() else { return };
        let mut store = store.lock().await;
        let stored = store.load::<Known>().await.ok().flatten();
        if u.latest.is_empty() || u.latest == fw.version_text {
            if stored.is_some() {
                let _ = store.remove::<Known>().await;
            }
            return;
        }
        let k = Known { latest: u.latest, summary: u.summary, release_url: u.release_url };
        if stored.as_ref() != Some(&k) {
            let _ = store.save(&k).await;
        }
    }
}
