//! Appliances: covers and valves, locks, fans, thermostats. The app applies commands and reports
//! the result with `set`.

use core::cell::Cell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};

use crate::{Binding, Command, Dirty, Entity, Kind, Meta, Spec, State, entities::builders};

/// Where a cover or valve is, or where it is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverState {
    Open,
    Closed,
    Opening,
    Closing,
    /// Stopped between open and closed.
    Stopped,
}

impl CoverState {
    pub fn as_str(self) -> &'static str {
        // A table in flash: a `match` would become a lookup table, which esp-hal keeps in RAM.
        static NAMES: [&str; 5] = ["open", "closed", "opening", "closing", "stopped"];
        NAMES[self as usize]
    }
}

/// What a controller asks a cover or valve to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverCommand {
    Open,
    Close,
    Stop,
    /// Go to this position: 0 closed, 100 open (only with `position()`).
    Position(u8),
}

/// Where a cover or valve is going, and its position.
type Place = (Option<CoverState>, Option<u8>);

/// The cover and valve body.
struct Motion {
    position: bool,
    state: Mutex<CriticalSectionRawMutex, Cell<Place>>,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, CoverCommand>,
}

impl Motion {
    const fn new() -> Self {
        Self {
            position: false,
            state: Mutex::new(Cell::new((None, None))),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }

    fn set(&self, state: Option<CoverState>, position: Option<u8>) {
        let next = (state, position.map(|p| p.min(100)));
        if self.state.lock(|s| s.replace(next)) != next {
            self.dirty.mark();
        }
    }

    fn get(&self) -> (Option<CoverState>, Option<u8>) {
        self.state.lock(Cell::get)
    }

    fn state(&self) -> State {
        match self.get() {
            (None, None) => State::Unknown,
            (state, position) => State::Cover { state, position },
        }
    }

    fn command(&self, command: Command) -> bool {
        match command {
            Command::Cover(CoverCommand::Position(p)) if !self.position || p > 100 => false,
            Command::Cover(c) => {
                self.commands.signal(c);
                true
            }
            _ => false,
        }
    }
}

macro_rules! motion {
    ($ty:ident, $kind:expr, $doc:literal) => {
        #[doc = $doc]
        pub struct $ty {
            meta: Meta,
            motion: Motion,
        }

        builders!($ty);

        impl $ty {
            pub const fn new(id: &'static str, name: &'static str) -> Self {
                Self { meta: Meta::new($kind, id, name), motion: Motion::new() }
            }
            /// Takes a position (0 closed, 100 open), and reports it.
            pub const fn position(mut self) -> Self {
                self.motion.position = true;
                self
            }
            /// Reports where it is or what it is doing (`position`: 0 closed, 100 open, if known).
            pub fn set(&self, state: Option<CoverState>, position: Option<u8>) {
                self.motion.set(state, position)
            }
            pub fn get(&self) -> (Option<CoverState>, Option<u8>) {
                self.motion.get()
            }
            /// The next command.
            pub async fn wait_command(&self) -> CoverCommand {
                self.motion.commands.wait().await
            }
        }

        impl Entity for $ty {
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn state(&self) -> State {
                self.motion.state()
            }
            fn spec(&self) -> Spec {
                Spec::Cover { position: self.motion.position }
            }
            fn command(&self, command: Command) -> bool {
                self.motion.command(command)
            }
            fn take_dirty(&self, binding: Binding) -> bool {
                self.motion.dirty.take(binding)
            }
            fn mark_dirty(&self, binding: Binding) {
                self.motion.dirty.mark_for(binding)
            }
        }
    };
}

motion!(Cover, Kind::Cover, "A blind, shade, curtain, garage door or gate (device class `blind`, `garage`, ...).");
motion!(Valve, Kind::Valve, "A water or gas valve (device class `water`, `gas`).");

/// A lock's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockState {
    Locked,
    Unlocked,
    Locking,
    Unlocking,
    Jammed,
    /// Unlatched (only with `open()`).
    Open,
    Opening,
}

