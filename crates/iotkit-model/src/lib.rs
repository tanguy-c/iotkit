//! The device model: what the device is, described once. MQTT (and later Matter) are bindings
//! that read it.
//!
//! Entities are `static`s; any task updates them or waits for their commands:
//!
//! ```ignore
//! static TEMPERATURE: Sensor = Sensor::new("temperature", "Chip temperature")
//!     .unit("°C").device_class("temperature").measurement();
//! static RELAY: Switch = Switch::new("relay", "Relay");
//! static ENTITIES: [&dyn Entity; 2] = [&TEMPERATURE, &RELAY];
//!
//! TEMPERATURE.set(21.5);
//! let on = RELAY.wait_command().await;   // from Home Assistant
//! RELAY.set(on);                         // report the new state
//! ```
//!
//! A change marks the entity dirty for every [`Binding`] and wakes them ([`changed`]); each
//! reads [`Entity::state`] and publishes it, independently of the others.

#![no_std]

/// `core::sync::atomic`, or `portable-atomic` on chips without compare-and-swap (ESP32-C3).
#[cfg(not(feature = "portable-atomic"))]
pub(crate) use core::sync::atomic;
#[cfg(feature = "portable-atomic")]
pub(crate) use portable_atomic as atomic;

mod appliances;
mod entities;
mod inputs;

use crate::atomic::{AtomicU8, Ordering};

pub use appliances::{
    Climate, ClimateCommand, ClimateState, Cover, CoverCommand, CoverState, Fan, FanCommand, FanDirection, FanState,
    HvacAction, HvacMode, Lock, LockCommand, LockState, Valve,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
pub use entities::{BinarySensor, Button, Light, LightState, Sensor, Switch};
pub use inputs::{Event, Number, Select, TEXT_LEN, TextSensor};

/// What kind of entity: decides the state, the commands, and how a binding shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sensor,
    BinarySensor,
    Switch,
    Light,
    Button,
    /// A value set from Home Assistant (a setpoint, a duration).
    Number,
    /// One of a fixed list of options.
    Select,
    /// A text reading.
    TextSensor,
    /// Something that happened (a button press): not a state.
    Event,
    Cover,
    Valve,
    Lock,
    Fan,
    /// A thermostat.
    Climate,
}

/// Where Home Assistant files an entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// A main control or reading.
    Primary,
    /// Device settings.
    Config,
    /// Device health (RSSI, uptime, restart).
    Diagnostic,
}

/// Static description of an entity.
#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub kind: Kind,
    /// Unique within the device: `[a-z0-9_]`.
    pub id: &'static str,
    pub name: &'static str,
    /// Home Assistant device class (`temperature`, `restart`, ...), if any.
    pub device_class: Option<&'static str>,
    pub unit: Option<&'static str>,
    /// Sensors: a measurement, so Home Assistant keeps statistics.
    pub measurement: bool,
    pub icon: Option<&'static str>,
    pub category: Category,
    /// Sensors: decimals to show.
    pub precision: Option<u8>,
}

/// Longest entity id: it goes into topics and unique ids of fixed size.
pub const MAX_ID_LEN: usize = 48;

impl Meta {
    /// Fails the build (entities are `static`s) on an id that is empty, longer than
    /// [`MAX_ID_LEN`], or not `[a-z0-9_]`.
    const fn new(kind: Kind, id: &'static str, name: &'static str) -> Self {
        let bytes = id.as_bytes();
        assert!(!bytes.is_empty() && bytes.len() <= MAX_ID_LEN, "entity id: 1 to 48 characters");
        let mut i = 0;
        while i < bytes.len() {
            assert!(matches!(bytes[i], b'a'..=b'z' | b'0'..=b'9' | b'_'), "entity id: only [a-z0-9_]");
            i += 1;
        }
        Self {
            kind,
            id,
            name,
            device_class: None,
            unit: None,
            measurement: false,
            icon: None,
            category: Category::Primary,
            precision: None,
        }
    }
}

/// The current state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// Never set (shown as unknown).
    Unknown,
    Number(f32),
    Bool(bool),
    Light(LightState),
    /// Buttons have no state.
    None,
    /// Select: the index of the option.
    Option(u8),
    /// Text sensor: read it with [`Entity::text`].
    Text,
    /// Event: the index of the last event type triggered.
    Event(u8),
    /// Cover or valve.
    Cover {
        state: Option<CoverState>,
        position: Option<u8>,
    },
    Lock(LockState),
    Fan(FanState),
    Climate(ClimateState),
}

