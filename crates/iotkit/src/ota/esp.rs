//! OTA on the chip: slots, download into flash, boot handling, confirmation, and a small HTTP
//! endpoint (`cargo xtask ota push`).

use core::fmt::{self, Write as _};

use crate::config::esp::{SharedStore, with};
use crate::image::{Digest, Manifest, PublicKey, Sha256, Version, app};
use crate::net::{
    esp::{GetError, Net, Sink, get, get_from},
    http,
};
#[cfg(any(not(feature = "ha-update"), not(feature = "ota-push")))]
use embassy_futures::select::{Either, select};
#[cfg(feature = "ha-update")]
use embassy_futures::select::{Either3, select3};
use embassy_net::tcp::TcpSocket;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex, signal::Signal};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
use esp_bootloader_esp_idf::{
    ota::{Ota, OtaImageState},
    partitions::{
        AppPartitionSubType, DataPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionEntry, PartitionType,
        read_partition_table,
    },
};
use esp_println::println;
use esp_storage::FlashStorage;
use heapless::String;

use crate::ota::{
    BootAction, LastUpdate, Outcome, Pending,
    atomic::{AtomicBool, Ordering},
    early_check, on_boot,
};

/// The chip this firmware runs on, as the image header names it.
#[cfg(feature = "esp32c6")]
const CHIP_ID: u16 = app::ESP32C6;
#[cfg(feature = "esp32c3")]
const CHIP_ID: u16 = app::ESP32C3;
#[cfg(feature = "esp32c5")]
const CHIP_ID: u16 = app::ESP32C5;
#[cfg(feature = "esp32s3")]
const CHIP_ID: u16 = app::ESP32S3;
#[cfg(feature = "esp32")]
const CHIP_ID: u16 = app::ESP32;

/// Raw subtype of `ota_0`; `ota_n` is this plus n.
const OTA_SUBTYPE: u8 = 0x10;
const SECTOR: usize = FlashStorage::SECTOR_SIZE as usize;
/// Largest manifest accepted.
const MAX_MANIFEST: usize = 1024;
/// Downloads of the image, the first included, before giving up.
const ATTEMPTS: u32 = 5;
/// Port of the push endpoint.
pub const PORT: u16 = 80;

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
        $crate::ota::esp::Firmware {
            product: env!("CARGO_PKG_NAME"),
            version: match $crate::ota::__version(env!("CARGO_PKG_VERSION")) {
                Some(v) => v,
                None => panic!("CARGO_PKG_VERSION is not major.minor.patch"),
            },
            version_text: env!("CARGO_PKG_VERSION"),
            key: $key,
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Why an update failed.
pub enum OtaError {
    /// Another update is running.
    Busy,
    /// No `otadata` and two OTA app partitions, or the running app is not in one.
    NoSlots,
    /// The manifest or image could not be downloaded.
    Download(GetError<()>),
    /// The manifest is invalid, badly signed, or not an update for this firmware.
    Manifest(crate::image::Error),
    /// The image does not match the manifest.
    Image(crate::image::Error),
    /// The image is larger than the OTA slot.
    TooLarge,
    /// Writing to flash failed.
    Flash,
}

impl fmt::Display for OtaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OtaError::Busy => f.write_str("an update is already running"),
            OtaError::NoSlots => f.write_str("no OTA partitions (see partitions.csv)"),
            OtaError::Download(e) => write!(f, "download failed: {e:?}"),
            OtaError::Manifest(e) => write!(f, "manifest: {e}"),
            OtaError::Image(e) => write!(f, "image: {e}"),
            OtaError::TooLarge => f.write_str("image larger than the OTA slot"),
            OtaError::Flash => f.write_str("flash error"),
        }
    }
}

fn slot_number(p: &PartitionEntry) -> Option<u8> {
    match p.partition_type() {
        PartitionType::App(s) if !matches!(s, AppPartitionSubType::Factory | AppPartitionSubType::Test) => {
            Some(s as u8 - OTA_SUBTYPE)
        }
        _ => None,
    }
}

