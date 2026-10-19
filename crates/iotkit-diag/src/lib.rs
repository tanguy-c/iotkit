//! Diagnostics: why the device restarted, the last crash, boot counters and a task watchdog.
//!
//! A panic (with the `panic-handler` feature) or a starving task writes a [`Crash`] to RTC memory,
//! which survives the restart that follows. At the next boot `esp::boot` takes it, and
//! [`record_boot`] keeps it in flash as [`LastCrash`] and counts boots and crashes in [`BootStats`].

#![no_std]

#[cfg(feature = "_esp")]
pub mod esp;
mod watch;

use core::fmt;

use embedded_storage_async::nor_flash::MultiwriteNorFlash;
use heapless::{String, Vec};
use iotkit_config::{Setting, Store, keys};
use serde::{Deserialize, Serialize};
pub use watch::{TaskWatch, starving};

/// Longest crash message kept.
pub const MESSAGE_LEN: usize = 160;
/// Backtrace frames kept.
pub const BACKTRACE_LEN: usize = 8;

/// What crashed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrashKind {
    Panic,
    /// A [`TaskWatch`] was not fed in time.
    Watchdog,
}

/// A crash, recorded before the restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crash {
    pub kind: CrashKind,
    /// Panic message and location, or the starving task; truncated to [`MESSAGE_LEN`].
    pub message: String<MESSAGE_LEN>,
    /// Program counters, innermost first. `espflash monitor` turns them into function names.
    pub backtrace: Vec<u32, BACKTRACE_LEN>,
    /// Seconds since boot.
    pub uptime_s: u32,
}

impl Crash {
    pub fn new(kind: CrashKind, uptime_s: u32) -> Self {
        Self { kind, message: String::new(), backtrace: Vec::new(), uptime_s }
    }
}

/// Writes into the message, dropping what does not fit.
impl fmt::Write for Crash {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            if self.message.push(c).is_err() {
                break;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Crash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            CrashKind::Panic => "panic",
            CrashKind::Watchdog => "watchdog",
        };
        write!(f, "{kind} after {} s: {}", self.uptime_s, self.message)?;
        for pc in &self.backtrace {
            write!(f, " 0x{pc:08x}")?;
        }
        Ok(())
    }
}

/// Why the chip restarted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetCause {
    PowerOn,
    /// `software_reset()`: a requested reboot, or the restart after a recorded crash.
    Software,
    /// A hardware watchdog fired: something blocked the executor.
    Watchdog,
    BrownOut,
    DeepSleep,
    /// Reset over USB or JTAG (e.g. by espflash).
    Debugger,
    Other(u8),
}

/// Kept in flash: the last crash, until the next one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastCrash(pub Crash);

impl Setting for LastCrash {
    const KEY: u16 = keys::LAST_CRASH;
    const VERSION: u8 = 1;
}

/// Kept in flash: boot and crash counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootStats {
    pub boots: u32,
    /// Panics and watchdog resets, hardware ones included.
    pub crashes: u32,
    /// Crashes in a row since the last boot that reached [`BootStats::mark_stable`]. Lets the
    /// firmware stop a boot loop (e.g. skip a feature, or wait for OTA).
    pub crashes_in_a_row: u32,
}

impl Setting for BootStats {
    const KEY: u16 = keys::BOOT_STATS;
    const VERSION: u8 = 1;
}

/// What is known about this boot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boot {
    pub cause: ResetCause,
    /// The crash that caused this restart, if one was recorded.
    pub crash: Option<Crash>,
}

impl Boot {
    /// A crash restarted the device: recorded, or a hardware watchdog.
    pub fn crashed(&self) -> bool {
        self.crash.is_some() || self.cause == ResetCause::Watchdog
    }
}

/// Counts this boot in flash and keeps its crash; returns the counters and the last crash.
pub async fn record_boot<F: MultiwriteNorFlash>(
    store: &mut Store<F>,
    boot: &Boot,
) -> Result<(BootStats, Option<Crash>), iotkit_config::Error> {
    let mut stats = store.load::<BootStats>().await?.unwrap_or_default();
    stats.boots = stats.boots.wrapping_add(1);
    if boot.crashed() {
        stats.crashes = stats.crashes.wrapping_add(1);
        stats.crashes_in_a_row = stats.crashes_in_a_row.saturating_add(1);
    }
    store.save(&stats).await?;
    if let Some(crash) = &boot.crash {
        store.save(&LastCrash(crash.clone())).await?;
    }
    let last = store.load::<LastCrash>().await?.map(|c| c.0);
    Ok((stats, last))
}

impl BootStats {
    /// Call once the firmware runs well (e.g. after the health check): resets `crashes_in_a_row`.
    pub async fn mark_stable<F: MultiwriteNorFlash>(store: &mut Store<F>) -> Result<(), iotkit_config::Error> {
        let mut stats = store.load::<BootStats>().await?.unwrap_or_default();
        if stats.crashes_in_a_row != 0 {
            stats.crashes_in_a_row = 0;
            store.save(&stats).await?;
        }
        Ok(())
    }
}

