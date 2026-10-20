//! Typed, versioned settings in flash: the store is [`iotkit_core::config`], and [`esp`] puts it
//! in the chip's `config` partition.

pub use iotkit_core::config::*;

#[cfg(feature = "_esp")]
pub mod esp;

#[cfg(feature = "_esp")]
pub(crate) use crate::atomic;
