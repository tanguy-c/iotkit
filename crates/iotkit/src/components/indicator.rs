//! Status LEDs: [`status_led`] (one colour) and [`status_rgb`] (addressable, `smartled`) show
//! [`status`](crate::status) with the kit's [`pattern`]s; [`rgb_status_light`] (`smartled`) is a
//! light that shows it too. An app driving its LED itself follows it with an [`Overlay`].

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

impl Pattern {
    /// `elapsed_ms` after it started: lit or dark, and for how many ms more; `None` once a
    /// pattern played once is over.
    pub const fn at(&self, elapsed_ms: u32) -> Option<(bool, u32)> {
        let mut period = 0u32;
        let mut i = 0;
        while i < self.steps.len() {
            period += self.steps[i].0 as u32 + self.steps[i].1 as u32;
            i += 1;
        }
        if period == 0 || (!self.repeat && elapsed_ms >= period) {
            return None;
        }
        let mut t = elapsed_ms % period;
        let mut i = 0;
        while i < self.steps.len() {
            let (lit, dark) = (self.steps[i].0 as u32, self.steps[i].1 as u32);
            if t < lit {
                return Some((true, lit - t));
            }
            t -= lit;
            if t < dark {
                return Some((false, dark - t));
            }
            t -= dark;
            i += 1;
        }
        None
    }
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

/// What a LED shared with the status shows now: a step of the status's [`pattern`], or the
/// app's own content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    /// The status: lit (in its [`color`]) or dark.
    Status {
        /// What is shown.
        status: Status,
        /// Lit, or dark between blinks.
        lit: bool,
    },
    /// No status to show (none, or a pattern played once is over): the app's content.
    Own,
}

/// Follows [`status`](crate::status) step by step, for a LED that does something else when no
/// status is shown (a light). Cancel-safe: `select` it with the app's own events.
///
/// ```ignore
/// let mut overlay = Overlay::new(Show::All);
/// loop {
///     match select(overlay.next(), LIGHT.wait_command()).await {
///         Either::First(Frame::Status { status, lit }) => { /* color(status), or dark */ }
///         Either::First(Frame::Own) => { /* the light */ }
///         Either::Second(state) => { /* a command */ }
///     }
/// }
/// ```
#[cfg(feature = "_esp")]
pub struct Overlay {
    watcher: crate::status::Watcher,
    /// The status and when its pattern started; `None` before the first `next`.
    status: Option<(Option<Status>, embassy_time::Instant)>,
    last: Option<Frame>,
}

#[cfg(feature = "_esp")]
impl Overlay {
    /// Follows the statuses `shown` shows. Takes one of the 4 [`watch`](crate::status::watch)ers.
    pub fn new(shown: Show) -> Self {
        Self { watcher: crate::status::watch(shown), status: None, last: None }
    }

    /// The frame: at once the first time, then each time it changes.
    pub async fn next(&mut self) -> Frame {
        use embassy_futures::select::{Either, select};
        use embassy_time::{Instant, Timer};
        loop {
            let (status, since) = match self.status {
                Some(s) => s,
                // Ready at once the first time.
                None => *self.status.insert((self.watcher.next().await, Instant::now())),
            };
            let now = Instant::now();
            let step =
                status.and_then(|s| pattern(s).at(now.duration_since(since).as_millis() as u32).map(|st| (s, st)));
            let (frame, until) = match step {
                Some((status, (lit, ms))) => {
                    (Frame::Status { status, lit }, Some(now + embassy_time::Duration::from_millis(ms.into())))
                }
                None => (Frame::Own, None),
            };
            if self.last != Some(frame) {
                self.last = Some(frame);
                return frame;
            }
            let step_over = async {
                match until {
                    Some(at) => Timer::at(at).await,
                    None => core::future::pending().await,
                }
            };
            if let Either::First(next) = select(self.watcher.next(), step_over).await {
                self.status = Some((next, Instant::now()));
            }
        }
    }
}

