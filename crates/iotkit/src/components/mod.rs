//! Ready-made parts: each is an `async fn` that drives one entity from embedded-hal hardware
//! (esp-hal's `Output`, `Input` and LEDC channels implement the traits). The app awaits it in
//! its own task, so a part it does not use is not linked.
//!
//! ```ignore
//! static PUMP: Switch = Switch::new("pump", "Pump");
//!
//! #[embassy_executor::task]
//! async fn pump_task(pin: Output<'static>) {
//!     components::relay(&PUMP, pin, Active::High).await
//! }
//! // in main:
//! spawner.spawn(pump_task(Output::new(p.GPIO5, Level::Low, OutputConfig::default())).unwrap());
//! ```
//!
//! A sensor is a read function and a filter chain, polled by [`poll`]. Any embedded-hal driver
//! fits in the read function:
//!
//! ```ignore
//! let mut sensor = SomeDriver::new(i2c);   // an embedded-hal-async I2C bus from the HAL
//! components::poll(&TEMPERATURE, Duration::from_secs(30), MovingAverage::<4>::new(), async || {
//!     sensor.read_celsius().await.ok()   // `None` on an error: the last value stays
//! })
//! .await
//! ```

pub mod filter;
mod gpio;
pub mod i2c;
mod indicator;
mod pwm;
mod sensor;
#[cfg(feature = "smartled")]
mod smartled;

pub use filter::Filter;
pub use gpio::{PressTiming, binary_input, button, relay};
#[cfg(feature = "_esp")]
pub use indicator::Overlay;
#[cfg(feature = "_esp")]
pub use indicator::status_led;
pub use indicator::{Frame, Pattern, STATUS_LEVEL, color, pattern};
#[cfg(all(feature = "_esp", feature = "smartled"))]
pub use indicator::{rgb_status_light, status_rgb};
pub use pwm::{dimmer, fan};
pub use sensor::poll;
#[cfg(feature = "smartled")]
pub use smartled::rgb_light;

/// Which pin level means on (pressed, detected).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Active {
    /// High is on.
    High,
    /// Low is on: a relay board switched by a low input, a button to ground.
    Low,
}

impl Active {
    /// The pin is high when `on`.
    const fn high_when(self, on: bool) -> bool {
        on == matches!(self, Active::High)
    }
}

#[cfg(test)]
mod tests;
