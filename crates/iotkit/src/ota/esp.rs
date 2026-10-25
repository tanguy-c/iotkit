//! OTA on the chip: slots, writing an image into flash, boot handling and confirmation. The
//! transports feed [`Install`]: HTTP (`http`, `cargo xtask ota push` and Home Assistant) and Matter
//! (`matter::ota`).

use core::fmt;

use crate::config::esp::{EspStore, SharedStore, with};
use crate::image::{Digest, Manifest, Sha256, Version, app};
#[cfg(feature = "ha-update")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use esp_bootloader_esp_idf::{
    ota::{Ota, OtaImageState},
    partitions::{
        AppPartitionSubType, DataPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionEntry, PartitionType,
        read_partition_table,
    },
};
use esp_println::println;
use esp_storage::FlashStorage;

#[cfg(esp_stage)]
pub use crate::ota::Staged;
use crate::ota::{
    BootAction, LastUpdate, Outcome, Pending,
    atomic::{AtomicBool, Ordering},
    early_check, on_boot,
};

#[cfg(esp_wifi)]
mod http;
#[cfg(esp_wifi)]
pub use http::*;

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
#[cfg(feature = "esp32h2")]
const CHIP_ID: u16 = app::ESP32H2;

/// Raw subtype of `ota_0`; `ota_n` is this plus n.
const OTA_SUBTYPE: u8 = 0x10;
const SECTOR: usize = FlashStorage::SECTOR_SIZE as usize;

/// Whether an update is being installed, or the running one is on trial: no other may start.
pub fn busy() -> bool {
    on_trial() || BUSY.load(Ordering::Acquire)
}

/// Whether an update is being written (or waits for the restart).
pub fn writing() -> bool {
    BUSY.load(Ordering::Acquire)
}

