//! The ESPHome API's messages for the device model: device info, the entity list, states and
//! commands. Message ids and fields from ESPHome's `api.proto`, API version [`VERSION`].

use core::fmt::Write as _;

use crate::esphome::proto::{Fields, Value, Writer};
use crate::model::{
    Category, ClimateCommand, Command, CoverCommand, CoverState, Device, Entity, FanCommand, FanDirection, HvacAction,
    HvacMode, Kind, LockCommand, LockState, Spec, State,
};

/// The API version spoken (major, minor). 1.14: climate feature flags; before 1.15's device
/// capabilities message, which clients do not ask a 1.14 device for.
pub const VERSION: (u32, u32) = (1, 14);

/// Message ids.
pub mod id {
    #![allow(missing_docs)]
    pub const HELLO_REQUEST: u16 = 1;
    pub const HELLO_RESPONSE: u16 = 2;
    pub const DISCONNECT_REQUEST: u16 = 5;
    pub const DISCONNECT_RESPONSE: u16 = 6;
    pub const PING_REQUEST: u16 = 7;
    pub const PING_RESPONSE: u16 = 8;
    pub const DEVICE_INFO_REQUEST: u16 = 9;
    pub const DEVICE_INFO_RESPONSE: u16 = 10;
    pub const LIST_ENTITIES_REQUEST: u16 = 11;
    pub const LIST_ENTITIES_DONE: u16 = 19;
    pub const SUBSCRIBE_STATES_REQUEST: u16 = 20;
}

/// The entity key in messages: the FNV-1a hash of its id, the same at every boot.
pub fn key(entity: &dyn Entity) -> u32 {
    crate::mqtt::ha::hash(entity.meta().id.as_bytes(), 0x811c_9dc5)
}

/// HelloResponse.
pub fn hello(w: &mut Writer, name: &str) -> u16 {
    w.uint(1, VERSION.0);
    w.uint(2, VERSION.1);
    w.str(3, concat!("iotkit ", env!("CARGO_PKG_VERSION")));
    w.str(4, name);
    id::HELLO_RESPONSE
}

/// DeviceInfoResponse; `mac` as `AA:BB:CC:DD:EE:FF`.
pub fn device_info(w: &mut Writer, device: &Device, mac: &str) -> u16 {
    w.str(2, device.name);
    w.str(3, mac);
    // Shown as the ESPHome version: iotkit's.
    w.str(4, env!("CARGO_PKG_VERSION"));
    w.str(6, device.model);
    // The app as an ESPHome "project", `maker.model`: Home Assistant shows the two parts as the
    // device's manufacturer and model (it requires the dot), and the firmware version.
    let mut project = heapless::String::<96>::new();
    let _ = write!(project, "{}.{}", device.manufacturer, device.model);
    w.str(8, &project);
    w.str(9, device.sw_version);
    w.str(12, device.manufacturer);
    w.str(13, device.name);
    id::DEVICE_INFO_RESPONSE
}

/// Field numbers that differ between the ListEntities messages.
struct Info {
    id: u16,
    icon: u32,
    category: u32,
    device_class: Option<u32>,
}

const fn info(id: u16, icon: u32, category: u32, device_class: Option<u32>) -> Info {
    Info { id, icon, category, device_class }
}