/// Shows each status of `shown` with its pattern (dark when none); `light(status, on)` lights or
/// darkens the LED.
#[cfg(feature = "_esp")]
async fn show(shown: Show, mut light: impl AsyncFnMut(Status, bool)) -> ! {
    use embassy_futures::select::{Either, select};
    let mut overlay = Overlay::new(shown);
    // Dark before a deep sleep or a restart: a WS2812 keeps its last colour (and draws for it).
    let mut party = crate::runtime::shutdown::register();
    // The last one lit, to darken.
    let mut lit = None;
    loop {
        match select(overlay.next(), shutdown(&mut party)).await {
            Either::First(Frame::Status { status, lit: on }) => {
                light(status, on).await;
                lit = Some(status);
            }
            Either::First(Frame::Own) => {
                if let Some(s) = lit.take() {
                    light(s, false).await;
                }
            }
            Either::Second(()) => break,
        }
    }
    if let Some(s) = lit {
        light(s, false).await;
    }
    if let Some(p) = party.as_mut() {
        p.done();
    }
    core::future::pending().await
}

/// The restart or deep sleep `party` (if any) waits for.
#[cfg(feature = "_esp")]
async fn shutdown(party: &mut Option<crate::runtime::shutdown::Party>) {
    match party {
        Some(p) => p.requested().await,
        None => core::future::pending().await,
    }
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

/// Brightness of the status on [`rgb_status_light`] (0-255): seen, not dazzling.
pub const STATUS_LEVEL: u8 = 32;

/// `count` addressable LEDs as an RGB light (`entity`) that shows the status too: while one of
/// `shown` plays (see [`pattern`]), the LEDs show it in its [`color`], then the light again.
/// With `toggle_on_press`, a short press of the setup button toggles the light. Starts off,
/// white: a remembered state (`.remember()`) comes back as a command.
///
/// With esp-hal: an `esp_hal_smartled::RmtSmartLeds` on an RMT channel.
#[cfg(all(feature = "_esp", feature = "smartled"))]
pub async fn rgb_status_light<L>(
    entity: &crate::model::Light,
    mut leds: L,
    count: usize,
    shown: Show,
    toggle_on_press: bool,
) -> !
where
    L: smart_leds::SmartLedsWriteAsync<Color = smart_leds::RGB8>,
    L::Error: core::fmt::Debug,
{
    use crate::model::{Entity, LightState};
    use embassy_futures::select::{Either4, select4};
    use smart_leds::{RGB8, brightness};
    let press = async || {
        if toggle_on_press { crate::provision::gesture::wait_press().await } else { core::future::pending().await }
    };
    let mut overlay = Overlay::new(shown);
    let mut party = crate::runtime::shutdown::register();
    let mut state = LightState { on: false, brightness: 64, rgb: [255, 255, 255] };
    let mut frame = Frame::Own;
    entity.set(state);
    loop {
        let (color, level) = match frame {
            Frame::Status { status, lit: true } => {
                let (r, g, b) = color(status);
                (RGB8::new(r, g, b), STATUS_LEVEL)
            }
            Frame::Status { lit: false, .. } => (RGB8::default(), 0),
            Frame::Own if state.on => (RGB8::new(state.rgb[0], state.rgb[1], state.rgb[2]), state.brightness),
            Frame::Own => (RGB8::default(), 0),
        };
        if let Err(e) = leds.write(brightness(core::iter::repeat_n(color, count), level)).await {
            log::warn!("{}: rgb_status_light write failed: {e:?}", entity.meta().id);
        }
        match select4(overlay.next(), entity.wait_command(), press(), shutdown(&mut party)).await {
            Either4::First(f) => frame = f,
            Either4::Second(wanted) => state = wanted,
            Either4::Third(()) => state = LightState { on: !state.on, ..state },
            Either4::Fourth(()) => break,
        }
        entity.set(state);
    }
    // Dark before a deep sleep or a restart: a WS2812 keeps its last colour.
    let _ = leds.write(core::iter::repeat_n(RGB8::default(), count)).await;
    if let Some(p) = party.as_mut() {
        p.done();
    }
    core::future::pending().await
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
        // The steps in time: lit, then dark, then the next; a pattern played once ends.
        let p = Pattern { steps: &[(100, 200), (300, 0)], repeat: false };
        assert_eq!(p.at(0), Some((true, 100)));
        assert_eq!(p.at(150), Some((false, 150)));
        assert_eq!(p.at(300), Some((true, 300)));
        assert_eq!(p.at(599), Some((true, 1)));
        assert_eq!(p.at(600), None);
        let p = Pattern { repeat: true, ..p };
        assert_eq!(p.at(600), Some((true, 100)), "again");
        assert_eq!(p.at(6150), Some((false, 150)));
        assert_eq!(pattern(Status::Held(Hold::Pairing)).at(5000), Some((true, 1000)), "lit on");
        // Told apart without colour.
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(pattern(*a), pattern(*b), "{a:?} and {b:?}");
            }
        }
    }
}