pub use super::Firmware;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Why an update failed.
pub enum OtaError {
    /// Another update is running, or the running one is on trial (its spare slot holds the
    /// firmware to go back to).
    Busy,
    /// No `otadata` and two OTA app partitions, or the running app is not in one.
    NoSlots,
    /// The manifest or image could not be downloaded.
    #[cfg(esp_wifi)]
    Download(crate::net::esp::GetError<()>),
    /// The transfer over Matter (query, BDX download) failed.
    #[cfg(feature = "matter-ota")]
    Matter(rs_matter_embassy::matter::error::ErrorCode),
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
            OtaError::Busy => f.write_str("an update is running or on trial"),
            OtaError::NoSlots => f.write_str("no OTA partitions (see partitions.csv)"),
            #[cfg(esp_wifi)]
            OtaError::Download(e) => write!(f, "download failed: {e:?}"),
            #[cfg(feature = "matter-ota")]
            OtaError::Matter(e) => write!(f, "Matter transfer failed: {e:?}"),
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
/// early, right after opening the store; `version` is this firmware's.
pub async fn boot(store: &SharedStore, version: Version) -> BootAction {
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
        pending = Some(Pending { slot, previous: previous(slot, slots), version: triple(version), boots: 0 });
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
            crate::status::notify();
            if store.save(&Pending { boots: boot, ..p }).await.is_err() {
                log::error!("ota: cannot count the trial boot");
            }
        }
        BootAction::RollBack { to } => {
            log::error!("ota: {} was not confirmed after {} boots, back to ota_{to}", p.version(), p.boots);
            if let Err(e) = roll_back(&mut store, to, p.version).await {
                // Keep trying the update rather than loop on a failing switch.
                log::error!("ota: rollback failed: {e}");
                return action;
            }
            drop(store);
            // Nothing to shut down yet: no task runs.
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

/// The slot an update in `slot` came from (of `slots`).
fn previous(slot: u8, slots: usize) -> u8 {
    (slot + slots as u8 - 1) % slots as u8
}

/// Selects slot `to`, as a firmware that works.
fn select_valid(ota: &mut Ota<'_, '_>, to: u8) -> Result<(), OtaError> {
    ota.set_current_app_partition(subtype(to)?).map_err(|_| OtaError::Flash)?;
    ota.set_current_ota_state(OtaImageState::Valid).map_err(|_| OtaError::Flash)
}

/// Selects slot `to` after the update `version` failed, and records it. Restart next.
async fn roll_back(store: &mut EspStore, to: u8, version: (u16, u16, u16)) -> Result<(), OtaError> {
    with(|flash| {
        let layout = Layout::read(flash)?;
        select_valid(&mut layout.ota(flash)?, to)
    })?;
    let _ = store.remove::<Pending>().await;
    let _ = store.save(&LastUpdate { outcome: Outcome::RolledBack, version }).await;
    Ok(())
}

/// An update on trial: not confirmed yet (a deep-sleeping device does not sleep then).
static TRIAL: AtomicBool = AtomicBool::new(false);

/// Whether the running firmware is an update on trial (see [`boot`]).
pub fn on_trial() -> bool {
    TRIAL.load(Ordering::Acquire)
}

/// The update on trial (`version`, this firmware) was not healthy within `timeout_s` (`missing`:
/// the checks not passed): goes back to the previous firmware and restarts. If it cannot, it
/// restarts still on trial: the boot counters ([`boot`], [`early_guard`]) roll back from there.
pub async fn give_up(store: &SharedStore, missing: u32, version: Version, timeout_s: u32) -> ! {
    let mut store = store.lock().await;
    log::error!("ota: {version} not healthy within {timeout_s} s (checks missing: {missing:#x})");
    // Its record is gone (the trial boot could not be saved, say): otadata knows the slots.
    let to = match store.load::<Pending>().await.ok().flatten() {
        Some(p) => Ok(p.previous),
        None => with(Layout::read).and_then(|l| l.booted.map(|b| previous(b, l.slots)).ok_or(OtaError::NoSlots)),
    };
    let rolled_back = match to {
        Ok(to) => {
            log::error!("ota: back to ota_{to}");
            roll_back(&mut store, to, triple(version)).await
        }
        Err(e) => Err(e),
    };
    if let Err(e) = rolled_back {
        log::error!("ota: rollback failed: {e}, restarting on trial");
    }
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
        let to = previous(layout.booted.ok_or(OtaError::NoSlots)?, layout.slots);
        select_valid(&mut ota, to)?;
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
    crate::status::notify();
}
static BUSY: AtomicBool = AtomicBool::new(false);

/// The right to install: one at a time, none while an update is on trial. Take it before the
/// first await of an install, so a sleepy device stays awake while the manifest is checked.
/// Dropped, it frees the slot.
pub struct Claim(());

impl Claim {
    /// Takes the right to install, or `Busy`.
    pub fn take() -> Result<Self, OtaError> {
        if on_trial() || BUSY.swap(true, Ordering::Acquire) {
            return Err(OtaError::Busy);
        }
        crate::status::notify();
        Ok(Self(()))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
        crate::status::notify();
    }
}

/// What an [`Install`] writes, and how to check it.
#[derive(Clone, Copy)]
pub enum Image<'m> {
    /// The image a signed manifest describes. Check the manifest first (`verify`, then
    /// `check_update`).
    Signed(&'m Manifest<'m>),
    /// A bare app image from a Matter OTA file (`matter-ota-unsigned`), for `product`: its
    /// header gives the size, version and SHA-256. Check the header first (`check_update`).
    #[cfg(feature = "matter-ota-unsigned")]
    Bare {
        /// The Matter OTA header.
        header: &'m crate::image::matter::Header<'m>,
        /// The running firmware's product, which the app descriptor must name.
        product: &'static str,
    },
}

impl Image<'_> {
    fn size(&self) -> u32 {
        match self {
            Image::Signed(m) => m.size,
            // Larger than any slot: `Install::begin` refuses it.
            #[cfg(feature = "matter-ota-unsigned")]
            Image::Bare { header, .. } => u32::try_from(header.payload_size).unwrap_or(u32::MAX),
        }
    }

    fn version(&self) -> Version {
        match self {
            Image::Signed(m) => m.version,
            #[cfg(feature = "matter-ota-unsigned")]
            Image::Bare { header, .. } => crate::image::matter::from_version(header.version),
        }
    }

    fn check_head(&self, head: &[u8]) -> Result<(), crate::image::Error> {
        match self {
            Image::Signed(m) => m.check_app(head, CHIP_ID),
            #[cfg(feature = "matter-ota-unsigned")]
            Image::Bare { header, product } => header.check_app(head, CHIP_ID, product),
        }
    }

    fn check(&self, size: u32, sha256: &[u8; 32]) -> Result<(), crate::image::Error> {
        match self {
            Image::Signed(m) => m.check_image(size, sha256),
            #[cfg(feature = "matter-ota-unsigned")]
            Image::Bare { header, .. } if u64::from(size) != header.payload_size => Err(crate::image::Error::Size),
            #[cfg(feature = "matter-ota-unsigned")]
            Image::Bare { header, .. } if sha256 != &header.digest => Err(crate::image::Error::Hash),
            #[cfg(feature = "matter-ota-unsigned")]
            Image::Bare { .. } => Ok(()),
        }
    }
}