fn subtype(slot: u8) -> Result<AppPartitionSubType, OtaError> {
    AppPartitionSubType::try_from(OTA_SUBTYPE + slot).map_err(|_| OtaError::NoSlots)
}

/// The OTA partitions and which one runs.
struct Layout {
    otadata: PartitionEntry,
    slots: usize,
    booted: Option<u8>,
    /// Where an update goes: the slot after the booted one.
    target: u8,
    target_offset: u32,
    target_len: u32,
}

impl Layout {
    fn read(flash: &mut FlashStorage<'_>) -> Result<Self, OtaError> {
        let mut buf = [0; PARTITION_TABLE_MAX_LEN];
        let table = read_partition_table(flash, &mut buf).map_err(|_| OtaError::NoSlots)?;
        let otadata = table
            .find_partition(PartitionType::Data(DataPartitionSubType::Ota))
            .ok()
            .flatten()
            .ok_or(OtaError::NoSlots)?;
        let slots = table.iter().filter(|p| slot_number(p).is_some()).count();
        if slots < 2 {
            return Err(OtaError::NoSlots);
        }
        let booted = table.booted_partition().ok().flatten().as_ref().and_then(slot_number);
        let target = booted.map_or(0, |b| (b + 1) % slots as u8);
        let entry = table.iter().find(|p| slot_number(p) == Some(target)).ok_or(OtaError::NoSlots)?;
        Ok(Self { otadata, slots, booted, target, target_offset: entry.offset(), target_len: entry.len() })
    }

    fn ota<'a, 'd>(&self, flash: &'a mut FlashStorage<'d>) -> Result<Ota<'a, 'd>, OtaError> {
        Ota::new(self.otadata.as_flash_region(flash), self.slots).map_err(|_| OtaError::Flash)
    }
}

fn triple(v: Version) -> (u16, u16, u16) {
    (v.major, v.minor, v.patch)
}

/// Handles a pending update at boot: counts a trial boot, or rolls back (and restarts). Call
/// early, right after opening the store.
pub async fn boot(store: &SharedStore) -> BootAction {
    let found = with(|flash| {
        let layout = Layout::read(flash)?;
        let unconfirmed = unconfirmed(&mut layout.ota(flash)?);
        Ok::<_, OtaError>((layout.booted, layout.slots, unconfirmed))
    });
    let booted = found.ok().and_then(|(booted, _, _)| booted);
    let mut store = store.lock().await;
    let mut pending = store.load::<Pending>().await.ok().flatten();
    // The settings were lost (erased as corrupted, say) while an update is on trial: otadata,
    // in its own partition, still says so. Count its trial boots again rather than keep it.
    if pending.is_none()
        && let Ok((Some(slot), slots, true)) = found
    {
        log::warn!("ota: this firmware is not confirmed but its record is gone: on trial again");
        let previous = (slot + slots as u8 - 1) % slots as u8;
        pending = Some(Pending { slot, previous, version: (0, 0, 0), boots: 0 });
    }
    let action = on_boot(pending.as_ref(), booted);
    let Some(p) = pending else { return action };
    match action {
        BootAction::Normal => {}
        BootAction::Trial { boot } => {
            log::info!(
                "ota: trial boot {boot}/{} of {}, {} s to get healthy",
                crate::ota::MAX_TRIAL_BOOTS,
                p.version(),
                crate::ota::UPDATE_TIMEOUT_S
            );
            TRIAL.store(true, Ordering::Release);
            if store.save(&Pending { boots: boot, ..p }).await.is_err() {
                log::error!("ota: cannot count the trial boot");
            }
        }
        BootAction::RollBack { to } => {
            log::error!("ota: {} was not confirmed after {} boots, back to ota_{to}", p.version(), p.boots);
            if let Err(e) = switch_to(to) {
                // Keep trying the update rather than loop on a failing switch.
                log::error!("ota: rollback failed: {e}");
                return action;
            }
            let _ = store.remove::<Pending>().await;
            let _ = store.save(&LastUpdate { outcome: Outcome::RolledBack, version: p.version }).await;
            drop(store);
            esp_hal::system::software_reset();
        }
        BootAction::Abandoned => {
            log::warn!("ota: running the previous firmware, update to {} abandoned", p.version());
            let _ = store.remove::<Pending>().await;
            let _ = store.save(&LastUpdate { outcome: Outcome::RolledBack, version: p.version }).await;
        }
    }
    action
}