/// The ListEntities message describing `entity`, and its id.
pub fn list_entity(w: &mut Writer, entity: &dyn Entity) -> u16 {
    let meta = entity.meta();
    let spec = entity.spec();
    let info = match meta.kind {
        Kind::BinarySensor => info(12, 8, 9, Some(5)),
        Kind::Cover => info(13, 10, 11, Some(8)),
        Kind::Fan => info(14, 10, 11, None),
        Kind::Light => info(15, 14, 15, None),
        Kind::Sensor => info(16, 5, 13, Some(9)),
        Kind::Switch => info(17, 5, 8, Some(9)),
        Kind::TextSensor => info(18, 5, 7, Some(8)),
        Kind::Climate => info(46, 19, 20, None),
        Kind::Number => info(49, 5, 10, Some(13)),
        Kind::Select => info(52, 5, 8, None),
        Kind::Lock => info(58, 5, 7, None),
        Kind::Button => info(61, 5, 7, Some(8)),
        Kind::Event => info(107, 5, 7, Some(8)),
        Kind::Valve => info(109, 5, 7, Some(8)),
        Kind::Update => info(116, 5, 7, Some(8)),
    };
    w.str(1, meta.id);
    w.fixed32(2, key(entity));
    w.str(3, meta.name);
    w.str(info.icon, meta.icon.unwrap_or(""));
    w.uint(info.category, category(meta.category));
    match (info.device_class, meta.device_class) {
        (Some(field), Some(class)) => w.str(field, class.as_str()),
        // The firmware entity: shown as such.
        (Some(field), None) if meta.kind == Kind::Update => w.str(field, "firmware"),
        _ => {}
    }
    match (meta.kind, spec) {
        (Kind::Sensor, _) => {
            w.str(6, meta.unit.map_or("", |u| u.as_str()));
            w.int(7, meta.precision.map_or(1, i32::from));
            w.uint(10, meta.measurement.into());
        }
        (Kind::Light, _) => w.item(12, COLOR_MODE_RGB),
        (Kind::Number, Spec::Number { min, max, step, slider }) => {
            w.float(6, min);
            w.float(7, max);
            w.float(8, step);
            w.str(11, meta.unit.map_or("", |u| u.as_str()));
            w.uint(12, if slider { 2 } else { 1 });
        }
        (Kind::Select, Spec::Select { options }) => options.iter().for_each(|o| w.text(6, o)),
        (Kind::Event, Spec::Event { types }) => types.iter().for_each(|t| w.text(9, t)),
        (Kind::Cover, Spec::Cover { position }) => {
            w.bool(6, position);
            w.bool(12, true);
        }
        (Kind::Valve, Spec::Cover { position }) => {
            w.bool(10, position);
            w.bool(11, true);
        }
        (Kind::Lock, Spec::Lock { open }) => w.bool(9, open),
        (Kind::Fan, Spec::Fan { speed, presets, oscillation, direction }) => {
            w.bool(5, oscillation);
            w.bool(6, speed);
            w.bool(7, direction);
            // Speed as a percentage.
            w.int(8, if speed { 100 } else { 0 });
            presets.iter().for_each(|p| w.text(12, p));
        }
        (Kind::Climate, Spec::Climate { modes, min, max, step }) => {
            modes.iter().for_each(|&m| w.item(7, climate_mode(m)));
            w.float(8, min);
            w.float(9, max);
            w.float(10, step);
            // Current temperature, action.
            w.uint(27, 1 | 1 << 5);
        }
        _ => {}
    }
    info.id
}

const COLOR_MODE_RGB: u32 = 35;

fn category(category: Category) -> u32 {
    match category {
        Category::Primary => 0,
        Category::Config => 1,
        Category::Diagnostic => 2,
    }
}

fn climate_mode(mode: HvacMode) -> u32 {
    match mode {
        HvacMode::Off => 0,
        HvacMode::HeatCool => 1,
        HvacMode::Cool => 2,
        HvacMode::Heat => 3,
        HvacMode::FanOnly => 4,
        HvacMode::Dry => 5,
        HvacMode::Auto => 6,
    }
}