impl LockState {
    pub fn as_str(self) -> &'static str {
        // A table in flash: a `match` would become a lookup table, which esp-hal keeps in RAM.
        static NAMES: [&str; 7] = ["LOCKED", "UNLOCKED", "LOCKING", "UNLOCKING", "JAMMED", "OPEN", "OPENING"];
        NAMES[self as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockCommand {
    Lock,
    Unlock,
    /// Unlatch (only with `open()`).
    Open,
}

/// A door lock.
pub struct Lock {
    meta: Meta,
    open: bool,
    state: Mutex<CriticalSectionRawMutex, Cell<Option<LockState>>>,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, LockCommand>,
}

builders!(Lock);

impl Lock {
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            meta: Meta::new(Kind::Lock, id, name),
            open: false,
            state: Mutex::new(Cell::new(None)),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    /// It can also unlatch the door ([`LockCommand::Open`]).
    pub const fn open(mut self) -> Self {
        self.open = true;
        self
    }
    pub fn set(&self, state: LockState) {
        if self.state.lock(|s| s.replace(Some(state))) != Some(state) {
            self.dirty.mark();
        }
    }
    pub fn get(&self) -> Option<LockState> {
        self.state.lock(Cell::get)
    }
    pub async fn wait_command(&self) -> LockCommand {
        self.commands.wait().await
    }
}

impl Entity for Lock {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Lock)
    }
    fn spec(&self) -> Spec {
        Spec::Lock { open: self.open }
    }
    fn command(&self, command: Command) -> bool {
        match command {
            Command::Lock(LockCommand::Open) if !self.open => false,
            Command::Lock(c) => {
                self.commands.signal(c);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanDirection {
    Forward,
    Reverse,
}

impl FanDirection {
    pub fn as_str(self) -> &'static str {
        // A table in flash: a `match` would become a lookup table, which esp-hal keeps in RAM.
        static NAMES: [&str; 2] = ["forward", "reverse"];
        NAMES[self as usize]
    }
}

/// A fan's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FanState {
    pub on: bool,
    /// 1-100 (with `speed()`).
    pub percentage: u8,
    /// Index into the presets, if one is active.
    pub preset: Option<u8>,
    pub oscillating: bool,
    pub direction: FanDirection,
}

impl FanState {
    pub const OFF: Self =
        Self { on: false, percentage: 100, preset: None, oscillating: false, direction: FanDirection::Forward };

    /// `self` with one setting changed.
    pub fn apply(self, command: FanCommand) -> Self {
        let mut next = self;
        match command {
            FanCommand::On(on) => next.on = on,
            FanCommand::Percentage(0) => next.on = false,
            FanCommand::Percentage(p) => {
                next.on = true;
                next.percentage = p.min(100);
                next.preset = None;
            }
            FanCommand::Preset(p) => {
                next.on = true;
                next.preset = Some(p);
            }
            FanCommand::Oscillate(o) => next.oscillating = o,
            FanCommand::Direction(d) => next.direction = d,
        }
        next
    }
}

/// One fan setting to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanCommand {
    On(bool),
    /// 0 turns it off.
    Percentage(u8),
    /// Index into the presets.
    Preset(u8),
    Oscillate(bool),
    Direction(FanDirection),
}

/// A fan: on/off, and optionally speed, presets, oscillation, direction.
pub struct Fan {
    meta: Meta,
    speed: bool,
    presets: &'static [&'static str],
    oscillation: bool,
    direction: bool,
    state: Mutex<CriticalSectionRawMutex, Cell<Option<FanState>>>,
    pending: Mutex<CriticalSectionRawMutex, Cell<Option<FanState>>>,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, FanState>,
}

builders!(Fan);

impl Fan {
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            meta: Meta::new(Kind::Fan, id, name),
            speed: false,
            presets: &[],
            oscillation: false,
            direction: false,
            state: Mutex::new(Cell::new(None)),
            pending: Mutex::new(Cell::new(None)),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    /// Takes a speed (percentage).
    pub const fn speed(mut self) -> Self {
        self.speed = true;
        self
    }
    pub const fn presets(mut self, presets: &'static [&'static str]) -> Self {
        assert!(presets.len() < u8::MAX as usize, "Fan: at most 254 presets");
        // Room for the longest one in the state published over MQTT.
        let mut i = 0;
        while i < presets.len() {
            assert!(presets[i].len() <= 24, "Fan: presets of 24 bytes at most");
            i += 1;
        }
        self.presets = presets;
        self
    }
    pub const fn oscillation(mut self) -> Self {
        self.oscillation = true;
        self
    }
    pub const fn direction(mut self) -> Self {
        self.direction = true;
        self
    }
    pub fn set(&self, state: FanState) {
        self.pending.lock(|p| p.set(None));
        if self.state.lock(|s| s.replace(Some(state))) != Some(state) {
            self.dirty.mark();
        }
    }
    pub fn get(&self) -> Option<FanState> {
        self.state.lock(Cell::get)
    }
    /// The state a command asks for (the current one with the command's change).
    pub async fn wait_command(&self) -> FanState {
        self.commands.wait().await
    }
    fn target_state(&self) -> FanState {
        self.pending.lock(Cell::get).or_else(|| self.get()).unwrap_or(FanState::OFF)
    }
}

impl Entity for Fan {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Fan)
    }
    fn target(&self) -> State {
        match self.pending.lock(Cell::get) {
            Some(p) => State::Fan(p),
            None => self.state(),
        }
    }
    fn spec(&self) -> Spec {
        Spec::Fan { speed: self.speed, presets: self.presets, oscillation: self.oscillation, direction: self.direction }
    }
    fn command(&self, command: Command) -> bool {
        let Command::Fan(c) = command else { return false };
        let supported = match c {
            FanCommand::On(_) => true,
            FanCommand::Percentage(_) => self.speed,
            FanCommand::Preset(i) => usize::from(i) < self.presets.len(),
            FanCommand::Oscillate(_) => self.oscillation,
            FanCommand::Direction(_) => self.direction,
        };
        if supported {
            let next = self.target_state().apply(c);
            self.pending.lock(|p| p.set(Some(next)));
            self.commands.signal(next);
        }
        supported
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}