/// Selects slot `to`, as a firmware that works.
fn switch_to(to: u8) -> Result<(), OtaError> {
    with(|flash| {
        let layout = Layout::read(flash)?;
        let mut ota = layout.ota(flash)?;
        ota.set_current_app_partition(subtype(to)?).map_err(|_| OtaError::Flash)?;
        ota.set_current_ota_state(OtaImageState::Valid).map_err(|_| OtaError::Flash)
    })
}

/// An update on trial: not confirmed yet (a deep-sleeping device does not sleep then).
static TRIAL: AtomicBool = AtomicBool::new(false);

/// Whether the running firmware is an update on trial (see [`boot`]).
pub fn on_trial() -> bool {
    TRIAL.load(Ordering::Acquire)
}

/// The update on trial was not healthy in time (`missing`: the checks not passed): goes back to
/// the previous firmware and restarts. Returns if it cannot switch back.
pub async fn give_up(store: &SharedStore, missing: u32) {
    let mut store = store.lock().await;
    let Some(p) = store.load::<Pending>().await.ok().flatten() else { return };
    log::error!(
        "ota: {} not healthy within {} s (checks missing: {missing:#x}), back to ota_{}",
        p.version(),
        crate::ota::UPDATE_TIMEOUT_S,
        p.previous
    );
    if let Err(e) = switch_to(p.previous) {
        log::error!("ota: rollback failed: {e}");
        return;
    }
    let _ = store.remove::<Pending>().await;
    let _ = store.save(&LastUpdate { outcome: Outcome::RolledBack, version: p.version }).await;
    drop(store);
    crate::diag::esp::restart().await
}

/// Whether otadata says the running firmware is an update not confirmed yet.
fn unconfirmed(ota: &mut Ota<'_, '_>) -> bool {
    matches!(ota.current_ota_state(), Ok(OtaImageState::New | OtaImageState::PendingVerify))
}

/// Boots of an unconfirmed firmware, counted in RTC memory (kept across resets, not power
/// loss): see [`early_check`].
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut EARLY: [u32; 2] = [0; 2];

/// Run by [`init!`](crate::init) right after `esp_hal::init` and `crate::config::esp::init`,
/// before the heap, the RTOS and the app, so little can crash before it (the logger is not up:
/// it prints). Counts the boots of an unconfirmed update in RTC memory, and goes back to the
/// previous firmware past twice the trial boots: an update that crashes before reaching [`boot`]
/// (which counts them in flash) is rolled back too.
pub fn early_guard() {
    fn check(flash: &mut FlashStorage<'_>) -> Result<Option<(u8, u32)>, OtaError> {
        let layout = Layout::read(flash)?;
        let mut ota = layout.ota(flash)?;
        let early: *mut [u32; 2] = &raw mut EARLY;
        // SAFETY: core 0 only, at boot, and nothing else touches it.
        let early = unsafe { &mut *early };
        let (counter, roll_back) = early_check(unconfirmed(&mut ota), *early);
        *early = counter;
        if !roll_back {
            return Ok(None);
        }
        let booted = layout.booted.ok_or(OtaError::NoSlots)?;
        let to = (booted + layout.slots as u8 - 1) % layout.slots as u8;
        ota.set_current_app_partition(subtype(to)?).map_err(|_| OtaError::Flash)?;
        ota.set_current_ota_state(OtaImageState::Valid).map_err(|_| OtaError::Flash)?;
        *early = [0; 2];
        Ok(Some((to, counter[1])))
    }
    match with(check) {
        Ok(Some((to, boots))) => {
            println!("ota: unconfirmed firmware booted {boots} times without counting them, back to ota_{to}");
            esp_hal::system::software_reset();
        }
        Ok(None) | Err(OtaError::NoSlots) => {}
        Err(e) => println!("ota: early check: {e}"),
    }
}

