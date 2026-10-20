//! On/off parts on a GPIO: relay, binary input, push button.

use crate::model::{BinarySensor, Event, Switch};
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use embedded_hal::digital::{InputPin, OutputPin, PinState};
use embedded_hal_async::digital::Wait;

use crate::components::Active;

/// A relay (or any on/off output). Starts off; applies each command and reports it.
pub async fn relay(entity: &Switch, mut pin: impl OutputPin, active: Active) -> ! {
    let mut on = false;
    loop {
        let _ = pin.set_state(PinState::from(active.high_when(on)));
        entity.set(on);
        on = entity.wait_command().await;
    }
}

/// A contact, motion or leak sensor: reports the level, read again `debounce` after each
/// change (bounces in between are ignored).
pub async fn binary_input(
    entity: &BinarySensor,
    mut pin: impl InputPin + Wait,
    active: Active,
    debounce: Duration,
) -> ! {
    loop {
        if let Ok(high) = pin.is_high() {
            entity.set(high == active.high_when(true));
        }
        let _ = pin.wait_for_any_edge().await;
        Timer::after(debounce).await;
    }
}

/// How presses are told apart.
#[derive(Clone, Copy, Debug)]
pub struct PressTiming {
    /// Contact bounce to ignore.
    pub debounce: Duration,
    /// Held this long: `long_press`.
    pub long: Duration,
    /// Longest gap between the presses of a double or triple press.
    pub gap: Duration,
}

impl PressTiming {
    /// 30 ms debounce, 800 ms long press, 300 ms between multiple presses.
    pub const DEFAULT: Self =
        Self { debounce: Duration::from_millis(30), long: Duration::from_millis(800), gap: Duration::from_millis(300) };
}

impl Default for PressTiming {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A push button as an event: `press`, `double_press`, `triple_press`, `long_press` (only the
/// types the entity has). A single press is sent at once when the entity has no multiple-press
/// type, else after `gap`.
pub async fn button(entity: &Event, mut pin: impl InputPin + Wait, active: Active, timing: PressTiming) -> ! {
    let types = entity.types();
    let max = [("triple_press", 3), ("double_press", 2)].iter().find(|(t, _)| types.contains(t)).map_or(1, |(_, n)| *n);
    loop {
        settle(&mut pin, active, true, timing.debounce).await;
        if let Either::Second(()) =
            select(settle(&mut pin, active, false, timing.debounce), Timer::after(timing.long)).await
        {
            entity.trigger_named("long_press");
            settle(&mut pin, active, false, timing.debounce).await;
            continue;
        }
        let mut count = 1;
        while count < max {
            match select(settle(&mut pin, active, true, timing.debounce), Timer::after(timing.gap)).await {
                Either::First(()) => {
                    settle(&mut pin, active, false, timing.debounce).await;
                    count += 1;
                }
                Either::Second(()) => break,
            }
        }
        entity.trigger_named(press_name(count));
    }
}

/// The event type of `count` presses in a row.
pub(crate) fn press_name(count: u8) -> &'static str {
    match count {
        1 => "press",
        2 => "double_press",
        _ => "triple_press",
    }
}

/// Waits until the pin is steadily `pressed` (or released) for `debounce`.
async fn settle(pin: &mut (impl InputPin + Wait), active: Active, pressed: bool, debounce: Duration) {
    let high = active.high_when(pressed);
    loop {
        let _ = if high { pin.wait_for_high().await } else { pin.wait_for_low().await };
        Timer::after(debounce).await;
        if pin.is_high().is_ok_and(|h| h == high) {
            return;
        }
    }
}
