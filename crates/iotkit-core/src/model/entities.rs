//! The entity types.

use core::cell::Cell;

use crate::model::atomic::{AtomicU8, AtomicU32, Ordering};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};

use crate::model::{Binding, Command, DeviceClass, Dirty, Entity, Kind, Meta, State, Unit};

macro_rules! builders {
    ($ty:ty) => {
        impl $ty {
            /// What it measures or is ([`DeviceClass`](crate::model::DeviceClass)).
            pub const fn device_class(mut self, class: $crate::model::DeviceClass) -> Self {
                self.meta.device_class = Some(class);
                self
            }
            /// Material Design icon (`mdi:...`).
            pub const fn icon(mut self, icon: &'static str) -> Self {
                self.meta.icon = Some(icon);
                self
            }
            /// Filed under the device's diagnostics.
            pub const fn diagnostic(mut self) -> Self {
                self.meta.category = $crate::model::Category::Diagnostic;
                self
            }
            /// Filed under the device's settings.
            pub const fn config(mut self) -> Self {
                self.meta.category = $crate::model::Category::Config;
                self
            }
        }
    };
}

pub(crate) use builders;

/// `.remember()`, for the kinds whose state one command restores.
macro_rules! remembers {
    ($ty:ty) => {
        #[cfg(feature = "remember")]
        impl $ty {
            /// Keeps the state across restarts: saved to flash once it settles, and handed back
            /// to the app as its first command ([`crate::remember`]).
            pub const fn remember(mut self) -> Self {
                self.meta.remember = true;
                self
            }
        }
    };
}

pub(crate) use remembers;

/// A numeric reading.
pub struct Sensor {
    meta: Meta,
    /// `f32` bits; `NAN` until set.
    value: AtomicU32,
    dirty: Dirty,
}

builders!(Sensor);

impl Sensor {
    /// A new entity (`id`: see [`Meta::id`]).
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self { meta: Meta::new(Kind::Sensor, id, name), value: AtomicU32::new(f32::NAN.to_bits()), dirty: Dirty::new() }
    }
    /// A measurement of `class` in `unit`, shown with `decimals`: what the shortcuts below build.
    pub const fn measuring(self, class: DeviceClass, unit: Unit, decimals: u8) -> Self {
        self.device_class(class).unit(unit).measurement().precision(decimals)
    }
    /// Temperature in °C, 1 decimal.
    pub const fn temperature(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::TEMPERATURE, Unit::CELSIUS, 1)
    }
    /// Relative humidity in %, 1 decimal.
    pub const fn humidity(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::HUMIDITY, Unit::PERCENT, 1)
    }
    /// Air pressure in hPa, 1 decimal.
    pub const fn pressure(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::PRESSURE, Unit::HECTOPASCAL, 1)
    }
    /// Light level in lux, no decimals.
    pub const fn illuminance(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::ILLUMINANCE, Unit::LUX, 0)
    }
    /// CO2 in ppm, no decimals.
    pub const fn carbon_dioxide(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::CARBON_DIOXIDE, Unit::PPM, 0)
    }
    /// PM2.5 in µg/m³, no decimals.
    pub const fn pm25(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::PM25, Unit::MICROGRAMS_PER_CUBIC_METER, 0)
    }
    /// Battery charge in %, no decimals.
    pub const fn battery(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::BATTERY, Unit::PERCENT, 0)
    }
    /// Voltage in V, 2 decimals.
    pub const fn voltage(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::VOLTAGE, Unit::VOLT, 2)
    }
    /// Power in W, 1 decimal.
    pub const fn power(id: &'static str, name: &'static str) -> Self {
        Self::new(id, name).measuring(DeviceClass::POWER, Unit::WATT, 1)
    }
    /// Unit of the value.
    pub const fn unit(mut self, unit: Unit) -> Self {
        self.meta.unit = Some(unit);
        self
    }
    /// A measurement: Home Assistant keeps its statistics.
    pub const fn measurement(mut self) -> Self {
        self.meta.measurement = true;
        self
    }
    /// Decimals to show.
    pub const fn precision(mut self, decimals: u8) -> Self {
        self.meta.precision = Some(decimals);
        self
    }

    /// Sets the value; publishes it when it differs. `NAN` makes it unknown.
    pub fn set(&self, value: f32) {
        if self.value.swap(value.to_bits(), Ordering::AcqRel) != value.to_bits() {
            self.dirty.mark();
        }
    }

    /// The last value, `None` until set.
    pub fn get(&self) -> Option<f32> {
        let v = f32::from_bits(self.value.load(Ordering::Acquire));
        (!v.is_nan()).then_some(v)
    }
}

impl Entity for Sensor {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Number)
    }
    fn command(&self, _: Command) -> bool {
        false
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}

const UNKNOWN: u8 = 2;

/// An on/off reading (motion, door, ...).
pub struct BinarySensor {
    meta: Meta,
    /// 0 off, 1 on, [`UNKNOWN`].
    value: AtomicU8,
    dirty: Dirty,
}

builders!(BinarySensor);

impl BinarySensor {
    /// A new entity (`id`: see [`Meta::id`]).
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self { meta: Meta::new(Kind::BinarySensor, id, name), value: AtomicU8::new(UNKNOWN), dirty: Dirty::new() }
    }
    /// Sets the value; publishes it when it differs.
    pub fn set(&self, on: bool) {
        if self.value.swap(u8::from(on), Ordering::AcqRel) != u8::from(on) {
            self.dirty.mark();
        }
    }
    /// The last value, `None` until set.
    pub fn get(&self) -> Option<bool> {
        match self.value.load(Ordering::Acquire) {
            UNKNOWN => None,
            v => Some(v == 1),
        }
    }
}

