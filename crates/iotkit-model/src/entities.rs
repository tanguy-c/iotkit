//! The entity types.

use core::cell::Cell;

use crate::atomic::{AtomicU8, AtomicU32, Ordering};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};

use crate::{Binding, Command, Dirty, Entity, Kind, Meta, State};

macro_rules! builders {
    ($ty:ty) => {
        impl $ty {
            pub const fn device_class(mut self, class: &'static str) -> Self {
                self.meta.device_class = Some(class);
                self
            }
            pub const fn icon(mut self, icon: &'static str) -> Self {
                self.meta.icon = Some(icon);
                self
            }
            /// Filed under the device's diagnostics.
            pub const fn diagnostic(mut self) -> Self {
                self.meta.category = $crate::Category::Diagnostic;
                self
            }
            /// Filed under the device's settings.
            pub const fn config(mut self) -> Self {
                self.meta.category = $crate::Category::Config;
                self
            }
        }
    };
}

pub(crate) use builders;

/// A numeric reading.
pub struct Sensor {
    meta: Meta,
    /// `f32` bits; `NAN` until set.
    value: AtomicU32,
    dirty: Dirty,
}

builders!(Sensor);

impl Sensor {
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self { meta: Meta::new(Kind::Sensor, id, name), value: AtomicU32::new(f32::NAN.to_bits()), dirty: Dirty::new() }
    }
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.meta.unit = Some(unit);
        self
    }
    /// A measurement: Home Assistant keeps its statistics.
    pub const fn measurement(mut self) -> Self {
        self.meta.measurement = true;
        self
    }
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
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self { meta: Meta::new(Kind::BinarySensor, id, name), value: AtomicU8::new(UNKNOWN), dirty: Dirty::new() }
    }
    pub fn set(&self, on: bool) {
        if self.value.swap(u8::from(on), Ordering::AcqRel) != u8::from(on) {
            self.dirty.mark();
        }
    }
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

impl Switch {
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
    pub on: bool,
    /// 0-255.
    pub brightness: u8,
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

impl Light {
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            meta: Meta::new(Kind::Light, id, name),
            state: Mutex::new(Cell::new(None)),
            pending: Mutex::new(Cell::new(None)),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    pub fn set(&self, state: LightState) {
        self.pending.lock(|p| p.set(None));
        if self.state.lock(|s| s.replace(Some(state))) != Some(state) {
            self.dirty.mark();
        }
    }
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
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self { meta: Meta::new(Kind::Button, id, name), presses: Signal::new(), dirty: Dirty::new() }
    }
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
