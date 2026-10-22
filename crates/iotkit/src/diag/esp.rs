//! The chip side: reset reason, the RTC crash record, the panic handler and the watchdog task.

use core::fmt::Write;

use embassy_time::{Duration, Instant, Ticker};
use esp_hal::{
    peripherals::TIMG1,
    ram,
    rtc_cntl::SocResetReason,
    system::{reset_reason, software_reset},
    timer::timg::{MwdtStage, Wdt},
};

use crate::diag::{Boot, Crash, CrashKind, ResetCause, TaskWatch, rtc, starving};

/// Hardware watchdog timeout: fires when the executor is blocked this long.
const HW_TIMEOUT: esp_hal::time::Duration = esp_hal::time::Duration::from_secs(5);

/// Survives software and watchdog resets (not power loss).
#[ram(unstable(rtc_fast, persistent))]
static mut RECORD: rtc::Record = [0; rtc::WORDS];

/// The record, without a reference to the `static mut`.
fn record() -> *mut rtc::Record {
    &raw mut RECORD
}

fn uptime_s() -> u32 {
    esp_hal::time::Instant::now().duration_since_epoch().as_secs() as u32
}

/// Why the chip restarted, and the crash recorded before, if any (taken: the next boot will not
/// see it again). Call once at boot.
pub fn boot() -> Boot {
    // SAFETY: only core 0 runs (iotkit never starts a second core), and nothing else touches
    // the record outside a crash.
    let crash = unsafe { rtc::take(&mut *record()) };
    Boot { cause: reset_reason().map_or(ResetCause::Other(0), cause), crash }
}

// The reset reasons differ by chip: one mapping per family.
#[cfg(any(feature = "esp32c6", feature = "esp32c5"))]
fn cause(reason: SocResetReason) -> ResetCause {
    use SocResetReason as R;
    match reason {
        R::ChipPowerOn => ResetCause::PowerOn,
        R::CoreSw | R::Cpu0Sw => ResetCause::Software,
        R::CoreMwdt0
        | R::CoreMwdt1
        | R::CoreRtcWdt
        | R::Cpu0Mwdt0
        | R::Cpu0Mwdt1
        | R::Cpu0RtcWdt
        | R::SysRtcWdt
        | R::SysSuperWdt => ResetCause::Watchdog,
        R::SysBrownOut => ResetCause::BrownOut,
        R::CoreDeepSleep => ResetCause::DeepSleep,
        R::CoreUsbUart | R::CoreUsbJtag | R::Cpu0JtagCpu => ResetCause::Debugger,
        other => ResetCause::Other(other as u8),
    }
}

/// No `Cpu0JtagCpu` on the ESP32-C3 and H2.
#[cfg(any(feature = "esp32c3", feature = "esp32h2"))]
fn cause(reason: SocResetReason) -> ResetCause {
    use SocResetReason as R;
    match reason {
        R::ChipPowerOn => ResetCause::PowerOn,
        R::CoreSw | R::Cpu0Sw => ResetCause::Software,
        R::CoreMwdt0
        | R::CoreMwdt1
        | R::CoreRtcWdt
        | R::Cpu0Mwdt0
        | R::Cpu0Mwdt1
        | R::Cpu0RtcWdt
        | R::SysRtcWdt
        | R::SysSuperWdt => ResetCause::Watchdog,
        R::SysBrownOut => ResetCause::BrownOut,
        R::CoreDeepSleep => ResetCause::DeepSleep,
        R::CoreUsbUart | R::CoreUsbJtag => ResetCause::Debugger,
        other => ResetCause::Other(other as u8),
    }
}

