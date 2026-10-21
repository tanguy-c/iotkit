//! The device model: what the device is, described once. MQTT, Matter and the ESPHome API are bindings
//! that read it.
//!
//! Entities are `static`s; any task updates them or waits for their commands:
//!
//! ```
//! use iotkit_core::model::{Entity, Sensor, Switch};
//!
//! static TEMPERATURE: Sensor = Sensor::temperature("temperature", "Chip temperature");
//! static RELAY: Switch = Switch::new("relay", "Relay");
//! static ENTITIES: &[&dyn Entity] = &[&TEMPERATURE, &RELAY];
//!
//! # RELAY.command(iotkit_core::model::Command::Set(true)); // what a binding does
//! # embassy_futures::block_on(async {
//! TEMPERATURE.set(21.5);
//! let on = RELAY.wait_command().await;   // from Home Assistant
//! RELAY.set(on);                         // report the new state
//! # });
//! ```
//!
//! A change marks the entity dirty for every [`Binding`] and wakes them ([`changed`]); each
//! reads [`Entity::state`] and publishes it, independently of the others.
//!
//! Every `wait_command` (and [`Button::wait_press`]) is cancel-safe: a command waits in the
//! entity until taken, and a newer one replaces it.

pub(crate) use crate::atomic;

mod appliances;
mod classes;
mod entities;
mod inputs;
mod update;

use crate::model::atomic::{AtomicU8, Ordering};

pub use appliances::{
    Climate, ClimateCommand, ClimateState, Cover, CoverCommand, CoverState, Fan, FanCommand, FanDirection, FanState,
    HvacAction, HvacMode, Lock, LockCommand, LockState, Valve,
};
pub use classes::{DeviceClass, Unit};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
pub use entities::{BinarySensor, Button, Light, LightState, Sensor, Switch};
pub use inputs::{Event, Number, Select, TEXT_LEN, TextSensor};
pub use update::{Update, UpdateState};

/// What kind of entity: decides the state, the commands, and how a binding shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A numeric reading.
    Sensor,
    /// An on/off reading.
    BinarySensor,
    /// An on/off output.
    Switch,
    /// A light: on/off, brightness, color.
    Light,
    /// A momentary action.
    Button,
    /// A value set from Home Assistant (a setpoint, a duration).
    Number,
    /// One of a fixed list of options.
    Select,
    /// A text reading.
    TextSensor,
    /// Something that happened (a button press): not a state.
    Event,
    /// An opening: blinds, garage door.
    Cover,
    /// A water or gas valve.
    Valve,
    /// A lock.
    Lock,
    /// A fan.
    Fan,
    /// A thermostat.
    Climate,
    /// The firmware: installed and latest version, install from the controller.
    Update,
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
    /// What it is.
    pub kind: Kind,
    /// Unique within the device: `[a-z0-9_]`.
    pub id: &'static str,
    /// Shown in Home Assistant.
    pub name: &'static str,
    /// What it measures or is, if set.
    pub device_class: Option<DeviceClass>,
    /// Unit of the value, if set.
    pub unit: Option<Unit>,
    /// Sensors: a measurement, so Home Assistant keeps statistics.
    pub measurement: bool,
    /// Material Design icon (`mdi:...`), if any.
    pub icon: Option<&'static str>,
    /// Where Home Assistant files it.
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
    /// Sensor or number.
    Number(f32),
    /// Binary sensor or switch.
    Bool(bool),
    /// Light.
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
        /// Moving or at rest, if known.
        state: Option<CoverState>,
        /// 0 closed, 100 open, if known.
        position: Option<u8>,
    },
    /// Lock.
    Lock(LockState),
    /// Fan.
    Fan(FanState),
    /// Climate.
    Climate(ClimateState),
    /// Update: read it with [`Entity::update`].
    Update,
}