/// The firmware works: keeps it (after an update), so later boots skip the trial.
pub async fn confirm(store: &SharedStore) {
    let mut store = store.lock().await;
    let pending = store.load::<Pending>().await.ok().flatten();
    let marked = with(|flash| {
        let layout = Layout::read(flash)?;
        let mut ota = layout.ota(flash)?;
        match ota.current_ota_state() {
            Ok(OtaImageState::New | OtaImageState::PendingVerify) => {
                ota.set_current_ota_state(OtaImageState::Valid).map_err(|_| OtaError::Flash)
            }
            _ => Ok(()),
        }
    });
    if let Err(e) = marked
        && e != OtaError::NoSlots
    {
        log::error!("ota: cannot mark the firmware valid: {e}");
    }
    if let Some(p) = pending {
        let _ = store.remove::<Pending>().await;
        let _ = store.save(&LastUpdate { outcome: Outcome::Confirmed, version: p.version }).await;
        log::info!("ota: {} confirmed", p.version());
    }
    TRIAL.store(false, Ordering::Release);
}

static BUSY: AtomicBool = AtomicBool::new(false);

/// An install is asked for, running, or done and waiting for the restart (a deep-sleeping
/// device does not sleep then).
pub fn installing() -> bool {
    BUSY.load(Ordering::Acquire) || REQUEST.signaled()
}
/// An install asked for by [`request_install`], run by [`serve_task`] (or `install_task`).
static REQUEST: Signal<CriticalSectionRawMutex, String<{ http::MAX_URL }>> = Signal::new();
static RESULT: Signal<CriticalSectionRawMutex, Result<Version, OtaError>> = Signal::new();
/// One [`request_install`] at a time.
static REQUESTING: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

/// Has [`serve_task`] or `install_task` (one must run) install the update whose manifest is at
/// `manifest_url`. On success the new firmware is selected: restart to boot it (on trial). Only
/// that task runs an install, so its large buffers exist once.
pub async fn request_install(manifest_url: &str) -> Result<Version, OtaError> {
    let Ok(_one) = REQUESTING.try_lock() else { return Err(OtaError::Busy) };
    if BUSY.load(Ordering::Acquire) {
        return Err(OtaError::Busy);
    }
    let url = String::try_from(manifest_url).map_err(|_| OtaError::Download(GetError::Url(http::Error::Url)))?;
    RESULT.reset();
    REQUEST.signal(url);
    RESULT.wait().await
}

/// A newer release, from its manifest.
#[cfg(feature = "ha-update")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: Version,
    /// What it changes (empty: none).
    pub summary: String<{ crate::image::MAX_SUMMARY_LEN }>,
    /// Where its notes are (empty: none).
    pub release_url: String<{ crate::image::MAX_RELEASE_URL_LEN }>,
}

/// A check asked for by [`request_check`], run by [`serve_task`] (or `install_task`).
#[cfg(feature = "ha-update")]
static CHECK: Signal<CriticalSectionRawMutex, String<{ http::MAX_URL }>> = Signal::new();
#[cfg(feature = "ha-update")]
static CHECKED: Signal<CriticalSectionRawMutex, Result<Option<Release>, OtaError>> = Signal::new();

