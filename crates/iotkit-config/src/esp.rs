//! The chip's flash, shared by every iotkit crate, and the `config` partition.
//!
//! Only one `FlashStorage` can exist (esp-storage reads the flash size once), so [`init`] makes
//! it and [`SharedFlash`] hands it out. Each flash operation blocks the executor for its
//! duration (a sector erase is about 50 ms). Interrupts stay on between sectors: esp-storage
//! masks them only for each ROM call, as it must while the cache is off.

use core::{
    cell::UnsafeCell,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_embedded_hal::adapter::BlockingAsync;
use embedded_storage::nor_flash::{ErrorType, MultiwriteNorFlash, NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{PARTITION_TABLE_MAX_LEN, read_partition_table};
use esp_hal::peripherals::FLASH;
use esp_storage::{FlashStorage, FlashStorageError};

use crate::{Error, Store};

/// Label of the settings partition in `partitions.csv`.
pub const CONFIG_LABEL: &str = "config";

/// The flash, and whether someone is using it. Not a critical section: a multi-sector erase
/// would keep interrupts (the radio's among them) off for its whole length.
struct Slot {
    storage: UnsafeCell<Option<FlashStorage<'static>>>,
    busy: AtomicBool,
}

// SAFETY: `busy` gives one user at a time. The flash is only used from tasks (never from an
// interrupt handler), on the single core.
unsafe impl Sync for Slot {}

static FLASH_STORAGE: Slot = Slot { storage: UnsafeCell::new(None), busy: AtomicBool::new(false) };

/// Sets up the shared flash. Call once, before anything else uses the flash.
pub fn init(flash: FLASH<'static>) {
    let storage = FlashStorage::new(flash);
    with_slot(|slot| {
        assert!(slot.is_none(), "iotkit_config::esp::init called twice");
        slot.replace(storage);
    });
}

/// Runs `f` on the flash. Panics before [`init`], or if `f` uses the flash itself.
pub fn with<R>(f: impl FnOnce(&mut FlashStorage<'static>) -> R) -> R {
    with_slot(|slot| f(slot.as_mut().expect("iotkit_config::esp::init not called")))
}

fn with_slot<R>(f: impl FnOnce(&mut Option<FlashStorage<'static>>) -> R) -> R {
    /// Frees the slot however `f` ends.
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            FLASH_STORAGE.busy.store(false, Ordering::Release);
        }
    }
    assert!(!FLASH_STORAGE.busy.swap(true, Ordering::Acquire), "iotkit_config::esp::with re-entered");
    let _release = Release;
    // SAFETY: `busy` was false, so this is the only reference until `Release` runs.
    f(unsafe { &mut *FLASH_STORAGE.storage.get() })
}

/// Handle to the shared flash (blocking `embedded-storage`). Copy it freely.
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedFlash;

impl ErrorType for SharedFlash {
    type Error = FlashStorageError;
}

impl ReadNorFlash for SharedFlash {
    const READ_SIZE: usize = FlashStorage::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        with(|flash| flash.read(offset, bytes))
    }

    fn capacity(&self) -> usize {
        with(|flash| flash.capacity())
    }
}

impl NorFlash for SharedFlash {
    const WRITE_SIZE: usize = FlashStorage::WRITE_SIZE;
    const ERASE_SIZE: usize = FlashStorage::ERASE_SIZE;

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        with(|flash| flash.write(offset, bytes))
    }

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        with(|flash| flash.erase(from, to))
    }
}

impl MultiwriteNorFlash for SharedFlash {}

/// Absolute flash range of the partition labelled `label`.
pub fn find_partition(label: &str) -> Option<Range<u32>> {
    let mut buf = [0; PARTITION_TABLE_MAX_LEN];
    with(|flash| {
        let table = read_partition_table(flash, &mut buf).ok()?;
        let entry = table.iter().find(|p| p.label_as_str() == label)?;
        Some(entry.offset()..entry.offset() + entry.len())
    })
}

/// The settings store on the chip.
pub type EspStore = Store<BlockingAsync<SharedFlash>>;

/// The store shared by the tasks that need it.
pub type SharedStore = embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, EspStore>;

/// Label of the partition where other stacks keep their state (Matter pairings).
pub const NVS_LABEL: &str = "nvs";

/// Forgets everything: every setting, and the [`NVS_LABEL`] partition (Matter pairings). Restart
/// afterwards.
pub async fn factory_reset(store: &SharedStore) -> Result<(), Error> {
    store.lock().await.factory_reset().await?;
    if let Some(nvs) = find_partition(NVS_LABEL) {
        with(|flash| flash.erase(nvs.start, nvs.end)).map_err(|_| Error::Flash)?;
    }
    Ok(())
}

/// Opens the store on the [`CONFIG_LABEL`] partition, erasing it first if it holds something
/// else. Call [`init`] first.
pub async fn open() -> Result<EspStore, Error> {
    let range = find_partition(CONFIG_LABEL).ok_or(Error::NoPartition)?;
    log::info!("config: partition at {:#x}, {} KiB", range.start, (range.end - range.start) / 1024);
    let mut store = Store::new(BlockingAsync::new(SharedFlash), range)?;
    store.format_if_needed().await?;
    Ok(store)
}