impl Entity for BinarySensor {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Bool)
    }
    fn command(&self, _: Command) -> bool {
        false
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}

/// Something to turn on and off. The app applies commands and reports the result with `set`.
pub struct Switch {
    meta: Meta,
    value: AtomicU8,
    /// The last command until the app reports a state: [`UNKNOWN`] for none.
    pending: AtomicU8,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, bool>,
}

builders!(Switch);
remembers!(Switch);

impl Switch {
    /// A new entity (`id`: see [`Meta::id`]).
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            meta: Meta::new(Kind::Switch, id, name),
            value: AtomicU8::new(UNKNOWN),
            pending: AtomicU8::new(UNKNOWN),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    /// Reports the state.
    pub fn set(&self, on: bool) {
        self.pending.store(UNKNOWN, Ordering::Release);
        if self.value.swap(u8::from(on), Ordering::AcqRel) != u8::from(on) {
            self.dirty.mark();
        }
    }
    /// The last reported state, `None` until set.
    pub fn get(&self) -> Option<bool> {
        match self.value.load(Ordering::Acquire) {
            UNKNOWN => None,
            v => Some(v == 1),
        }
    }
    /// The next command (the latest one, if several came meanwhile).
    pub async fn wait_command(&self) -> bool {
        self.commands.wait().await
    }
}

impl Entity for Switch {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Bool)
    }
    fn target(&self) -> State {
        match self.pending.load(Ordering::Acquire) {
            UNKNOWN => self.state(),
            v => State::Bool(v == 1),
        }
    }
    fn command(&self, command: Command) -> bool {
        match command {
            Command::Set(on) => {
                self.pending.store(u8::from(on), Ordering::Release);
                self.commands.signal(on);
                true
            }
            _ => false,
        }
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}

/// State of a [`Light`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightState {
    /// On or off.
    pub on: bool,
    /// 0-255.
    pub brightness: u8,
    /// Red, green, blue.
    pub rgb: [u8; 3],
}

impl LightState {
    /// `self` with the fields of a light command.
    pub fn apply(self, on: Option<bool>, brightness: Option<u8>, rgb: Option<[u8; 3]>) -> Self {
        let mut next = self;
        if let Some(b) = brightness {
            next.brightness = b;
            next.on = b > 0;
        }
        if let Some(rgb) = rgb {
            next.rgb = rgb;
            next.on = true;
        }
        if let Some(on) = on {
            next.on = on;
        }
        // Turning on at brightness 0 would show nothing.
        if next.on && next.brightness == 0 {
            next.brightness = 255;
        }
        next
    }
}

/// A dimmable RGB light. The app applies commands and reports the result with `set`.
pub struct Light {
    meta: Meta,
    state: Mutex<CriticalSectionRawMutex, Cell<Option<LightState>>>,
    /// What the last command asked for, until the app reports a state.
    pending: Mutex<CriticalSectionRawMutex, Cell<Option<LightState>>>,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, LightState>,
}

builders!(Light);
remembers!(Light);

impl Light {
    /// A new entity (`id`: see [`Meta::id`]).
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            meta: Meta::new(Kind::Light, id, name),
            state: Mutex::new(Cell::new(None)),
            pending: Mutex::new(Cell::new(None)),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    /// Reports the state.
    pub fn set(&self, state: LightState) {
        self.pending.lock(|p| p.set(None));
        if self.state.lock(|s| s.replace(Some(state))) != Some(state) {
            self.dirty.mark();
        }
    }
    /// The last reported state, `None` until set.
    pub fn get(&self) -> Option<LightState> {
        self.state.lock(Cell::get)
    }
    /// What the next command builds on (see [`Entity::target`]).
    fn target_state(&self) -> LightState {
        self.pending.lock(Cell::get).or_else(|| self.get()).unwrap_or_default()
    }
    /// The state a command asks for (the current one with the command's changes).
    pub async fn wait_command(&self) -> LightState {
        self.commands.wait().await
    }
}

impl Entity for Light {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Light)
    }
    fn target(&self) -> State {
        match self.pending.lock(Cell::get) {
            Some(pending) => State::Light(pending),
            None => self.state(),
        }
    }
    fn command(&self, command: Command) -> bool {
        match command {
            Command::Light { on, brightness, rgb } => {
                let next = self.target_state().apply(on, brightness, rgb);
                self.pending.lock(|p| p.set(Some(next)));
                self.commands.signal(next);
                true
            }
            Command::Set(on) => self.command(Command::Light { on: Some(on), brightness: None, rgb: None }),
            _ => false,
        }
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}

/// An action (restart, identify, ...).
pub struct Button {
    meta: Meta,
    presses: Signal<CriticalSectionRawMutex, ()>,
    dirty: Dirty,
}

builders!(Button);

impl Button {
    /// A new entity (`id`: see [`Meta::id`]).
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self { meta: Meta::new(Kind::Button, id, name), presses: Signal::new(), dirty: Dirty::new() }
    }
    /// The next press.
    pub async fn wait_press(&self) {
        self.presses.wait().await
    }
}

impl Entity for Button {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        State::None
    }
    fn command(&self, command: Command) -> bool {
        if command == Command::Press {
            self.presses.signal(());
            true
        } else {
            false
        }
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}
