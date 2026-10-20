//! Settings and readings beyond on/off and numbers: a number set from the controller, a choice
//! among options, a text reading, an event.

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};
use heapless::String;

use crate::{
    Binding, Command, Dirty, Entity, Kind, Meta, Spec, State,
    atomic::{AtomicU8, AtomicU32, Ordering},
    entities::builders,
};

/// Longest text of a [`TextSensor`], in bytes.
pub const TEXT_LEN: usize = 64;

/// No value (for the `u8` atomics).
const NONE: u8 = u8::MAX;

/// A value Home Assistant sets (a setpoint, a duration, a level), within a range. The app applies
/// commands and reports the value with `set`.
pub struct Number {
    meta: Meta,
    min: f32,
    max: f32,
    step: f32,
    slider: bool,
    /// `f32` bits; `NAN` until set.
    value: AtomicU32,
    /// The last command until the app reports a value; `NAN` for none.
    pending: AtomicU32,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, f32>,
}

builders!(Number);

impl Number {
    /// From `min` to `max`, in steps of `step` (shown as a box; see [`slider`](Self::slider)).
    pub const fn new(id: &'static str, name: &'static str, min: f32, max: f32, step: f32) -> Self {
        assert!(min < max && step > 0.0, "Number: needs min < max and step > 0");
        Self {
            meta: Meta::new(Kind::Number, id, name),
            min,
            max,
            step,
            slider: false,
            value: AtomicU32::new(f32::NAN.to_bits()),
            pending: AtomicU32::new(f32::NAN.to_bits()),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    /// Unit of the value (`°C`, `%`, ...).
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.meta.unit = Some(unit);
        self
    }
    /// Shown as a slider.
    pub const fn slider(mut self) -> Self {
        self.slider = true;
        self
    }
    /// Reports the value; `NAN` makes it unknown.
    pub fn set(&self, value: f32) {
        self.pending.store(f32::NAN.to_bits(), Ordering::Release);
        if self.value.swap(value.to_bits(), Ordering::AcqRel) != value.to_bits() {
            self.dirty.mark();
        }
    }
    /// The last reported value, `None` until set.
    pub fn get(&self) -> Option<f32> {
        let v = f32::from_bits(self.value.load(Ordering::Acquire));
        (!v.is_nan()).then_some(v)
    }
    /// The next value asked for (the latest one, if several came meanwhile).
    pub async fn wait_command(&self) -> f32 {
        self.commands.wait().await
    }
    /// `value` within the range, on a step.
    fn snap(&self, value: f32) -> f32 {
        let v = value.clamp(self.min, self.max);
        // No `round` without std: the offset from `min` is never negative.
        let steps = ((v - self.min) / self.step + 0.5) as u32;
        (self.min + steps as f32 * self.step).min(self.max)
    }
}

impl Entity for Number {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Number)
    }
    fn target(&self) -> State {
        let pending = f32::from_bits(self.pending.load(Ordering::Acquire));
        if pending.is_nan() { self.state() } else { State::Number(pending) }
    }
    fn spec(&self) -> Spec {
        Spec::Number { min: self.min, max: self.max, step: self.step, slider: self.slider }
    }
    fn command(&self, command: Command) -> bool {
        match command {
            Command::Number(v) if v.is_finite() => {
                let v = self.snap(v);
                self.pending.store(v.to_bits(), Ordering::Release);
                self.commands.signal(v);
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

/// One of a fixed list of options (a mode, a preset). The app applies commands and reports the
/// option with `set`. Options are given by index into the list.
pub struct Select {
    meta: Meta,
    options: &'static [&'static str],
    value: AtomicU8,
    pending: AtomicU8,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, u8>,
}

builders!(Select);

impl Select {
    /// A new entity (`id`: see [`Meta::id`]) with 1 to 254 `options` (checked at build time).
    pub const fn new(id: &'static str, name: &'static str, options: &'static [&'static str]) -> Self {
        assert!(!options.is_empty() && options.len() < NONE as usize, "Select: 1 to 254 options");
        Self {
            meta: Meta::new(Kind::Select, id, name),
            options,
            value: AtomicU8::new(NONE),
            pending: AtomicU8::new(NONE),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    /// Reports the option (an index into the list; out of range makes it unknown).
    pub fn set(&self, index: u8) {
        let index = if usize::from(index) < self.options.len() { index } else { NONE };
        self.pending.store(NONE, Ordering::Release);
        if self.value.swap(index, Ordering::AcqRel) != index {
            self.dirty.mark();
        }
    }
    /// The last reported option, `None` until set.
    pub fn get(&self) -> Option<u8> {
        match self.value.load(Ordering::Acquire) {
            NONE => None,
            i => Some(i),
        }
    }
    /// The option's text.
    pub fn option(&self) -> Option<&'static str> {
        self.get().and_then(|i| self.options.get(usize::from(i)).copied())
    }
    /// The next option asked for, as an index.
    pub async fn wait_command(&self) -> u8 {
        self.commands.wait().await
    }
}

impl Entity for Select {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Option)
    }
    fn target(&self) -> State {
        match self.pending.load(Ordering::Acquire) {
            NONE => self.state(),
            i => State::Option(i),
        }
    }
    fn spec(&self) -> Spec {
        Spec::Select { options: self.options }
    }
    fn command(&self, command: Command) -> bool {
        match command {
            Command::Option(i) if usize::from(i) < self.options.len() => {
                self.pending.store(i, Ordering::Release);
                self.commands.signal(i);
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

/// A text reading (a status, a firmware name, the last error), up to [`TEXT_LEN`] bytes.
pub struct TextSensor {
    meta: Meta,
    value: Mutex<CriticalSectionRawMutex, RefCell<Option<String<TEXT_LEN>>>>,
    dirty: Dirty,
}

builders!(TextSensor);

impl TextSensor {
    /// A new entity (`id`: see [`Meta::id`]).
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self { meta: Meta::new(Kind::TextSensor, id, name), value: Mutex::new(RefCell::new(None)), dirty: Dirty::new() }
    }
    /// Sets the text, cut at [`TEXT_LEN`] bytes (on a character boundary).
    pub fn set(&self, text: &str) {
        let mut end = text.len().min(TEXT_LEN);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let new = String::try_from(&text[..end]).ok();
        let changed = self.value.lock(|v| {
            let mut v = v.borrow_mut();
            let changed = *v != new;
            *v = new;
            changed
        });
        if changed {
            self.dirty.mark();
        }
    }
}

impl Entity for TextSensor {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        if self.value.lock(|v| v.borrow().is_some()) { State::Text } else { State::Unknown }
    }
    fn text(&self) -> Option<String<TEXT_LEN>> {
        self.value.lock(|v| v.borrow().clone())
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

/// Something that happened: a button pressed, double-pressed, held. Not a state: each
/// [`trigger`](Self::trigger) is sent once, and nothing is sent again after a reconnection.
pub struct Event {
    meta: Meta,
    types: &'static [&'static str],
    last: AtomicU8,
    dirty: Dirty,
}

builders!(Event);

impl Event {
    /// `types`: the event types it can trigger, e.g. `&["press", "double_press", "long_press"]`.
    pub const fn new(id: &'static str, name: &'static str, types: &'static [&'static str]) -> Self {
        assert!(!types.is_empty() && types.len() < NONE as usize, "Event: 1 to 254 event types");
        Self { meta: Meta::new(Kind::Event, id, name), types, last: AtomicU8::new(NONE), dirty: Dirty::new() }
    }
    /// Triggers event type `index` (into the list given to `new`).
    pub fn trigger(&self, index: u8) {
        if usize::from(index) < self.types.len() {
            self.last.store(index, Ordering::Release);
            self.dirty.mark();
        }
    }
}

impl Entity for Event {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        match self.last.load(Ordering::Acquire) {
            NONE => State::Unknown,
            i => State::Event(i),
        }
    }
    fn spec(&self) -> Spec {
        Spec::Event { types: self.types }
    }
    fn command(&self, _: Command) -> bool {
        false
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    /// Events are not replayed (after a reconnection, say).
    fn mark_dirty(&self, _: Binding) {}
}
