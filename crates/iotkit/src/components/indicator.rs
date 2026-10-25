//! Status LEDs: [`status_led`] (one colour) and [`status_rgb`] (addressable, `smartled`) show
//! [`status`](crate::status) with the kit's [`pattern`]s.

#[cfg(feature = "_esp")]
use crate::status::Show;
use crate::status::{Hold, Status};

/// A blink pattern: each step lights for its first number of ms, then stays dark for the second
/// (0: lit on to the next step). Repeated, or played once and then dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// The steps, `(lit_ms, dark_ms)`.
    pub steps: &'static [(u16, u16)],
    /// Played again and again (else once).
    pub repeat: bool,
}

const fn repeat(steps: &'static [(u16, u16)]) -> Pattern {
    Pattern { steps, repeat: true }
}

const fn once(steps: &'static [(u16, u16)]) -> Pattern {
    Pattern { steps, repeat: false }
}

/// The kit's pattern for `status`.
pub const fn pattern(status: Status) -> Pattern {
    match status {
        Status::Held(Hold::Pairing) => repeat(&[(1000, 0)]),
        Status::Held(Hold::FactoryReset) => repeat(&[(50, 50)]),
        Status::Installing => repeat(&[(100, 100), (100, 700)]),
        Status::Pairing => repeat(&[(500, 500)]),
        Status::Offline => once(&[(100, 150), (100, 150), (100, 150)]),
        Status::Trial => repeat(&[(100, 100), (100, 1700)]),
        Status::Online => once(&[(1000, 0)]),
        Status::Connecting => repeat(&[(100, 100)]),
    }
}

/// The kit's colour for `status` (red, green, blue), for [`status_rgb`].
pub const fn color(status: Status) -> (u8, u8, u8) {
    match status {
        Status::Held(Hold::Pairing) | Status::Pairing => (0, 0, 255),
        Status::Held(Hold::FactoryReset) | Status::Offline => (255, 0, 0),
        Status::Installing | Status::Trial => (160, 0, 255),
        Status::Online => (0, 255, 0),
        Status::Connecting => (255, 160, 0),
    }
}

/// Plays `p` once (to its end, dark if not repeated) or forever.
#[cfg(feature = "_esp")]
async fn play(p: Pattern, mut light: impl AsyncFnMut(bool)) {
    use embassy_time::Timer;
    loop {
        for &(lit, dark) in p.steps {
            light(true).await;
            Timer::after_millis(lit.into()).await;
            if dark > 0 {
                light(false).await;
                Timer::after_millis(dark.into()).await;
            }
        }
        if !p.repeat {
            light(false).await;
            return;
        }
    }
}

/// Shows each status of `shown` with its pattern (dark when none); `light(status, on)` lights or
/// darkens the LED.
#[cfg(feature = "_esp")]
async fn show(shown: Show, mut light: impl AsyncFnMut(Status, bool)) -> ! {
    use embassy_futures::select::{Either, Either3, select, select3};
    let mut watcher = crate::status::watch(shown);
    // Dark before a deep sleep or a restart: a WS2812 keeps its last colour (and draws for it).
    let mut party = crate::runtime::shutdown::register();
    let shutdown = async |party: &mut Option<crate::runtime::shutdown::Party>| match party {
        Some(p) => p.requested().await,
        None => core::future::pending().await,
    };
    let mut status = watcher.next().await;
    // The last one lit, to darken.
    let mut lit = status.unwrap_or(Status::Connecting);
    loop {
        let played = async {
            if let Some(s) = status {
                play(pattern(s), async |on| light(s, on).await).await
            }
        };
        if let Some(s) = status {
            lit = s;
        }
        status = match select3(watcher.next(), played, shutdown(&mut party)).await {
            Either3::First(next) => next,
            // Played once (or nothing to show): dark until the next status.
            Either3::Second(()) => match select(watcher.next(), shutdown(&mut party)).await {
                Either::First(next) => next,
                Either::Second(()) => break,
            },
            Either3::Third(()) => break,
        };
        if status.is_none() {
            light(lit, false).await;
        }
    }
    light(lit, false).await;
    if let Some(p) = party.as_mut() {
        p.done();
    }
    core::future::pending().await
}

/// A one-colour status LED on `led` (see [`pattern`]) showing `shown` (its families). Off between
/// statuses, so the CPU sleeps.
#[cfg(feature = "_esp")]
pub async fn status_led(
    mut led: impl embedded_hal::digital::OutputPin,
    active: crate::components::Active,
    shown: Show,
) -> ! {
    use embedded_hal::digital::PinState;
    show(shown, async |_, on| {
        let _ = led.set_state(PinState::from(active.high_when(on)));
    })
    .await
}

/// `count` addressable LEDs showing the status (`shown`: its families) in [`color`]s at `level`
/// (0-255) brightness.
///
/// With esp-hal: an `esp_hal_smartled::RmtSmartLeds` on an RMT channel. A WS2812 draws about
/// 1 mA even dark: on battery, switch its power off during deep sleep.
#[cfg(all(feature = "_esp", feature = "smartled"))]
pub async fn status_rgb<L>(mut leds: L, count: usize, level: u8, shown: Show) -> !
where
    L: smart_leds::SmartLedsWriteAsync<Color = smart_leds::RGB8>,
    L::Error: core::fmt::Debug,
{
    use smart_leds::{RGB8, brightness};
    show(shown, async |status, on| {
        let (r, g, b) = color(status);
        let c = if on { RGB8::new(r, g, b) } else { RGB8::default() };
        if let Err(e) = leds.write(brightness(core::iter::repeat_n(c, count), level)).await {
            log::warn!("status_rgb write failed: {e:?}");
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let all = [
            Status::Held(Hold::Pairing),
            Status::Held(Hold::FactoryReset),
            Status::Installing,
            Status::Pairing,
            Status::Offline,
            Status::Trial,
            Status::Online,
            Status::Connecting,
        ];
        for s in all {
            let p = pattern(s);
            assert!(!p.steps.is_empty() && p.steps.iter().all(|&(lit, _)| lit > 0), "{s:?}");
            // A repeated pattern goes dark at some point, or is meant to stay lit.
            assert!(!p.repeat || p.steps.iter().any(|&(_, dark)| dark > 0) || s == Status::Held(Hold::Pairing));
        }
        // Told apart without colour.
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(pattern(*a), pattern(*b), "{a:?} and {b:?}");
            }
        }
    }
}