/// The ESP32-S3 names its CPU resets `Cpu*` (two cores), and has no `Cpu0JtagCpu`.
#[cfg(feature = "esp32s3")]
fn cause(reason: SocResetReason) -> ResetCause {
    use SocResetReason as R;
    match reason {
        R::ChipPowerOn => ResetCause::PowerOn,
        R::CoreSw | R::CpuSw => ResetCause::Software,
        R::CoreMwdt0
        | R::CoreMwdt1
        | R::CoreRtcWdt
        | R::CpuMwdt0
        | R::CpuMwdt1
        | R::CpuRtcWdt
        | R::SysRtcWdt
        | R::SysSuperWdt => ResetCause::Watchdog,
        R::SysBrownOut => ResetCause::BrownOut,
        R::CoreDeepSleep => ResetCause::DeepSleep,
        R::CoreUsbUart | R::CoreUsbJtag => ResetCause::Debugger,
        other => ResetCause::Other(other as u8),
    }
}

/// The original ESP32: two cores, no USB.
#[cfg(feature = "esp32")]
fn cause(reason: SocResetReason) -> ResetCause {
    use SocResetReason as R;
    match reason {
        R::ChipPowerOn => ResetCause::PowerOn,
        R::CoreSw | R::Cpu0Sw => ResetCause::Software,
        R::CoreMwdt0 | R::CoreMwdt1 | R::CoreRtcWdt | R::CpuMwdt0 | R::Cpu0RtcWdt | R::SysRtcWdt => {
            ResetCause::Watchdog
        }
        R::SysBrownOut => ResetCause::BrownOut,
        R::CoreDeepSleep => ResetCause::DeepSleep,
        other => ResetCause::Other(other as u8),
    }
}

/// A planned restart: lets the links say goodbye ([`crate::runtime::shutdown`], at most 2 s),
/// then resets.
pub async fn restart() -> ! {
    let _ = embassy_time::with_timeout(Duration::from_secs(2), crate::runtime::shutdown::shutdown()).await;
    software_reset()
}

/// Records `crash` for the next boot (unless an earlier one is still there) and restarts.
pub fn restart_with(crash: &Crash) -> ! {
    // SAFETY: see `boot`; we do not return.
    unsafe { rtc::store(&mut *record(), crash) };
    software_reset()
}

/// Feeds the hardware watchdog (TIMG1) every second while every [`TaskWatch`] in `watches` is fed
/// in time. A starving task restarts the device with a [`CrashKind::Watchdog`] record; a blocked
/// executor lets the hardware watchdog reset it after 5 s.
#[embassy_executor::task]
pub async fn watchdog_task(mut wdt: Wdt<TIMG1<'static>>, watches: &'static [&'static TaskWatch]) {
    wdt.set_timeout(MwdtStage::Stage0, HW_TIMEOUT);
    wdt.enable();
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        wdt.feed();
        if let Some(watch) = starving(watches, Instant::now().as_millis() as u32) {
            let mut crash = Crash::new(CrashKind::Watchdog, uptime_s());
            let _ = write!(crash, "task `{}` was not fed in time", watch.name());
            log::error!("watchdog: {crash}, restarting");
            restart_with(&crash);
        }
        ticker.next().await;
    }
}

#[cfg(feature = "panic-handler")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use crate::atomic::{AtomicBool, Ordering};
    use esp_println::println;

    static PANICKING: AtomicBool = AtomicBool::new(false);
    if PANICKING.swap(true, Ordering::Relaxed) {
        // Panicked while handling a panic: keep the first record.
        software_reset();
    }

    let mut crash = Crash::new(CrashKind::Panic, uptime_s());
    let _ = write!(crash, "{info}");
    let backtrace = esp_backtrace::Backtrace::capture();
    for frame in backtrace.frames().iter().take(crate::diag::BACKTRACE_LEN) {
        let _ = crash.backtrace.push(frame.program_counter() as u32);
    }

    println!("\n====================== PANIC ======================");
    println!("{info}\n\nBacktrace:\n");
    for frame in backtrace.frames() {
        println!("0x{:x}", frame.program_counter());
    }
    println!("\nrestarting\n");
    restart_with(&crash)
}
