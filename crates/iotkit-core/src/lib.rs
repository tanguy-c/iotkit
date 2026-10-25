//! The chip-independent half of iotkit, tested on the host: the device model, typed settings in
//! flash, signed update images and manifests, and the health check. `iotkit` re-exports it.

#![no_std]
#![warn(missing_docs)]

/// `core::sync::atomic`, or `portable-atomic` on chips without compare-and-swap (ESP32-C3).
#[cfg(not(feature = "portable-atomic"))]
pub(crate) use core::sync::atomic;
#[cfg(feature = "portable-atomic")]
pub(crate) use portable_atomic as atomic;

pub mod config;
pub use iotkit_image as image;
pub mod model;
pub mod runtime;
pub mod time;