/// A command from a controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Switch: on or off.
    Set(bool),
    /// Light: the fields to change.
    Light {
        on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<[u8; 3]>,
    },
    /// Button.
    Press,
    /// Number: the new value.
    Number(f32),
    /// Select: the index of the option.
    Option(u8),
    Cover(CoverCommand),
    Lock(LockCommand),
    Fan(FanCommand),
    Climate(ClimateCommand),
}

/// What an entity kind can do beyond its [`Meta`], for the bindings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spec {
    /// Nothing more (sensors, switches, lights, buttons, text sensors).
    None,
    Number {
        min: f32,
        max: f32,
        step: f32,
        slider: bool,
    },
    Select {
        options: &'static [&'static str],
    },
    Event {
        types: &'static [&'static str],
    },
    /// Cover or valve: whether it takes a position (0 closed, 100 open).
    Cover {
        position: bool,
    },
    /// Whether it can also open (unlatch).
    Lock {
        open: bool,
    },
    Fan {
        speed: bool,
        presets: &'static [&'static str],
        oscillation: bool,
        direction: bool,
    },
    Climate {
        modes: &'static [HvacMode],
        min: f32,
        max: f32,
        step: f32,
    },
}

/// An entity, as the bindings see it.
pub trait Entity: Sync {
    fn meta(&self) -> &Meta;
    fn state(&self) -> State;
    /// The state the next command builds on: what the last command asked for until the app
    /// reports a state, else the state. A toggle or a partial light command must start from it,
    /// so commands that come before the app has applied the previous ones are not lost.
    fn target(&self) -> State {
        self.state()
    }
    /// What the kind can do beyond [`Meta`] (options, ranges, features).
    fn spec(&self) -> Spec {
        Spec::None
    }
    /// The text of a text sensor; `None` for the other kinds, or before the first `set`.
    fn text(&self) -> Option<heapless::String<TEXT_LEN>> {
        None
    }
    /// Hands a command to the app. `false` if the entity takes no such command.
    fn command(&self, command: Command) -> bool;
    /// Whether the state changed for `binding` since its last call (and clears it).
    fn take_dirty(&self, binding: Binding) -> bool;
    /// Marks the state as changed for `binding`, e.g. to publish it again after a reconnection.
    fn mark_dirty(&self, binding: Binding);
}

/// The device the entities belong to.
#[derive(Clone, Copy)]
pub struct Device {
    /// Shown in Home Assistant.
    pub name: &'static str,
    pub manufacturer: &'static str,
    pub model: &'static str,
    pub sw_version: &'static str,
    pub entities: &'static [&'static dyn Entity],
}

impl Device {
    pub fn entity(&self, id: &str) -> Option<&'static dyn Entity> {
        self.entities.iter().copied().find(|e| e.meta().id == id)
    }
}

/// A transport that publishes the entities. Each has its own dirty flags and wake-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding(u8);

impl Binding {
    pub const MQTT: Binding = Binding(0);
    pub const MATTER: Binding = Binding(1);

    const fn bit(self) -> u8 {
        1 << self.0
    }
}

/// Bindings there can be.
const BINDINGS: usize = 2;

static CHANGED: [Signal<CriticalSectionRawMutex, ()>; BINDINGS] = [Signal::new(), Signal::new()];

/// Waits until some entity changed for `binding` (check [`Entity::take_dirty`] after).
pub async fn changed(binding: Binding) {
    CHANGED[usize::from(binding.0)].wait().await
}

/// Dirty flags, one bit per binding, shared by the entity types.
pub(crate) struct Dirty(AtomicU8);

impl Dirty {
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(0))
    }
    /// Changed for every binding.
    pub(crate) fn mark(&self) {
        self.0.store(u8::MAX, Ordering::Release);
        CHANGED.iter().for_each(|s| s.signal(()));
    }
    pub(crate) fn mark_for(&self, binding: Binding) {
        self.0.fetch_or(binding.bit(), Ordering::AcqRel);
        CHANGED[usize::from(binding.0)].signal(());
    }
    pub(crate) fn take(&self, binding: Binding) -> bool {
        self.0.fetch_and(!binding.bit(), Ordering::AcqRel) & binding.bit() != 0
    }
}

#[cfg(test)]
mod tests;
