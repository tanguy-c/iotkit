//! Typed, versioned settings in flash.
//!
//! Each setting is a serde type with a unique [`Setting::KEY`] and a [`Setting::VERSION`]. It is
//! stored as its version byte followed by its postcard encoding, in a
//! [`sequential-storage`](sequential_storage) map: writes are wear-levelled and survive power loss.
//! When the stored version differs from the type's, [`Setting::migrate`] converts it.
//!
//! ```
//! # use embedded_storage_async::nor_flash::MultiwriteNorFlash;
//! use iotkit_core::config::{Setting, Store};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct Boots(u32);
//! impl Setting for Boots { const KEY: u16 = 0x1000; const VERSION: u8 = 1; }
//!
//! # async fn count<F: MultiwriteNorFlash>(store: &mut Store<F>) -> Result<(), iotkit_core::config::Error> {
//! let boots = store.load::<Boots>().await?.unwrap_or(Boots(0));
//! store.save(&Boots(boots.0 + 1)).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Keys below [`APP_KEYS`] are reserved for the iotkit crates.

use core::{fmt, ops::Range};

use embedded_storage_async::nor_flash::MultiwriteNorFlash;
use sequential_storage::{cache::NoCache, map};
use serde::{Serialize, de::DeserializeOwned};

/// First key free for applications; the ones below belong to iotkit.
pub const APP_KEYS: u16 = 0x1000;

/// Keys used by the iotkit crates.
pub mod keys {
    /// `iotkit::diag`: the last crash.
    pub const LAST_CRASH: u16 = 0x0010;
    /// `iotkit::diag`: boot and crash counters.
    pub const BOOT_STATS: u16 = 0x0011;
    /// `iotkit::net`: the Wi-Fi network.
    pub const WIFI: u16 = 0x0020;
    /// `iotkit::ota`: an installed image waiting to be confirmed.
    pub const OTA_PENDING: u16 = 0x0030;
    /// `iotkit::ota`: what the last update did.
    pub const OTA_LAST: u16 = 0x0031;
    /// `iotkit::ota`: the manifest URL to check for updates.
    pub const UPDATE_SOURCE: u16 = 0x0032;
    /// `iotkit::mqtt`: the broker.
    pub const MQTT_BROKER: u16 = 0x0040;
    /// `iotkit::matter`: the setup code (passcode and discriminator).
    pub const MATTER_SETUP: u16 = 0x0050;
}

/// Largest encoded setting, version byte included.
pub const MAX_VALUE_LEN: usize = 256;

/// Item buffer: key, value and sequential-storage's header, rounded up.
const BUF_LEN: usize = MAX_VALUE_LEN + 32;

/// A value kept in flash.
pub trait Setting: Serialize + DeserializeOwned {
    /// Unique per setting type. Applications use [`APP_KEYS`] and above.
    const KEY: u16;
    /// Bump when the encoding changes, and handle the old one in [`migrate`](Self::migrate).
    const VERSION: u8;

    /// Converts a value stored as version `from` (its postcard bytes). `None` drops it, and
    /// [`Store::load`] returns `None` as if nothing were stored.
    fn migrate(from: u8, bytes: &[u8]) -> Option<Self> {
        let _ = (from, bytes);
        None
    }
}

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The flash driver failed.
    Flash,
    /// The flash range holds something sequential-storage cannot repair.
    Corrupted,
    /// No space left, even after garbage collection.
    Full,
    /// The setting does not fit in [`MAX_VALUE_LEN`].
    TooLarge,
    /// The stored bytes do not decode as the setting.
    Decode,
    /// The range is not sector-aligned, or shorter than two sectors.
    Range,
    /// No `config` partition (see `esp::open`).
    NoPartition,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::Flash => "flash error",
            Error::Corrupted => "config storage corrupted",
            Error::Full => "config storage full",
            Error::TooLarge => "setting too large",
            Error::Decode => "stored setting does not decode",
            Error::Range => "config range not sector-aligned or too small",
            Error::NoPartition => "no `config` partition",
        })
    }
}