/// An update being written into the spare slot. One at a time, and none while an update is on
/// trial (the spare slot holds the firmware to go back to). Dropped before [`commit`](Self::commit),
/// it erases what it wrote: never a bootable partial image.
pub struct Install<'m> {
    layout: Layout,
    previous: u8,
    sink: FlashSink<'m>,
    /// `None` once committed: the selected image keeps the slot until the restart.
    claim: Option<Claim>,
}

impl<'m> Install<'m> {
    /// Starts writing `image` into the spare slot, a sector at a time through `sector`.
    pub fn begin(claim: Claim, image: Image<'m>, sector: &'m mut Sector) -> Result<Self, OtaError> {
        let (layout, previous) = with(Layout::read).and_then(|layout| {
            let previous = layout.booted.ok_or(OtaError::NoSlots)?;
            if image.size() > layout.target_len {
                return Err(OtaError::TooLarge);
            }
            Ok((layout, previous))
        })?;
        log::info!("ota: installing {} ({} KiB) into ota_{}", image.version(), image.size() / 1024, layout.target);
        let sink = FlashSink::new(image, sector, layout.target_offset, layout.target_len);
        Ok(Self { layout, previous, sink, claim: Some(claim) })
    }

    /// Bytes written so far.
    pub fn received(&self) -> u32 {
        self.sink.received
    }

    /// The image's size.
    pub fn size(&self) -> u32 {
        self.sink.size
    }

    /// Checks that `len` more bytes (when known) complete the image.
    pub fn expect(&self, len: Option<u32>) -> Result<(), OtaError> {
        match len {
            Some(len) if self.sink.received + len != self.sink.size => Err(OtaError::Image(crate::image::Error::Size)),
            _ => Ok(()),
        }
    }

    /// Writes the next bytes of the image.
    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), OtaError> {
        self.sink.data(bytes)
    }

    /// Checks the whole image, then selects it for the next boot (on trial). Restart to run it.
    pub async fn commit(mut self, store: &SharedStore) -> Result<Version, OtaError> {
        let (size, sha256) = self.sink.finish();
        self.sink.image.check(size, &sha256).map_err(OtaError::Image)?;
        let version = self.sink.image.version();
        select_update(&self.layout, self.previous, version, store).await?;
        // The selected image waits for the restart: no other install may overwrite it.
        core::mem::forget(self.claim.take());
        Ok(version)
    }

    /// Checks the whole image, then keeps it in the spare slot without selecting it, for
    /// [`apply_staged`] once the provider says so; other installs may run meanwhile. `source`:
    /// the SHA-256 of what was downloaded, to recognise it when offered again.
    #[cfg(esp_stage)]
    pub async fn stage(mut self, store: &SharedStore, source: [u8; 32]) -> Result<Staged, OtaError> {
        let (size, sha256) = self.sink.finish();
        self.sink.image.check(size, &sha256).map_err(OtaError::Image)?;
        let version = triple(self.sink.image.version());
        let staged = Staged { slot: self.layout.target, size, sha256, version, source };
        store.lock().await.save(&staged).await.map_err(|_| OtaError::Flash)?;
        log::info!("ota: {} downloaded, kept to install later", self.sink.image.version());
        // Not erased on drop, and not bootable either: otadata does not select it.
        drop(self.claim.take());
        Ok(staged)
    }
}

/// The image kept by [`Install::stage`], if it may still be applied over `running`.
#[cfg(esp_stage)]
pub async fn staged(store: &SharedStore, running: Version) -> Option<Staged> {
    let mut store = store.lock().await;
    let staged = store.load::<Staged>().await.ok().flatten()?;
    let target = with(Layout::read).ok()?.target;
    if staged.usable(target, running, on_trial()) {
        return Some(staged);
    }
    let _ = store.remove::<Staged>().await;
    None
}

/// Forgets the image kept by [`Install::stage`] (left in the spare slot, unselected).
#[cfg(esp_stage)]
pub async fn forget_staged(store: &SharedStore) {
    let _ = store.lock().await.remove::<Staged>().await;
}