/// A thermostat's operating mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HvacMode {
    Off,
    Heat,
    Cool,
    /// Heat or cool to stay at the target.
    HeatCool,
    Auto,
    Dry,
    FanOnly,
}

impl HvacMode {
    pub const ALL: [HvacMode; 7] =
        [Self::Off, Self::Heat, Self::Cool, Self::HeatCool, Self::Auto, Self::Dry, Self::FanOnly];

    pub fn as_str(self) -> &'static str {
        // A table in flash: a `match` would become a lookup table, which esp-hal keeps in RAM.
        static NAMES: [&str; 7] = ["off", "heat", "cool", "heat_cool", "auto", "dry", "fan_only"];
        NAMES[self as usize]
    }
}

/// What a thermostat is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HvacAction {
    Off,
    Heating,
    Cooling,
    Drying,
    Idle,
    Fan,
}

impl HvacAction {
    pub fn as_str(self) -> &'static str {
        // A table in flash: a `match` would become a lookup table, which esp-hal keeps in RAM.
        static NAMES: [&str; 6] = ["off", "heating", "cooling", "drying", "idle", "fan"];
        NAMES[self as usize]
    }
}

/// A thermostat's state (temperatures in °C).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClimateState {
    pub mode: HvacMode,
    pub target: f32,
    /// The measured temperature.
    pub current: Option<f32>,
    pub action: Option<HvacAction>,
}

/// One thermostat setting to change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClimateCommand {
    /// One of the thermostat's modes.
    Mode(HvacMode),
    /// The target temperature (°C).
    Target(f32),
}

/// A thermostat: modes, a target temperature, the measured one and what it is doing.
pub struct Climate {
    meta: Meta,
    modes: &'static [HvacMode],
    min: f32,
    max: f32,
    step: f32,
    state: Mutex<CriticalSectionRawMutex, Cell<Option<ClimateState>>>,
    pending: Mutex<CriticalSectionRawMutex, Cell<Option<ClimateState>>>,
    dirty: Dirty,
    commands: Signal<CriticalSectionRawMutex, ClimateState>,
}

builders!(Climate);

impl Climate {
    /// `modes`: those it supports, the first being the default. Targets from 7 to 35 °C in
    /// steps of 0.5 (see [`range`](Self::range)).
    pub const fn new(id: &'static str, name: &'static str, modes: &'static [HvacMode]) -> Self {
        assert!(!modes.is_empty(), "Climate: needs at least one mode");
        Self {
            meta: Meta::new(Kind::Climate, id, name),
            modes,
            min: 7.0,
            max: 35.0,
            step: 0.5,
            state: Mutex::new(Cell::new(None)),
            pending: Mutex::new(Cell::new(None)),
            dirty: Dirty::new(),
            commands: Signal::new(),
        }
    }
    /// Target temperatures from `min` to `max` °C, in steps of `step` (0.1, 0.5 or 1).
    pub const fn range(mut self, min: f32, max: f32, step: f32) -> Self {
        assert!(min < max, "Climate: needs min < max");
        assert!(step == 0.1 || step == 0.5 || step == 1.0, "Climate: step is 0.1, 0.5 or 1");
        self.min = min;
        self.max = max;
        self.step = step;
        self
    }
    pub fn set(&self, state: ClimateState) {
        self.pending.lock(|p| p.set(None));
        if self.state.lock(|s| s.replace(Some(state))) != Some(state) {
            self.dirty.mark();
        }
    }
    pub fn get(&self) -> Option<ClimateState> {
        self.state.lock(Cell::get)
    }
    /// The state a command asks for (the current one with the command's change).
    pub async fn wait_command(&self) -> ClimateState {
        self.commands.wait().await
    }
    fn target_state(&self) -> ClimateState {
        self.pending.lock(Cell::get).or_else(|| self.get()).unwrap_or(ClimateState {
            mode: self.modes[0],
            target: 21f32.clamp(self.min, self.max),
            current: None,
            action: None,
        })
    }
    fn snap(&self, t: f32) -> f32 {
        let t = t.clamp(self.min, self.max);
        let steps = ((t - self.min) / self.step + 0.5) as u32;
        (self.min + steps as f32 * self.step).min(self.max)
    }
}

impl Entity for Climate {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        self.get().map_or(State::Unknown, State::Climate)
    }
    fn target(&self) -> State {
        match self.pending.lock(Cell::get) {
            Some(p) => State::Climate(p),
            None => self.state(),
        }
    }
    fn spec(&self) -> Spec {
        Spec::Climate { modes: self.modes, min: self.min, max: self.max, step: self.step }
    }
    fn command(&self, command: Command) -> bool {
        let mut next = self.target_state();
        match command {
            Command::Climate(ClimateCommand::Mode(m)) if self.modes.contains(&m) => next.mode = m,
            Command::Climate(ClimateCommand::Target(t)) if t.is_finite() => next.target = self.snap(t),
            _ => return false,
        }
        self.pending.lock(|p| p.set(Some(next)));
        self.commands.signal(next);
        true
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}