impl core::error::Error for Error {}

impl<E> From<sequential_storage::Error<E>> for Error {
    fn from(e: sequential_storage::Error<E>) -> Self {
        use sequential_storage::Error as S;
        match e {
            S::Storage { .. } => Error::Flash,
            S::FullStorage => Error::Full,
            S::Corrupted { .. } => Error::Corrupted,
            S::BufferTooBig | S::BufferTooSmall(_) | S::ItemTooBig | S::SerializationError(_) => Error::TooLarge,
            _ => Error::Corrupted,
        }
    }
}

/// Settings in a flash range.
pub struct Store<F> {
    flash: F,
    range: Range<u32>,
    buf: [u8; BUF_LEN],
}

impl<F: MultiwriteNorFlash> Store<F> {
    /// Uses `range` of `flash`, which must be sector-aligned and at least two sectors long.
    pub fn new(flash: F, range: Range<u32>) -> Result<Self, Error> {
        let sector = F::ERASE_SIZE as u32;
        let aligned = range.start.is_multiple_of(sector) && range.end.is_multiple_of(sector);
        if !aligned || range.end < range.start + 2 * sector {
            return Err(Error::Range);
        }
        Ok(Self { flash, range, buf: [0; BUF_LEN] })
    }

    /// Checks that the range holds a store; if it holds something else (e.g. an old firmware, or
    /// it was never erased), erases it. Returns `true` when it did. Call once after [`new`](Self::new).
    pub async fn format_if_needed(&mut self) -> Result<bool, Error> {
        let probe: Result<Option<&[u8]>, _> =
            map::fetch_item(&mut self.flash, self.range.clone(), &mut NoCache::new(), &mut self.buf, &0u16).await;
        match probe.map_err(Error::from) {
            Ok(_) => Ok(false),
            Err(Error::Corrupted) => {
                log::warn!(
                    "config: {:#x}..{:#x} is not a settings store, erasing it",
                    self.range.start,
                    self.range.end
                );
                self.factory_reset().await?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    /// The flash range in use.
    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    /// Reads a setting; `None` if it was never saved (or its old version was dropped).
    pub async fn load<S: Setting>(&mut self) -> Result<Option<S>, Error> {
        let raw: Option<&[u8]> =
            map::fetch_item(&mut self.flash, self.range.clone(), &mut NoCache::new(), &mut self.buf, &S::KEY).await?;
        let Some((&version, bytes)) = raw.and_then(<[u8]>::split_first) else {
            return Ok(None);
        };
        if version == S::VERSION {
            postcard::from_bytes(bytes).map(Some).map_err(|_| Error::Decode)
        } else {
            let migrated = S::migrate(version, bytes);
            if migrated.is_none() {
                log::warn!("config: dropped setting {:#06x} v{version} (now v{})", S::KEY, S::VERSION);
            }
            Ok(migrated)
        }
    }

    /// Writes a setting (only when it differs from what is stored, to spare the flash).
    pub async fn save<S: Setting>(&mut self, value: &S) -> Result<(), Error> {
        let mut encoded = [0; MAX_VALUE_LEN];
        encoded[0] = S::VERSION;
        let n = postcard::to_slice(value, &mut encoded[1..]).map_err(|_| Error::TooLarge)?.len() + 1;
        let encoded = &encoded[..n];

        let stored: Option<&[u8]> =
            map::fetch_item(&mut self.flash, self.range.clone(), &mut NoCache::new(), &mut self.buf, &S::KEY).await?;
        if stored == Some(encoded) {
            return Ok(());
        }
        map::store_item(&mut self.flash, self.range.clone(), &mut NoCache::new(), &mut self.buf, &S::KEY, &encoded)
            .await?;
        Ok(())
    }

    /// Forgets a setting.
    pub async fn remove<S: Setting>(&mut self) -> Result<(), Error> {
        map::remove_item(&mut self.flash, self.range.clone(), &mut NoCache::new(), &mut self.buf, &S::KEY).await?;
        Ok(())
    }

    /// Erases every setting.
    pub async fn factory_reset(&mut self) -> Result<(), Error> {
        self.flash.erase(self.range.start, self.range.end).await.map_err(|_| Error::Flash)
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embedded_storage_async::nor_flash::{ErrorType, NorFlash, NorFlashErrorKind, ReadNorFlash};
    use serde::Deserialize;

    use super::*;

    const SECTOR: usize = 4096;

    /// RAM flash with NOR semantics: erase sets 0xff, writes can only clear bits.
    struct RamFlash([u8; 4 * SECTOR]);

    impl ErrorType for RamFlash {
        type Error = NorFlashErrorKind;
    }

    impl ReadNorFlash for RamFlash {
        const READ_SIZE: usize = 4;
        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let o = offset as usize;
            bytes.copy_from_slice(self.0.get(o..o + bytes.len()).ok_or(NorFlashErrorKind::OutOfBounds)?);
            Ok(())
        }
        fn capacity(&self) -> usize {
            self.0.len()
        }
    }

    impl NorFlash for RamFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = SECTOR;
        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            self.0.get_mut(from as usize..to as usize).ok_or(NorFlashErrorKind::OutOfBounds)?.fill(0xff);
            Ok(())
        }
        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let o = offset as usize;
            let dst = self.0.get_mut(o..o + bytes.len()).ok_or(NorFlashErrorKind::OutOfBounds)?;
            dst.iter_mut().zip(bytes).for_each(|(d, b)| *d &= b);
            Ok(())
        }
    }

