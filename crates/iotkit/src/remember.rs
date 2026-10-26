//! Entities that keep their state across restarts (`remember` feature). Build a switch, light,
//! number or select with `.remember()`: [`start`](crate::start) hands each its saved state back
//! as its first command, and a task saves the states a few seconds after they settle.
//!
//! ```ignore
//! static LED: Light = Light::new("led", "LED").remember();
//! // Nothing else: the app takes commands and reports states as usual.
//! ```
//!
//! Over Matter, a light's colour comes back on the device, but the Color Control cluster reports
//! its own default until the next colour command (rs-matter cannot set it at start yet).

pub use iotkit_core::remember::*;

#[cfg(any(esp_wifi, all(feature = "_esp", feature = "matter-thread")))]
pub(crate) mod esp;
