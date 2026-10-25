//! Wall-clock time from Home Assistant (`GetTimeResponse` over the ESPHome API), kept across deep
//! sleep by the RTC timer, in Home Assistant's time zone. Unknown after a power loss until Home
//! Assistant sends it again (at each connection).

use esp_hal::rtc_cntl::Rtc;

use crate::time::Zone;

/// The RTC timer counts on through deep sleep. Borrowed only to read and set the time, as
/// esp-hal's own sleep code does.
fn rtc() -> Rtc<'static> {
    // SAFETY: reads, and sets its boot-time registers, which nothing else in iotkit uses.
    Rtc::new(unsafe { esp_hal::peripherals::RTC_TIMER::steal() })
}

const MAGIC: u32 = 0x7130_0001;

/// The time zone, after a magic word: the RTC holds the time once it is valid.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut ZONE: [u32; 1 + Zone::WORDS] = [0; 1 + Zone::WORDS];

fn with<R>(f: impl FnOnce(&mut [u32; 1 + Zone::WORDS]) -> R) -> R {
    let zone: *mut [u32; 1 + Zone::WORDS] = &raw mut ZONE;
    // SAFETY: core 0 only, from tasks (never an interrupt), never across an await.
    f(unsafe { &mut *zone })
}

/// Sets the time (`epoch`: Unix seconds) and its time zone.
pub(crate) fn set(epoch: u32, zone: Zone) {
    let first = now().is_none();
    rtc().set_current_time_us(u64::from(epoch) * 1_000_000);
    with(|w| {
        w[1..].copy_from_slice(&zone.to_words());
        w[0] = MAGIC;
    });
    if first {
        let offset = zone.offset(i64::from(epoch));
        let local = zone.local(i64::from(epoch)).rem_euclid(86_400);
        log::info!(
            "clock: {:02}:{:02} local (UTC{:+}:{:02})",
            local / 3600,
            local % 3600 / 60,
            offset / 3600,
            offset.abs() % 3600 / 60
        );
    }
}

/// Unix seconds now, if Home Assistant gave the time since the last power loss.
pub fn now() -> Option<i64> {
    with(|w| w[0] == MAGIC).then(|| (rtc().current_time_us() / 1_000_000) as i64)
}

/// Home Assistant's time zone (UTC while unknown).
pub fn zone() -> Zone {
    with(|w| if w[0] == MAGIC { Zone::from_words(w[1..].try_into().unwrap_or(&[0; Zone::WORDS])) } else { Zone::UTC })
}