    impl MultiwriteNorFlash for RamFlash {}

    fn store() -> Store<RamFlash> {
        Store::new(RamFlash([0xff; 4 * SECTOR]), 0..(4 * SECTOR) as u32).unwrap()
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Wifi {
        ssid: heapless_str::Str,
        channel: u8,
    }

    /// Short fixed string, so the tests need no allocator.
    mod heapless_str {
        use serde::{Deserialize, Deserializer, Serialize, Serializer};
        #[derive(Debug, PartialEq)]
        pub struct Str(pub [u8; 8], pub usize);
        impl Str {
            pub fn new(s: &str) -> Self {
                let mut b = [0; 8];
                b[..s.len()].copy_from_slice(s.as_bytes());
                Str(b, s.len())
            }
        }
        impl Serialize for Str {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(core::str::from_utf8(&self.0[..self.1]).unwrap())
            }
        }
        impl<'de> Deserialize<'de> for Str {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <&str>::deserialize(d)?;
                Ok(Str::new(s))
            }
        }
    }

    impl Setting for Wifi {
        const KEY: u16 = APP_KEYS;
        const VERSION: u8 = 2;
        /// v1 had only the channel.
        fn migrate(from: u8, bytes: &[u8]) -> Option<Self> {
            if from != 1 {
                return None;
            }
            Some(Wifi { ssid: heapless_str::Str::new("?"), channel: postcard::from_bytes(bytes).ok()? })
        }
    }

    /// The v1 encoding of `Wifi`, to test migration.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct WifiV1(u8);
    impl Setting for WifiV1 {
        const KEY: u16 = APP_KEYS;
        const VERSION: u8 = 1;
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Boots(u32);
    impl Setting for Boots {
        const KEY: u16 = APP_KEYS + 1;
        const VERSION: u8 = 1;
    }

    #[test]
    fn save_load_remove() {
        block_on(async {
            let mut s = store();
            assert_eq!(s.load::<Boots>().await, Ok(None));
            s.save(&Boots(3)).await.unwrap();
            let wifi = Wifi { ssid: heapless_str::Str::new("home"), channel: 6 };
            s.save(&wifi).await.unwrap();
            assert_eq!(s.load::<Boots>().await, Ok(Some(Boots(3))));
            assert_eq!(s.load::<Wifi>().await, Ok(Some(wifi)));
            s.remove::<Boots>().await.unwrap();
            assert_eq!(s.load::<Boots>().await, Ok(None));
        });
    }

    #[test]
    fn survives_many_writes() {
        // Far more writes than fit in the range: garbage collection must keep only the latest.
        block_on(async {
            let mut s = store();
            for i in 0..5000 {
                s.save(&Boots(i)).await.unwrap();
            }
            assert_eq!(s.load::<Boots>().await, Ok(Some(Boots(4999))));
        });
    }

    #[test]
    fn reopening_keeps_values() {
        block_on(async {
            let mut s = store();
            s.save(&Boots(42)).await.unwrap();
            let flash = s.flash;
            let mut s = Store::new(flash, 0..(4 * SECTOR) as u32).unwrap();
            assert_eq!(s.load::<Boots>().await, Ok(Some(Boots(42))));
        });
    }

    #[test]
    fn migrates_old_versions() {
        block_on(async {
            let mut s = store();
            s.save(&WifiV1(11)).await.unwrap();
            assert_eq!(s.load::<Wifi>().await, Ok(Some(Wifi { ssid: heapless_str::Str::new("?"), channel: 11 })));
            // A version without a migration is dropped: v1 code reading what v2 wrote.
            s.save(&Wifi { ssid: heapless_str::Str::new("new"), channel: 1 }).await.unwrap();
            assert_eq!(s.load::<WifiV1>().await, Ok(None));
        });
    }

    #[test]
    fn factory_reset_erases_everything() {
        block_on(async {
            let mut s = store();
            s.save(&Boots(1)).await.unwrap();
            s.factory_reset().await.unwrap();
            assert_eq!(s.load::<Boots>().await, Ok(None));
            s.save(&Boots(2)).await.unwrap();
            assert_eq!(s.load::<Boots>().await, Ok(Some(Boots(2))));
        });
    }

    #[test]
    fn formats_foreign_data_once() {
        block_on(async {
            // Leftover firmware: arbitrary bytes, not a store.
            let mut flash = RamFlash([0xff; 4 * SECTOR]);
            flash.0.iter_mut().enumerate().for_each(|(i, b)| *b = (i * 7 + 3) as u8);
            let mut s = Store::new(flash, 0..(4 * SECTOR) as u32).unwrap();
            assert_eq!(s.load::<Boots>().await, Err(Error::Corrupted));
            assert_eq!(s.format_if_needed().await, Ok(true));
            s.save(&Boots(1)).await.unwrap();
            assert_eq!(s.format_if_needed().await, Ok(false));
            assert_eq!(s.load::<Boots>().await, Ok(Some(Boots(1))));
            // Erased flash is a valid, empty store.
            assert_eq!(store().format_if_needed().await, Ok(false));
        });
    }

    #[test]
    fn rejects_bad_ranges() {
        let flash = || RamFlash([0xff; 4 * SECTOR]);
        assert!(matches!(Store::new(flash(), 0..SECTOR as u32), Err(Error::Range)));
        assert!(matches!(Store::new(flash(), 100..(3 * SECTOR) as u32), Err(Error::Range)));
    }

    #[test]
    fn rejects_oversized_settings() {
        #[derive(Serialize, Deserialize)]
        struct Big(#[serde(with = "big")] [u8; MAX_VALUE_LEN]);
        mod big {
            use serde::{Deserializer, Serializer};
            pub fn serialize<S: Serializer>(v: &[u8; super::MAX_VALUE_LEN], s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bytes(v)
            }
            pub fn deserialize<'de, D: Deserializer<'de>>(_: D) -> Result<[u8; super::MAX_VALUE_LEN], D::Error> {
                unreachable!()
            }
        }
        impl Setting for Big {
            const KEY: u16 = APP_KEYS + 2;
            const VERSION: u8 = 1;
        }
        block_on(async {
            assert_eq!(store().save(&Big([0; MAX_VALUE_LEN])).await, Err(Error::TooLarge));
        });
    }
}
