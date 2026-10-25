//! BOOT button gestures: hold 3 s to open provisioning, 10 s to factory-reset.

/// Hold at least this long (ms) to open provisioning.
pub const PROVISION_MS: u32 = 3_000;
/// Hold at least this long (ms) to factory-reset.
pub const FACTORY_RESET_MS: u32 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// What a press of the BOOT button asks for.
pub enum Gesture {
    /// Shorter than [`PROVISION_MS`]: left to the app.
    Press,
    /// Open provisioning (held [`PROVISION_MS`]).
    Provision,
    /// Erase the settings and restart (held [`FACTORY_RESET_MS`]).
    FactoryReset,
}

/// A short press of the BOOT button (under [`PROVISION_MS`]), for the app.
#[cfg(feature = "_esp")]
static PRESSED: embassy_sync::signal::Signal<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, ()> =
    embassy_sync::signal::Signal::new();

/// The BOOT button task saw a short press.
#[cfg(any(esp_wifi, all(feature = "_esp", feature = "matter-thread")))]
pub(crate) fn pressed() {
    PRESSED.signal(());
}

/// Waits for a short press of the BOOT button (the kit's button task must run: `start` with a
/// `boot_button`).
#[cfg(feature = "_esp")]
pub async fn wait_press() {
    PRESSED.wait().await
}

/// How long the button has been held, for the app's LED: 0 not held or under 3 s, 1 past
/// [`PROVISION_MS`], 2 past [`FACTORY_RESET_MS`]. [`status`](crate::status) shows it too.
#[cfg(feature = "_esp")]
pub static HELD: crate::atomic::AtomicU8 = crate::atomic::AtomicU8::new(0);

#[cfg(any(esp_wifi, all(feature = "_esp", feature = "matter-thread")))]
fn set_held(held: u8) {
    if HELD.swap(held, crate::atomic::Ordering::Relaxed) != held {
        crate::status::notify();
    }
}

/// The button is down: a deep-sleeping device waits for the gesture.
#[cfg(any(esp_wifi, all(feature = "_esp", feature = "matter-thread")))]
static PRESSING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether the button is down now.
#[cfg(all(esp_wifi, feature = "deep-sleep"))]
pub(crate) fn pressing() -> bool {
    PRESSING.load(core::sync::atomic::Ordering::Relaxed)
}

/// Waits for the next gesture on `button` (active low), [`HELD`] showing the thresholds reached.
/// Interrupt-driven: no polling that would keep the CPU from sleeping. `since`: when a press
/// began, if it began before (the press that woke the chip).
#[cfg(any(esp_wifi, all(feature = "_esp", feature = "matter-thread")))]
pub(crate) async fn next(button: &mut esp_hal::gpio::Input<'static>, since: Option<embassy_time::Instant>) -> Gesture {
    use embassy_futures::select::{Either, select};
    use embassy_time::{Duration, Instant, Timer};
    let pressed = match since {
        Some(at) => at,
        None => {
            button.wait_for_low().await;
            let at = Instant::now();
            Timer::after_millis(50).await; // bounces
            at
        }
    };
    PRESSING.store(true, core::sync::atomic::Ordering::Relaxed);
    let held = loop {
        let held = pressed.elapsed().as_millis() as u32;
        set_held(u8::from(held >= PROVISION_MS) + u8::from(held >= FACTORY_RESET_MS));
        let threshold = if held < PROVISION_MS { PROVISION_MS } else { FACTORY_RESET_MS };
        let shown = Timer::at(pressed + Duration::from_millis(threshold.into()));
        if held >= FACTORY_RESET_MS {
            button.wait_for_high().await;
        } else if let Either::Second(()) = select(button.wait_for_high(), shown).await {
            continue;
        }
        break pressed.elapsed().as_millis() as u32;
    };
    Timer::after_millis(50).await;
    PRESSING.store(false, core::sync::atomic::Ordering::Relaxed);
    set_held(0);
    on_release(held)
}

/// The gesture of a press held `held_ms`, decided at release.
pub fn on_release(held_ms: u32) -> Gesture {
    match held_ms {
        FACTORY_RESET_MS.. => Gesture::FactoryReset,
        PROVISION_MS.. => Gesture::Provision,
        _ => Gesture::Press,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        assert_eq!(on_release(100), Gesture::Press);
        assert_eq!(on_release(2_999), Gesture::Press);
        assert_eq!(on_release(3_000), Gesture::Provision);
        assert_eq!(on_release(9_999), Gesture::Provision);
        assert_eq!(on_release(10_000), Gesture::FactoryReset);
        assert_eq!(on_release(60_000), Gesture::FactoryReset);
    }
}