/// Has [`serve_task`] or `install_task` fetch and verify the manifest at `manifest_url`: the
/// release if it is an update for this firmware, `None` if it is not newer.
#[cfg(feature = "ha-update")]
pub async fn request_check(manifest_url: &str) -> Result<Option<Release>, OtaError> {
    let Ok(_one) = REQUESTING.try_lock() else { return Err(OtaError::Busy) };
    let url = String::try_from(manifest_url).map_err(|_| OtaError::Download(GetError::Url(http::Error::Url)))?;
    CHECKED.reset();
    CHECK.signal(url);
    CHECKED.wait().await
}

#[cfg(feature = "ha-update")]
async fn check(net: Net, fw: Firmware, manifest_url: &str) -> Result<Option<Release>, OtaError> {
    let mut collect = Collect { buf: [0; MAX_MANIFEST], len: 0 };
    get(net.stack, manifest_url, &mut collect).await.map_err(|e| OtaError::Download(drop_sink(e)))?;
    let manifest = Manifest::parse(&collect.buf[..collect.len]).map_err(OtaError::Manifest)?;
    manifest.verify(fw.key).map_err(OtaError::Manifest)?;
    match manifest.check_update(fw.product, fw.version) {
        Ok(()) => Ok(Some(Release {
            version: manifest.version,
            // Both fit: the manifest's lengths are checked when parsed.
            summary: String::try_from(manifest.summary).unwrap_or_default(),
            release_url: String::try_from(manifest.release_url).unwrap_or_default(),
        })),
        Err(crate::image::Error::NotNewer) => Ok(None),
        Err(e) => Err(OtaError::Manifest(e)),
    }
}

/// Download progress of the install running, in % (in steps of 5).
#[cfg(feature = "ha-update")]
static PROGRESS: Signal<CriticalSectionRawMutex, u8> = Signal::new();

/// Waits for the next progress step of an install (0-100 %).
#[cfg(feature = "ha-update")]
pub async fn wait_progress() -> u8 {
    PROGRESS.wait().await
}

/// Downloads and installs the update described by the manifest at `manifest_url`. On success
/// the new firmware is selected: restart to boot it (on trial).
async fn install(net: Net, store: &SharedStore, fw: Firmware, manifest_url: &str) -> Result<Version, OtaError> {
    /// Clears [`BUSY`] however the install fails, a dropped future included. After a success it
    /// stays set: the selected image waits for the restart, and must not be overwritten.
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::Release);
        }
    }
    if BUSY.swap(true, Ordering::Acquire) {
        return Err(OtaError::Busy);
    }
    let guard = Guard;
    let installed = install_inner(net, store, fw, manifest_url).await;
    if installed.is_ok() {
        core::mem::forget(guard);
    }
    installed
}

