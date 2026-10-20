use core::{
    convert::Infallible,
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
};
use std::sync::Mutex;

use embassy_futures::{block_on, select::select, yield_now};
use embassy_time::{Duration, MockDriver};
use embedded_hal::digital::{ErrorType, InputPin, OutputPin};
use embedded_hal_async::digital::Wait;
use iotkit_model::{BinarySensor, Command, Entity, Event, State, Switch};

use crate::{
    Active, PressTiming, binary_input, button,
    filter::{Clamp, Delta, Filter, Linear, MovingAverage},
    gpio::press_name,
    relay,
};

extern crate std;

/// The mock clock is global: one timed test at a time.
static CLOCK: Mutex<()> = Mutex::new(());

/// A pin whose level the test sets (input) or reads (output).
struct Pin(&'static AtomicBool);

impl ErrorType for Pin {
    type Error = Infallible;
}
impl InputPin for Pin {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.0.load(Ordering::SeqCst))
    }
    fn is_low(&mut self) -> Result<bool, Infallible> {
        Ok(!self.0.load(Ordering::SeqCst))
    }
}
impl OutputPin for Pin {
    fn set_high(&mut self) -> Result<(), Infallible> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn set_low(&mut self) -> Result<(), Infallible> {
        self.0.store(false, Ordering::SeqCst);
        Ok(())
    }
}
impl Wait for Pin {
    async fn wait_for_high(&mut self) -> Result<(), Infallible> {
        while !self.0.load(Ordering::SeqCst) {
            yield_now().await;
        }
        Ok(())
    }
    async fn wait_for_low(&mut self) -> Result<(), Infallible> {
        while self.0.load(Ordering::SeqCst) {
            yield_now().await;
        }
        Ok(())
    }
    async fn wait_for_rising_edge(&mut self) -> Result<(), Infallible> {
        self.wait_for_low().await?;
        self.wait_for_high().await
    }
    async fn wait_for_falling_edge(&mut self) -> Result<(), Infallible> {
        self.wait_for_high().await?;
        self.wait_for_low().await
    }
    async fn wait_for_any_edge(&mut self) -> Result<(), Infallible> {
        let start = self.0.load(Ordering::SeqCst);
        while self.0.load(Ordering::SeqCst) == start {
            yield_now().await;
        }
        Ok(())
    }
}

/// Lets the part run until it waits again.
async fn settle() {
    for _ in 0..20 {
        yield_now().await;
    }
}

/// Advances the mock clock by `ms`, in steps, letting the part run.
async fn wait_ms(ms: u64) {
    for _ in 0..ms / 10 {
        MockDriver::get().advance(Duration::from_millis(10));
        settle().await;
    }
}

/// Runs `part` (which never ends) next to the test `script`.
fn run(part: impl Future, script: impl Future<Output = ()>) {
    block_on(select(part, script));
}

#[test]
fn filters() {
    let mut percent = Linear::new((3000.0, 0.0), (1200.0, 100.0)).then(Clamp::new(0.0, 100.0));
    let near = |got: Option<f32>, want: f32| got.is_some_and(|v| (v - want).abs() < 1e-3);
    assert!(near(percent.apply(3000.0), 0.0) && near(percent.apply(2100.0), 50.0) && near(percent.apply(500.0), 100.0));
    let mut mean = MovingAverage::<3>::new();
    assert_eq!([1.0, 2.0, 3.0, 7.0].map(|v| mean.apply(v)), [Some(1.0), Some(1.5), Some(2.0), Some(4.0)]);
    let mut delta = Delta::new(0.5);
    assert_eq!([20.0, 20.2, 20.6].map(|v| delta.apply(v)), [Some(20.0), None, Some(20.6)]);
    let mut positive = (|v: f32| (v >= 0.0).then_some(v)).then(Clamp::new(0.0, 1.0));
    assert_eq!((positive.apply(-1.0), positive.apply(3.0)), (None, Some(1.0)));
}

#[test]
fn press_names() {
    assert_eq!([1, 2, 3].map(press_name), ["press", "double_press", "triple_press"]);
}

#[test]
fn relay_follows_commands() {
    static SWITCH: Switch = Switch::new("relay", "Relay");
    static LEVEL: AtomicBool = AtomicBool::new(false);
    run(relay(&SWITCH, Pin(&LEVEL), Active::Low), async {
        settle().await;
        assert!(LEVEL.load(Ordering::SeqCst), "starts off: high for an active-low relay");
        assert_eq!(SWITCH.get(), Some(false));
        SWITCH.command(Command::Set(true));
        settle().await;
        assert!(!LEVEL.load(Ordering::SeqCst));
        assert_eq!(SWITCH.get(), Some(true));
    });
}

#[test]
fn binary_input_debounces() {
    let _clock = CLOCK.lock().unwrap();
    static DOOR: BinarySensor = BinarySensor::new("door", "Door");
    static LEVEL: AtomicBool = AtomicBool::new(false);
    run(binary_input(&DOOR, Pin(&LEVEL), Active::High, Duration::from_millis(50)), async {
        settle().await;
        assert_eq!(DOOR.get(), Some(false));
        LEVEL.store(true, Ordering::SeqCst);
        settle().await;
        LEVEL.store(false, Ordering::SeqCst); // a bounce
        settle().await;
        LEVEL.store(true, Ordering::SeqCst);
        assert_eq!(DOOR.get(), Some(false), "not before the debounce time");
        wait_ms(60).await;
        assert_eq!(DOOR.get(), Some(true));
    });
}

/// Presses (`true`) and releases the button for `ms` each.
async fn press(level: &AtomicBool, pressed_ms: u64, released_ms: u64) {
    level.store(false, Ordering::SeqCst); // active low: pressed
    wait_ms(pressed_ms).await;
    level.store(true, Ordering::SeqCst);
    wait_ms(released_ms).await;
}

fn last_event(event: &Event) -> Option<&'static str> {
    match event.state() {
        State::Event(i) => Some(event.types()[usize::from(i)]),
        _ => None,
    }
}

#[test]
fn button_presses() {
    let _clock = CLOCK.lock().unwrap();
    static BUTTON: Event = Event::new("button", "Button", &["press", "double_press", "long_press"]);
    static LEVEL: AtomicBool = AtomicBool::new(true);
    run(button(&BUTTON, Pin(&LEVEL), Active::Low, PressTiming::DEFAULT), async {
        settle().await;
        press(&LEVEL, 100, 400).await;
        assert_eq!(last_event(&BUTTON), Some("press"));
        press(&LEVEL, 100, 100).await;
        press(&LEVEL, 100, 400).await;
        assert_eq!(last_event(&BUTTON), Some("double_press"));
        press(&LEVEL, 1000, 100).await;
        assert_eq!(last_event(&BUTTON), Some("long_press"));
    });
}

#[test]
fn single_press_is_immediate_without_multiple_presses() {
    let _clock = CLOCK.lock().unwrap();
    static BUTTON: Event = Event::new("bell", "Bell", &["press"]);
    static LEVEL: AtomicBool = AtomicBool::new(true);
    run(button(&BUTTON, Pin(&LEVEL), Active::Low, PressTiming::DEFAULT), async {
        settle().await;
        press(&LEVEL, 100, 50).await;
        assert_eq!(last_event(&BUTTON), Some("press"), "no wait for a second press");
    });
}