/// The crash record in RTC memory: 64 words that survive a software or watchdog reset.
///
/// Layout: magic, checksum of the rest, uptime, `kind | frames << 8 | message length << 16`,
/// [`BACKTRACE_LEN`] frames, then the message bytes.
pub mod rtc {
    use super::*;

    pub const WORDS: usize = 64;
    const MAGIC: u32 = 0x10c4_c4a5;
    const HEADER: usize = 4;
    const TEXT: usize = HEADER + BACKTRACE_LEN;
    const _: () = assert!((WORDS - TEXT) * 4 >= MESSAGE_LEN);

    pub type Record = [u32; WORDS];

    fn checksum(words: &[u32]) -> u32 {
        // FNV-1a over the words: enough to tell a record from leftover RAM.
        words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .fold(0x811c_9dc5, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
    }

    /// Writes `crash` into `record`.
    pub fn store(record: &mut Record, crash: &Crash) {
        let kind = match crash.kind {
            CrashKind::Panic => 0,
            CrashKind::Watchdog => 1,
        };
        record[2] = crash.uptime_s;
        record[3] = kind | (crash.backtrace.len() as u32) << 8 | (crash.message.len() as u32) << 16;
        for (i, slot) in record[HEADER..TEXT].iter_mut().enumerate() {
            *slot = crash.backtrace.get(i).copied().unwrap_or(0);
        }
        let mut text = [0u8; (WORDS - TEXT) * 4];
        text[..crash.message.len()].copy_from_slice(crash.message.as_bytes());
        for (slot, chunk) in record[TEXT..].iter_mut().zip(text.as_chunks::<4>().0) {
            *slot = u32::from_le_bytes(*chunk);
        }
        record[1] = checksum(&record[2..]);
        record[0] = MAGIC;
    }

    /// Reads and clears the record; `None` if it holds no valid crash.
    pub fn take(record: &mut Record) -> Option<Crash> {
        let crash = read(record);
        record[0] = 0;
        crash
    }

    fn read(record: &Record) -> Option<Crash> {
        if record[0] != MAGIC || record[1] != checksum(&record[2..]) {
            return None;
        }
        let kind = match record[3] & 0xff {
            0 => CrashKind::Panic,
            1 => CrashKind::Watchdog,
            _ => return None,
        };
        let frames = (record[3] >> 8 & 0xff) as usize;
        let len = (record[3] >> 16) as usize;
        if frames > BACKTRACE_LEN || len > MESSAGE_LEN {
            return None;
        }
        let mut crash = Crash::new(kind, record[2]);
        crash.backtrace.extend_from_slice(&record[HEADER..HEADER + frames]).ok()?;
        let mut text = [0u8; (WORDS - TEXT) * 4];
        for (chunk, word) in text.as_chunks_mut::<4>().0.iter_mut().zip(&record[TEXT..]) {
            *chunk = word.to_le_bytes();
        }
        crash.message.push_str(core::str::from_utf8(&text[..len]).ok()?).ok()?;
        Some(crash)
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use super::*;

    fn crash() -> Crash {
        let mut c = Crash::new(CrashKind::Panic, 42);
        write!(c, "panicked at src/main.rs:10:5: boom").unwrap();
        c.backtrace.extend_from_slice(&[0x4200_1234, 0x4200_5678]).unwrap();
        c
    }

    #[test]
    fn rtc_round_trip() {
        let mut record = [0xdead_beef; rtc::WORDS];
        assert_eq!(rtc::take(&mut record), None);
        rtc::store(&mut record, &crash());
        assert_eq!(rtc::take(&mut record), Some(crash()));
        // Taken once only.
        assert_eq!(rtc::take(&mut record), None);
    }

    #[test]
    fn rtc_rejects_damage() {
        let mut record = [0; rtc::WORDS];
        rtc::store(&mut record, &crash());
        record[20] ^= 1;
        assert_eq!(rtc::take(&mut record), None);
    }

    #[test]
    fn rtc_full_message_and_backtrace() {
        let mut c = Crash::new(CrashKind::Watchdog, u32::MAX);
        for _ in 0..50 {
            write!(c, "é-long-").unwrap(); // truncated at a char boundary
        }
        assert!(c.message.len() <= MESSAGE_LEN);
        while c.backtrace.push(0x4200_0000).is_ok() {}
        let mut record = [0; rtc::WORDS];
        rtc::store(&mut record, &c);
        assert_eq!(rtc::take(&mut record), Some(c));
    }

    #[test]
    fn settings_fit_and_round_trip() {
        let mut c = Crash::new(CrashKind::Panic, 1);
        while c.message.push('x').is_ok() {}
        while c.backtrace.push(u32::MAX).is_ok() {}
        let mut buf = [0; iotkit_config::MAX_VALUE_LEN - 1];
        let bytes = postcard::to_slice(&LastCrash(c.clone()), &mut buf).unwrap();
        assert_eq!(postcard::from_bytes::<LastCrash>(bytes).unwrap(), LastCrash(c));
    }

    #[test]
    fn display() {
        let s = {
            let mut s = String::<256>::new();
            write!(s, "{}", crash()).unwrap();
            s
        };
        assert_eq!(s, "panic after 42 s: panicked at src/main.rs:10:5: boom 0x42001234 0x42005678");
    }
}