async fn install_inner(net: Net, store: &SharedStore, fw: Firmware, manifest_url: &str) -> Result<Version, OtaError> {
    let mut collect = Collect { buf: [0; MAX_MANIFEST], len: 0 };
    get(net.stack, manifest_url, &mut collect).await.map_err(|e| OtaError::Download(drop_sink(e)))?;
    let manifest = Manifest::parse(&collect.buf[..collect.len]).map_err(OtaError::Manifest)?;
    manifest.verify(fw.key).map_err(OtaError::Manifest)?;
    manifest.check_update(fw.product, fw.version).map_err(OtaError::Manifest)?;

    let layout = with(Layout::read)?;
    let previous = layout.booted.ok_or(OtaError::NoSlots)?;
    if manifest.size > layout.target_len {
        return Err(OtaError::TooLarge);
    }
    let url = http::Url::parse(manifest_url)
        .and_then(|base| base.join(manifest.url))
        .map_err(|e| OtaError::Download(GetError::Url(e)))?;
    log::info!(
        "ota: installing {} {} ({} KiB) into ota_{}",
        manifest.product,
        manifest.version,
        manifest.size / 1024,
        layout.target
    );

    let mut sink = FlashSink::new(&manifest, layout.target_offset, layout.target_len);
    let started = embassy_time::Instant::now();
    let mut downloaded = Ok(());
    for attempt in 1..=ATTEMPTS {
        downloaded = get_from(net.stack, &url, sink.received, &mut sink).await;
        match downloaded {
            // A dropped connection: carry on from where it stopped.
            Err(GetError::Truncated { .. } | GetError::Connect | GetError::Io) if attempt < ATTEMPTS => {
                log::warn!(
                    "ota: download interrupted at {} of {} bytes ({:?}), resuming",
                    sink.received,
                    manifest.size,
                    downloaded
                );
                Timer::after_secs(1).await;
            }
            _ => break,
        }
    }
    let (size, sha256) = sink.finish();
    let ms = started.elapsed().as_millis().max(1) as u32;
    log::info!("ota: {size} bytes in {}.{:01} s ({} KiB/s)", ms / 1000, ms % 1000 / 100, size / ms * 1000 / 1024);
    let checked = match downloaded {
        Ok(()) => manifest.check_image(size, &sha256).map_err(OtaError::Image),
        Err(GetError::Sink(e)) => Err(e),
        Err(e) => Err(OtaError::Download(drop_sink(e))),
    };
    if let Err(e) = checked {
        // Never leave a bootable partial image behind.
        let _ = with(|flash| flash.erase(layout.target_offset, layout.target_offset + SECTOR as u32));
        return Err(e);
    }

    // `Pending` first, then the switch: a power cut in between boots the old slot, which forgets
    // the update (`Abandoned`). The other order could boot the update with no trial count.
    let mut store = store.lock().await;
    let version = triple(manifest.version);
    store.save(&Pending { slot: layout.target, previous, version, boots: 0 }).await.map_err(|_| OtaError::Flash)?;
    let switched = with(|flash| {
        let mut ota = layout.ota(flash)?;
        // Blank OTA data (just flashed over USB) selects no slot: record the running one first,
        // so the switch counts from it.
        if ota.current_app_partition().map_err(|_| OtaError::Flash)? == AppPartitionSubType::Factory {
            ota.set_current_app_partition(subtype(previous)?).map_err(|_| OtaError::Flash)?;
        }
        ota.set_current_app_partition(subtype(layout.target)?).map_err(|_| OtaError::Flash)?;
        ota.set_current_ota_state(OtaImageState::New).map_err(|_| OtaError::Flash)
    });
    if let Err(e) = switched {
        let _ = store.remove::<Pending>().await;
        return Err(e);
    }
    let _ = store.save(&LastUpdate { outcome: Outcome::Installed, version }).await;
    log::info!("ota: {} installed, restart to run it", manifest.version);
    Ok(manifest.version)
}

fn drop_sink<E>(e: GetError<E>) -> GetError<()> {
    match e {
        GetError::Url(e) => GetError::Url(e),
        GetError::Dns => GetError::Dns,
        GetError::Connect => GetError::Connect,
        #[cfg(feature = "tls")]
        GetError::Tls => GetError::Tls,
        GetError::Io => GetError::Io,
        GetError::BadResponse(e) => GetError::BadResponse(e),
        GetError::Status(s) => GetError::Status(s),
        GetError::Truncated { received, expected } => GetError::Truncated { received, expected },
        GetError::Sink(_) => GetError::Sink(()),
    }
}

/// Keeps the manifest in memory.
struct Collect {
    buf: [u8; MAX_MANIFEST],
    len: usize,
}