/// The state message for `entity` and its id; `None` when it has none to send (buttons, a
/// light never set).
pub fn state(w: &mut Writer, entity: &dyn Entity) -> Option<u16> {
    let spec = entity.spec();
    w.fixed32(1, key(entity));
    let unknown = entity.state() == State::Unknown;
    let id = match (entity.meta().kind, entity.state()) {
        (Kind::Sensor, s) => {
            w.float(2, number(s));
            w.bool(3, unknown);
            25
        }
        (Kind::Number, s) => {
            w.float(2, number(s));
            w.bool(3, unknown);
            50
        }
        (Kind::BinarySensor, s) => {
            w.bool(2, s == State::Bool(true));
            w.bool(3, unknown);
            21
        }
        (Kind::Switch, s) => {
            w.bool(2, s == State::Bool(true));
            w.bool(4, unknown);
            26
        }
        (Kind::TextSensor, _) => {
            let text = entity.text();
            w.str(2, text.as_deref().unwrap_or(""));
            w.bool(3, text.is_none());
            27
        }
        (Kind::Select, s) => {
            let option = match (s, spec) {
                (State::Option(i), Spec::Select { options }) => options.get(usize::from(i)).copied(),
                _ => None,
            };
            w.str(2, option.unwrap_or(""));
            w.bool(3, option.is_none());
            53
        }
        (Kind::Light, State::Light(l)) => {
            w.bool(2, l.on);
            w.float(3, f32::from(l.brightness) / 255.0);
            w.uint(11, COLOR_MODE_RGB);
            w.float(10, 1.0);
            for (field, c) in (4..).zip(l.rgb) {
                w.float(field, f32::from(c) / 255.0);
            }
            24
        }
        (Kind::Cover, State::Cover { state, position }) => {
            w.float(3, cover_position(state, position));
            w.uint(5, operation(state));
            22
        }
        (Kind::Valve, State::Cover { state, position }) => {
            w.float(2, cover_position(state, position));
            w.uint(3, operation(state));
            110
        }
        (Kind::Lock, State::Lock(l)) => {
            w.uint(
                2,
                match l {
                    LockState::Locked => 1,
                    LockState::Unlocked => 2,
                    LockState::Jammed => 3,
                    LockState::Locking => 4,
                    LockState::Unlocking => 5,
                    LockState::Opening => 6,
                    LockState::Open => 7,
                },
            );
            59
        }
        (Kind::Fan, State::Fan(f)) => {
            w.bool(2, f.on);
            w.bool(3, f.oscillating);
            w.uint(5, (f.direction == FanDirection::Reverse).into());
            w.int(6, f.percentage.into());
            if let (Some(i), Spec::Fan { presets, .. }) = (f.preset, spec) {
                w.str(7, presets.get(usize::from(i)).copied().unwrap_or(""));
            }
            23
        }
        (Kind::Climate, State::Climate(c)) => {
            w.uint(2, climate_mode(c.mode));
            w.float(3, c.current.unwrap_or(f32::NAN));
            w.float(4, c.target);
            w.uint(
                8,
                match c.action {
                    None | Some(HvacAction::Off) => 0,
                    Some(HvacAction::Cooling) => 2,
                    Some(HvacAction::Heating) => 3,
                    Some(HvacAction::Idle) => 4,
                    Some(HvacAction::Drying) => 5,
                    Some(HvacAction::Fan) => 6,
                },
            );
            47
        }
        (Kind::Event, State::Event(i)) => {
            let Spec::Event { types } = spec else { return None };
            w.str(2, types.get(usize::from(i))?);
            108
        }
        (Kind::Update, _) => {
            let Some(u) = entity.update() else {
                w.bool(2, true);
                return Some(117);
            };
            w.bool(3, u.in_progress);
            w.bool(4, u.percentage.is_some());
            w.float(5, u.percentage.map_or(0.0, f32::from));
            w.str(6, &u.installed);
            w.str(7, &u.latest);
            w.str(8, entity.meta().name);
            w.str(9, &u.summary);
            w.str(10, &u.release_url);
            117
        }
        _ => return None,
    };
    Some(id)
}

fn number(state: State) -> f32 {
    match state {
        State::Number(v) => v,
        _ => f32::NAN,
    }
}

/// 0 closed to 1 open; without a position, open unless closed.
fn cover_position(state: Option<CoverState>, position: Option<u8>) -> f32 {
    match (position, state) {
        (Some(p), _) => f32::from(p) / 100.0,
        (None, Some(CoverState::Closed)) => 0.0,
        (None, _) => 1.0,
    }
}

fn operation(state: Option<CoverState>) -> u32 {
    match state {
        Some(CoverState::Opening) => 1,
        Some(CoverState::Closing) => 2,
        _ => 0,
    }
}

