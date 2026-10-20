//! First-boot Wi-Fi setup: [Improv](improv) over Bluetooth LE and the USB serial port, and a
//! setup access point with a captive [portal] ([`dhcp`], [`dns`]).
//!
//! The protocols are host-testable; the `esp` module (chip feature) runs them: it hands the
//! received network to `iotkit-net`, which tries it, and stores it once it works.

#![no_std]
#![warn(missing_docs)]

pub mod dhcp;
pub mod dns;
#[cfg(feature = "_esp")]
pub mod esp;
pub mod gesture;
pub mod improv;
pub mod portal;