impl Sink for Collect {
    type Error = ();
    fn begin(&mut self, len: Option<u32>) -> Result<(), ()> {
        if len.is_some_and(|l| l as usize > MAX_MANIFEST) { Err(()) } else { Ok(()) }
    }
    async fn data(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let dst = self.buf.get_mut(self.len..self.len + bytes.len()).ok_or(())?;
        dst.copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

/// Writes the image to the target slot sector by sector, checking its header and hashing it.
struct FlashSink<'m> {
    manifest: &'m Manifest<'m>,
    offset: u32,
    capacity: u32,
    sector: [u8; SECTOR],
    fill: usize,
    written: u32,
    received: u32,
    hasher: Sha256,
    header_checked: bool,
}

impl<'m> FlashSink<'m> {
    fn new(manifest: &'m Manifest<'m>, offset: u32, capacity: u32) -> Self {
        Self {
            manifest,
            offset,
            capacity,
            sector: [0xff; SECTOR],
            fill: 0,
            written: 0,
            received: 0,
            hasher: Sha256::default(),
            header_checked: false,
        }
    }

    fn flush(&mut self) -> Result<(), OtaError> {
        if self.fill == 0 {
            return Ok(());
        }
        // Writes are 4-byte aligned: pad the last one with erased bytes.
        let len = self.fill.next_multiple_of(4);
        self.sector[self.fill..len].fill(0xff);
        let at = self.offset + self.written;
        with(|flash| {
            flash.erase(at, at + SECTOR as u32)?;
            flash.write(at, &self.sector[..len])
        })
        .map_err(|e| {
            log::error!("ota: flash write at {at:#x} failed: {e:?}");
            OtaError::Flash
        })?;
        #[cfg(feature = "ha-update")]
        let before = self.written * 20 / self.manifest.size.max(1);
        self.written += self.fill as u32;
        self.fill = 0;
        #[cfg(feature = "ha-update")]
        {
            let step = self.written * 20 / self.manifest.size.max(1);
            if step != before {
                PROGRESS.signal((step * 5).min(100) as u8);
            }
        }
        Ok(())
    }

    /// Bytes received and their SHA-256.
    fn finish(&mut self) -> (u32, [u8; 32]) {
        let digest = core::mem::take(&mut self.hasher).finalize().into();
        (self.received, digest)
    }
}

impl Sink for FlashSink<'_> {
    type Error = OtaError;

    /// `len`: what is left of the image (all of it, or the rest after a resume).
    fn begin(&mut self, len: Option<u32>) -> Result<(), OtaError> {
        match len {
            Some(len) if self.received + len != self.manifest.size => Err(OtaError::Image(crate::image::Error::Size)),
            _ => Ok(()),
        }
    }

    async fn data(&mut self, mut bytes: &[u8]) -> Result<(), OtaError> {
        self.received += bytes.len() as u32;
        if self.received > self.capacity || self.received > self.manifest.size {
            return Err(OtaError::TooLarge);
        }
        self.hasher.update(bytes);
        while !bytes.is_empty() {
            let n = bytes.len().min(SECTOR - self.fill);
            self.sector[self.fill..self.fill + n].copy_from_slice(&bytes[..n]);
            self.fill += n;
            bytes = &bytes[n..];
            if !self.header_checked && self.written == 0 && self.fill >= app::HEAD_LEN {
                self.manifest.check_app(&self.sector[..self.fill], CHIP_ID).map_err(OtaError::Image)?;
                self.header_checked = true;
            }
            if self.fill == SECTOR {
                self.flush()?;
            }
        }
        // Keep `received` and the flash in step at the end of the body.
        if self.received == self.manifest.size {
            if !self.header_checked {
                return Err(OtaError::Image(crate::image::Error::NotAnImage));
            }
            self.flush()?;
        }
        Ok(())
    }
}