/// Hands the commands a command message `body` (of message `id`) carries to `apply`, with
/// their entity, in order (a fan or thermostat command can change several settings), and
/// returns the entity. `None` for another message, or an entity the device does not have.
pub fn command<'d>(
    device: &'d Device,
    id: u16,
    body: &[u8],
    mut apply: impl FnMut(&'d dyn Entity, Command),
) -> Option<&'d dyn Entity> {
    let kind = match id {
        30 => Kind::Cover,
        31 => Kind::Fan,
        32 => Kind::Light,
        33 => Kind::Switch,
        48 => Kind::Climate,
        51 => Kind::Number,
        54 => Kind::Select,
        60 => Kind::Lock,
        62 => Kind::Button,
        111 => Kind::Valve,
        118 => Kind::Update,
        _ => return None,
    };
    let wanted = Fields::new(body).find(|&(f, _)| f == 1)?.1.fixed32();
    let entity = device.entities.iter().copied().find(|e| e.meta().kind == kind && key(*e) == wanted)?;
    let mut apply = |command| apply(entity, command);
    // The `has_` flags come before their values, as their field numbers do.
    let get = |field| Fields::new(body).find(|&(f, _)| f == field).map(|(_, v)| v);
    let has = |field| get(field).is_some_and(Value::bool);
    // Values at their default (0) are left out of the message.
    let float = |field| get(field).map_or(0.0, Value::float);
    let percent = |field| (float(field).clamp(0.0, 1.0) * 100.0 + 0.5) as u8;
    let byte = |field| (float(field).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let index = |v: Option<Value>, names: &[&str]| {
        let wanted = v.map_or("", Value::str);
        names.iter().position(|&n| n == wanted).and_then(|i| u8::try_from(i).ok())
    };
    let cover = |stop: u32, has_position: u32, position: u32| {
        if has(stop) {
            return Some(CoverCommand::Stop);
        }
        if !has(has_position) {
            return None;
        }
        let p = percent(position);
        Some(match p {
            0 => CoverCommand::Close,
            100 => CoverCommand::Open,
            _ => CoverCommand::Position(p),
        })
    };
    match (kind, entity.spec()) {
        (Kind::Switch, _) => apply(Command::Set(get(2).is_some_and(Value::bool))),
        (Kind::Button, _) => apply(Command::Press),
        (Kind::Number, _) => apply(Command::Number(float(2))),
        (Kind::Select, Spec::Select { options }) => apply(Command::Option(index(get(2), options)?)),
        (Kind::Light, _) => apply(Command::Light {
            on: has(2).then(|| has(3)),
            brightness: has(4).then(|| byte(5)),
            rgb: has(6).then(|| [byte(7), byte(8), byte(9)]),
        }),
        (Kind::Cover, _) => apply(Command::Cover(cover(8, 4, 5)?)),
        (Kind::Valve, _) => apply(Command::Cover(cover(4, 2, 3)?)),
        (Kind::Lock, _) => apply(Command::Lock(match get(2).map_or(0, Value::uint) {
            0 => LockCommand::Unlock,
            1 => LockCommand::Lock,
            2 => LockCommand::Open,
            _ => return None,
        })),
        (Kind::Fan, Spec::Fan { presets, .. }) => {
            if has(2) {
                apply(Command::Fan(FanCommand::On(has(3))));
            }
            if has(10) {
                let level = get(11).map_or(0, Value::uint).min(100);
                apply(Command::Fan(FanCommand::Percentage(level as u8)));
            }
            if has(6) {
                apply(Command::Fan(FanCommand::Oscillate(has(7))));
            }
            if has(8) {
                let reverse = get(9).is_some_and(Value::bool);
                apply(Command::Fan(FanCommand::Direction(if reverse {
                    FanDirection::Reverse
                } else {
                    FanDirection::Forward
                })));
            }
            if has(12)
                && let Some(i) = index(get(13), presets)
            {
                apply(Command::Fan(FanCommand::Preset(i)));
            }
        }
        (Kind::Climate, _) => {
            if has(2) {
                let mode = get(3).map_or(0, Value::uint);
                let mode = HvacMode::ALL.into_iter().find(|&m| climate_mode(m) == mode)?;
                apply(Command::Climate(ClimateCommand::Mode(mode)));
            }
            if has(4) {
                apply(Command::Climate(ClimateCommand::Target(float(5))));
            }
        }
        (Kind::Update, _) if get(2).map_or(0, Value::uint) == 1 => apply(Command::Install),
        _ => return None,
    }
    Some(entity)
}

#[cfg(test)]
mod tests;