/// Selects the image kept by [`Install::stage`] for the next boot (on trial), once hashed again:
/// another install may have written the slot since. Restart to run it.
#[cfg(esp_stage)]
pub async fn apply_staged(store: &SharedStore, staged: &Staged, sector: &mut Sector) -> Result<Version, OtaError> {
    let claim = Claim::take()?;
    let layout = with(Layout::read)?;
    let previous = layout.booted.ok_or(OtaError::NoSlots)?;
    let mut hasher = Sha256::new();
    let mut at = 0;
    while at < staged.size && staged.size <= layout.target_len && layout.target == staged.slot {
        let n = (staged.size - at).min(SECTOR as u32) as usize;
        with(|flash| flash.read(layout.target_offset + at, &mut sector[..n])).map_err(|_| OtaError::Flash)?;
        hasher.update(&sector[..n]);
        at += n as u32;
        // About a second for a whole slot: let the stack run.
        embassy_futures::yield_now().await;
    }
    if at != staged.size || <[u8; 32]>::from(hasher.finalize()) != staged.sha256 {
        log::warn!("ota: the downloaded {} was overwritten", staged.version());
        let _ = store.lock().await.remove::<Staged>().await;
        return Err(OtaError::Image(crate::image::Error::Hash));
    }
    select_update(&layout, previous, staged.version(), store).await?;
    core::mem::forget(claim);
    Ok(staged.version())
}

/// Selects the image in the target slot for the next boot, on trial.
async fn select_update(layout: &Layout, previous: u8, version: Version, store: &SharedStore) -> Result<(), OtaError> {
    // `Pending` first, then the switch: a power cut in between boots the old slot, which
    // forgets the update (`Abandoned`). The other order could boot the update with no trial
    // count.
    let mut store = store.lock().await;
    let triple = triple(version);
    store
        .save(&Pending { slot: layout.target, previous, version: triple, boots: 0 })
        .await
        .map_err(|_| OtaError::Flash)?;
    let switched = with(|flash| {
        let mut ota = layout.ota(flash)?;
        // Blank OTA data (just flashed over USB) selects no slot: record the running one
        // first, so the switch counts from it.
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
    // A staged image is selected now, or overwritten.
    #[cfg(esp_stage)]
    let _ = store.remove::<Staged>().await;
    let _ = store.save(&LastUpdate { outcome: Outcome::Installed, version: triple }).await;
    log::info!("ota: {version} installed, restart to run it");
    Ok(())
}

impl Drop for Install<'_> {
    fn drop(&mut self) {
        // Erased before the claim (a field) is dropped and frees the slot.
        if self.claim.is_some() && self.sink.written > 0 {
            let at = self.layout.target_offset;
            let _ = with(|flash| flash.erase(at, at + SECTOR as u32));
        }
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

/// A flash sector's worth of image, for [`Install::begin`].
pub type Sector = [u8; SECTOR];

/// Writes the image to the target slot sector by sector, checking its header and hashing it.
struct FlashSink<'m> {
    image: Image<'m>,
    size: u32,
    offset: u32,
    capacity: u32,
    sector: &'m mut Sector,
    fill: usize,
    written: u32,
    received: u32,
    hasher: Sha256,
    header_checked: bool,
}

impl<'m> FlashSink<'m> {
    fn new(image: Image<'m>, sector: &'m mut Sector, offset: u32, capacity: u32) -> Self {
        Self {
            size: image.size(),
            image,
            offset,
            capacity,
            sector,
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
        let before = self.written * 20 / self.size.max(1);
        self.written += self.fill as u32;
        self.fill = 0;
        #[cfg(feature = "ha-update")]
        {
            let step = self.written * 20 / self.size.max(1);
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

    fn data(&mut self, mut bytes: &[u8]) -> Result<(), OtaError> {
        self.received += bytes.len() as u32;
        if self.received > self.capacity || self.received > self.size {
            return Err(OtaError::TooLarge);
        }
        self.hasher.update(bytes);
        while !bytes.is_empty() {
            let n = bytes.len().min(SECTOR - self.fill);
            self.sector[self.fill..self.fill + n].copy_from_slice(&bytes[..n]);
            self.fill += n;
            bytes = &bytes[n..];
            if !self.header_checked && self.written == 0 && self.fill >= app::HEAD_LEN {
                self.image.check_head(&self.sector[..self.fill]).map_err(OtaError::Image)?;
                self.header_checked = true;
            }
            if self.fill == SECTOR {
                self.flush()?;
            }
        }
        // Keep `received` and the flash in step at the end of the image.
        if self.received == self.size {
            if !self.header_checked {
                return Err(OtaError::Image(crate::image::Error::NotAnImage));
            }
            self.flush()?;
        }
        Ok(())
    }
}