/// HTTP endpoint on port 80, and the installs of [`request_install`]:
/// - `GET /` answers `<product> <version>`;
/// - `GET /ota?manifest=<url>` installs that update, answers the result, then restarts.
#[embassy_executor::task]
pub async fn serve_task(net: Net, store: &'static SharedStore, fw: Firmware) {
    let (mut rx, mut tx) = ([0u8; 1024], [0u8; 512]);
    loop {
        let mut socket = TcpSocket::new(net.stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(10)));
        #[cfg(not(feature = "ha-update"))]
        match select(socket.accept(PORT), REQUEST.wait()).await {
            Either::First(Ok(())) => {}
            Either::First(Err(_)) => continue,
            Either::Second(url) => {
                drop(socket);
                RESULT.signal(install(net, store, fw, &url).await);
                continue;
            }
        }
        #[cfg(feature = "ha-update")]
        match select3(socket.accept(PORT), REQUEST.wait(), CHECK.wait()).await {
            Either3::First(Ok(())) => {}
            Either3::First(Err(_)) => continue,
            Either3::Second(url) => {
                drop(socket);
                RESULT.signal(install(net, store, fw, &url).await);
                continue;
            }
            Either3::Third(url) => {
                drop(socket);
                CHECKED.signal(check(net, fw, &url).await);
                continue;
            }
        }
        let mut buf = [0u8; 512];
        let mut filled = 0;
        let head_end = loop {
            match with_timeout(Duration::from_secs(5), socket.read(&mut buf[filled..])).await {
                Ok(Ok(n)) if n > 0 => filled += n,
                _ => break None,
            }
            if let Some(end) = buf[..filled].windows(4).position(|w| w == b"\r\n\r\n") {
                break Some(end);
            }
            if filled == buf.len() {
                break None;
            }
        };
        let Some(end) = head_end else {
            socket.abort();
            continue;
        };
        let line = core::str::from_utf8(&buf[..end]).ok().and_then(|h| h.lines().next()).unwrap_or("");
        let mut url: String<{ http::MAX_URL }> = String::new();
        let mut reply: String<192> = String::new();
        let (status, restart) = match http::parse_request_line(line) {
            Some(("GET", "/")) => {
                let _ = writeln!(reply, "{} {}", fw.product, fw.version);
                ("200 OK", false)
            }
            Some(("GET", target)) if http::target_path(target) == "/ota" => {
                match http::query_param(target, "manifest").map(|m| url.push_str(m)) {
                    Some(Ok(())) => {
                        log::info!("ota: push from the network: {url}");
                        // The client waits, silent, for the whole install.
                        socket.set_timeout(None);
                        match install(net, store, fw, &url).await {
                            Ok(v) => {
                                let _ = writeln!(reply, "installed {v}, restarting");
                                ("200 OK", true)
                            }
                            Err(e) => {
                                log::error!("ota: {e}");
                                let _ = writeln!(reply, "{e}");
                                ("500 Internal Server Error", false)
                            }
                        }
                    }
                    _ => {
                        let _ = reply.push_str("usage: /ota?manifest=http://host:port/path/manifest.json\n");
                        ("400 Bad Request", false)
                    }
                }
            }
            _ => {
                let _ = reply.push_str("not found\n");
                ("404 Not Found", false)
            }
        };
        let mut head: String<128> = String::new();
        let _ = write!(head, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(reply.as_bytes()).await;
        let _ = socket.flush().await;
        socket.close();
        // Wait for the client's FIN: dropping the socket earlier would leave it waiting.
        let _ = with_timeout(Duration::from_secs(2), async {
            let mut rest = [0u8; 64];
            while let Ok(n) = socket.read(&mut rest).await {
                if n == 0 {
                    break;
                }
            }
        })
        .await;
        if restart {
            // Let the reply leave first.
            Timer::after_millis(500).await;
            crate::diag::esp::restart().await;
        }
    }
}

/// The installs and checks of [`request_install`] and [`request_check`] without the port-80
/// endpoint: `ha-update` without `ota-push`.
#[cfg(all(feature = "ha-update", not(feature = "ota-push")))]
#[embassy_executor::task]
pub async fn install_task(net: Net, store: &'static SharedStore, fw: Firmware) {
    loop {
        match select(REQUEST.wait(), CHECK.wait()).await {
            Either::First(url) => RESULT.signal(install(net, store, fw, &url).await),
            Either::Second(url) => CHECKED.signal(check(net, fw, &url).await),
        }
    }
}