/// A command from a controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Switch: on or off.
    Set(bool),
    /// Light: the fields to change.
    Light {
        /// Turn on or off.
        on: Option<bool>,
        /// 0 to 255.
        brightness: Option<u8>,
        /// Red, green, blue.
        rgb: Option<[u8; 3]>,
    },
    /// Button.
    Press,
    /// Number: the new value.
    Number(f32),
    /// Select: the index of the option.
    Option(u8),
    /// Cover or valve.
    Cover(CoverCommand),
    /// Lock.
    Lock(LockCommand),
    /// Fan.
    Fan(FanCommand),
    /// Climate.
    Climate(ClimateCommand),
    /// Update: install the latest version.
    Install,
}

/// What an entity kind can do beyond its [`Meta`], for the bindings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spec {
    /// Nothing more (sensors, switches, lights, buttons, text sensors).
    None,
    /// Number: its range.
    Number {
        /// Lowest value.
        min: f32,
        /// Highest value.
        max: f32,
        /// Step between values.
        step: f32,
        /// A slider rather than a box.
        slider: bool,
    },
    /// Select: its options.
    Select {
        /// The options, in order: a state or command carries the index.
        options: &'static [&'static str],
    },
    /// Event: its types.
    Event {
        /// The event types (`press`, `double_press`, ...): a state carries the index.
        types: &'static [&'static str],
    },
    /// Cover or valve: whether it takes a position (0 closed, 100 open).
    Cover {
        /// Takes a position.
        position: bool,
    },
    /// Whether it can also open (unlatch).
    Lock {
        /// Can open.
        open: bool,
    },
    /// Fan: its features.
    Fan {
        /// Takes a speed (percentage).
        speed: bool,
        /// Preset modes, if any.
        presets: &'static [&'static str],
        /// Can oscillate.
        oscillation: bool,
        /// Can change direction.
        direction: bool,
    },
    /// Climate: its modes and target range.
    Climate {
        /// The modes it takes.
        modes: &'static [HvacMode],
        /// Lowest target, °C.
        min: f32,
        /// Highest target, °C.
        max: f32,
        /// Step between targets.
        step: f32,
    },
}

/// An entity, as the bindings see it.
pub trait Entity: Sync {
    /// The static description.
    fn meta(&self) -> &Meta;
    /// The current state.
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
    /// The state of an update entity; `None` for the other kinds.
    fn update(&self) -> Option<UpdateState> {
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
    /// Shown in Home Assistant.
    pub manufacturer: &'static str,
    /// Shown in Home Assistant.
    pub model: &'static str,
    /// The firmware version, shown in Home Assistant.
    pub sw_version: &'static str,
    /// Every entity of the device, in order.
    pub entities: &'static [&'static dyn Entity],
}

impl Device {
    /// The entity with this id.
    pub fn entity(&self, id: &str) -> Option<&'static dyn Entity> {
        self.entities.iter().copied().find(|e| e.meta().id == id)
    }
}

/// A transport that publishes the entities. Each has its own dirty flags and wake-up.
///
/// A transport outside iotkit takes one with [`Binding::allocate`] (`extra-bindings` feature).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding(u8);

impl Binding {
    /// Home Assistant over MQTT.
    pub const MQTT: Binding = Binding(0);
    /// Matter.
    pub const MATTER: Binding = Binding(1);

    /// A binding of its own, for another transport: call it once, at start. `None` once the
    /// six there can be are taken.
    #[cfg(feature = "extra-bindings")]
    pub fn allocate() -> Option<Binding> {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        (usize::from(n) < BINDINGS).then_some(Binding(n))
    }

    const fn bit(self) -> u8 {
        1 << self.0
    }
}

/// Bindings there can be: one bit each in [`Dirty`].
const BINDINGS: usize = if cfg!(feature = "extra-bindings") { 8 } else { 2 };

/// The next binding [`Binding::allocate`] hands out.
#[cfg(feature = "extra-bindings")]
static NEXT: AtomicU8 = AtomicU8::new(2);

static CHANGED: [Signal<CriticalSectionRawMutex, ()>; BINDINGS] = [const { Signal::new() }; BINDINGS];

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
