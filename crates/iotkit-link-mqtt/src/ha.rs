//! Home Assistant MQTT discovery: topics, discovery payloads, states and commands.
//!
//! Topics, for a device `iotkit-aefc` and an entity `led`:
//! - `iotkit/iotkit-aefc/status`: `online` / `offline` (retained, last will);
//! - `iotkit/iotkit-aefc/led/state`: the state (retained);
//! - `iotkit/iotkit-aefc/led/set`: commands;
//! - `iotkit/iotkit-aefc/fan/percentage/set`: commands for one setting (fans, thermostats, a
//!   cover's position);
//! - `homeassistant/light/iotkit-aefc/led/config`: discovery (retained).

use core::fmt::Write;

use heapless::String;
use iotkit_model::{
    Category, ClimateCommand, Command, CoverCommand, CoverState, Device, Entity, FanCommand, FanDirection, HvacMode,
    Kind, LockCommand, Spec, State,
};
use serde::Deserialize;

pub const PREFIX: &str = "homeassistant";
/// Home Assistant publishes `online` here when it (re)starts: send discovery again.
pub const HA_STATUS: &str = "homeassistant/status";

pub type Topic = String<128>;

fn component(kind: Kind) -> &'static str {
    // A table in flash: a `match` would become a lookup table, which esp-hal keeps in RAM. In
    // the order of `Kind`.
    static COMPONENTS: [&str; 14] = [
        "sensor",
        "binary_sensor",
        "switch",
        "light",
        "button",
        "number",
        "select",
        "sensor",
        "event",
        "cover",
        "valve",
        "lock",
        "fan",
        "climate",
    ];
    COMPONENTS[kind as usize]
}

/// Whether some entity of `device` takes commands for one setting (`<id>/<feature>/set`).
pub fn has_feature_commands(device: &Device) -> bool {
    device.entities.iter().any(|e| match e.spec() {
        Spec::Cover { position } => position && e.meta().kind == Kind::Cover,
        Spec::Fan { .. } | Spec::Climate { .. } => true,
        _ => false,
    })
}

fn topic(args: core::fmt::Arguments) -> Topic {
    let mut t = String::new();
    let _ = t.write_fmt(args);
    t
}

pub fn availability_topic(node: &str) -> Topic {
    topic(format_args!("iotkit/{node}/status"))
}

pub fn state_topic(node: &str, id: &str) -> Topic {
    topic(format_args!("iotkit/{node}/{id}/state"))
}

pub fn command_topic(node: &str, id: &str) -> Topic {
    topic(format_args!("iotkit/{node}/{id}/set"))
}

/// Subscription for the commands of every entity.
pub fn commands_filter(node: &str) -> Topic {
    topic(format_args!("iotkit/{node}/+/set"))
}

/// The command topic of one setting of an entity (`percentage`, `mode`, ...).
pub fn feature_topic(node: &str, id: &str, feature: &str) -> Topic {
    topic(format_args!("iotkit/{node}/{id}/{feature}/set"))
}

/// Subscription for the commands of one setting, of every entity.
pub fn feature_commands_filter(node: &str) -> Topic {
    topic(format_args!("iotkit/{node}/+/+/set"))
}

pub fn discovery_topic(node: &str, entity: &dyn Entity) -> Topic {
    let m = entity.meta();
    topic(format_args!("{PREFIX}/{}/{node}/{}/config", component(m.kind), m.id))
}

/// The entity id and the setting (`""` for the entity's own command topic) of a command topic
/// of `node`.
pub fn command_entity<'t>(node: &str, topic: &'t str) -> Option<(&'t str, &'t str)> {
    let rest = topic.strip_prefix("iotkit/")?.strip_prefix(node)?.strip_prefix('/')?;
    let rest = rest.strip_suffix("/set")?;
    let (id, feature) = rest.split_once('/').unwrap_or((rest, ""));
    (!id.is_empty() && !feature.contains('/')).then_some((id, feature))
}

/// Writes `items` as a JSON array of strings.
fn json_strs(out: &mut impl Write, items: impl IntoIterator<Item = &'static str>) -> core::fmt::Result {
    out.write_char('[')?;
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.write_char(',')?;
        }
        json_str(out, item)?;
    }
    out.write_char(']')
}

/// Writes `s` as a JSON string.
fn json_str(out: &mut impl Write, s: &str) -> core::fmt::Result {
    out.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

/// The discovery payload of `entity`.
pub fn discovery_payload<const N: usize>(device: &Device, node: &str, entity: &dyn Entity) -> Option<String<N>> {
    let m = entity.meta();
    let mut s = String::<N>::new();
    (|| -> core::fmt::Result {
        s.write_str("{\"name\":")?;
        json_str(&mut s, m.name)?;
        write!(s, ",\"unique_id\":\"{node}_{}\"", m.id)?;
        write!(s, ",\"availability_topic\":\"{}\"", availability_topic(node))?;
        // A thermostat has a topic per setting, below.
        if !matches!(m.kind, Kind::Button | Kind::Climate) {
            write!(s, ",\"state_topic\":\"{}\"", state_topic(node, m.id))?;
        }
        if matches!(
            m.kind,
            Kind::Switch
                | Kind::Light
                | Kind::Button
                | Kind::Number
                | Kind::Select
                | Kind::Cover
                | Kind::Valve
                | Kind::Lock
                | Kind::Fan
        ) {
            write!(s, ",\"command_topic\":\"{}\"", command_topic(node, m.id))?;
        }
        // The newer kinds use Home Assistant's abbreviated keys: a thermostat's discovery would
        // not fit in the buffer with the long ones.
        spec_fields(&mut s, node, entity)?;
        if m.kind == Kind::Light {
            s.write_str(",\"schema\":\"json\",\"brightness\":true,\"supported_color_modes\":[\"rgb\"]")?;
        }
        if let Some(class) = m.device_class {
            s.write_str(",\"device_class\":")?;
            json_str(&mut s, class)?;
        }
        if let Some(unit) = m.unit {
            s.write_str(",\"unit_of_measurement\":")?;
            json_str(&mut s, unit)?;
        }
        if m.measurement {
            s.write_str(",\"state_class\":\"measurement\"")?;
        }
        if let Some(p) = m.precision {
            write!(s, ",\"suggested_display_precision\":{p}")?;
        }
        if let Some(icon) = m.icon {
            s.write_str(",\"icon\":")?;
            json_str(&mut s, icon)?;
        }
        match m.category {
            Category::Primary => {}
            Category::Config => s.write_str(",\"entity_category\":\"config\"")?,
            Category::Diagnostic => s.write_str(",\"entity_category\":\"diagnostic\"")?,
        }
        write!(s, ",\"device\":{{\"identifiers\":[\"{node}\"],\"name\":")?;
        json_str(&mut s, device.name)?;
        s.write_str(",\"manufacturer\":")?;
        json_str(&mut s, device.manufacturer)?;
        s.write_str(",\"model\":")?;
        json_str(&mut s, device.model)?;
        s.write_str(",\"sw_version\":")?;
        json_str(&mut s, device.sw_version)?;
        s.write_str("}}")
    })()
    .ok()?;
    Some(s)
}

/// The discovery fields of the kinds that have a [`Spec`].
fn spec_fields(s: &mut impl Write, node: &str, entity: &dyn Entity) -> core::fmt::Result {
    let m = entity.meta();
    // Topics relative to Home Assistant's base topic `~` (`iotkit/<node>/<id>`), set once.
    let state = "~/state";
    let feature = |f: &str| {
        let mut t: String<24> = String::new();
        let _ = write!(t, "~/{f}/set");
        t
    };
    let spec = entity.spec();
    if matches!(spec, Spec::Cover { position: true } if m.kind == Kind::Cover)
        || matches!(spec, Spec::Fan { .. } | Spec::Climate { .. })
    {
        write!(s, ",\"~\":\"iotkit/{node}/{}\"", m.id)?;
    }
    match spec {
        Spec::None => {}
        Spec::Number { min, max, step, slider } => {
            write!(s, ",\"min\":{min},\"max\":{max},\"step\":{step}")?;
            s.write_str(if slider { ",\"mode\":\"slider\"" } else { ",\"mode\":\"box\"" })?;
        }
        Spec::Select { options } => {
            s.write_str(",\"ops\":")?;
            json_strs(s, options.iter().copied())?;
        }
        Spec::Event { types } => {
            s.write_str(",\"evt_typ\":")?;
            json_strs(s, types.iter().copied())?;
        }
        Spec::Cover { position } if m.kind == Kind::Valve => {
            s.write_str(",\"pl_stop\":\"STOP\"")?;
            if position {
                s.write_str(",\"pos\":true")?;
            }
        }
        Spec::Cover { position } => {
            s.write_str(",\"val_tpl\":\"{{ value_json.state | default('') }}\"")?;
            if position {
                write!(s, ",\"pos_t\":\"{state}\",\"pos_tpl\":\"{{{{ value_json.position | default('') }}}}\"")?;
                write!(s, ",\"set_pos_t\":\"{}\"", feature("position"))?;
            }
        }
        Spec::Lock { open } => {
            if open {
                s.write_str(",\"pl_open\":\"OPEN\"")?;
            }
        }
        Spec::Fan { speed, presets, oscillation, direction } => {
            s.write_str(",\"stat_val_tpl\":\"{{ value_json.state }}\"")?;
            if speed {
                write!(s, ",\"pct_cmd_t\":\"{}\",\"pct_stat_t\":\"{state}\"", feature("percentage"))?;
                s.write_str(",\"pct_val_tpl\":\"{{ value_json.percentage }}\"")?;
            }
            if !presets.is_empty() {
                write!(s, ",\"pr_mode_cmd_t\":\"{}\",\"pr_mode_stat_t\":\"{state}\"", feature("preset"))?;
                s.write_str(",\"pr_mode_val_tpl\":\"{{ value_json.preset_mode }}\",\"pr_modes\":")?;
                json_strs(s, presets.iter().copied())?;
            }
            if oscillation {
                write!(s, ",\"osc_cmd_t\":\"{}\",\"osc_stat_t\":\"{state}\"", feature("oscillation"))?;
                s.write_str(
                    ",\"osc_val_tpl\":\"{{ value_json.oscillation }}\",\"pl_osc_on\":\"ON\",\"pl_osc_off\":\"OFF\"",
                )?;
            }
            if direction {
                write!(s, ",\"dir_cmd_t\":\"{}\",\"dir_stat_t\":\"{state}\"", feature("direction"))?;
                s.write_str(",\"dir_val_tpl\":\"{{ value_json.direction }}\"")?;
            }
        }
        Spec::Climate { modes, min, max, step } => {
            s.write_str(",\"modes\":")?;
            json_strs(s, modes.iter().map(|m| m.as_str()))?;
            write!(s, ",\"mode_cmd_t\":\"{}\",\"mode_stat_t\":\"{state}\"", feature("mode"))?;
            s.write_str(",\"mode_stat_tpl\":\"{{ value_json.mode }}\"")?;
            write!(s, ",\"temp_cmd_t\":\"{}\",\"temp_stat_t\":\"{state}\"", feature("temperature"))?;
            s.write_str(",\"temp_stat_tpl\":\"{{ value_json.target }}\"")?;
            write!(s, ",\"curr_temp_t\":\"{state}\",\"act_t\":\"{state}\"")?;
            s.write_str(",\"curr_temp_tpl\":\"{{ value_json.current }}\",\"act_tpl\":\"{{ value_json.action }}\"")?;
            write!(s, ",\"min_temp\":{min},\"max_temp\":{max},\"temp_step\":{step},\"temp_unit\":\"C\"")?;
        }
    }
    Ok(())
}

/// Whether the state is published retained: events are not states.
pub fn retained(entity: &dyn Entity) -> bool {
    entity.meta().kind != Kind::Event
}

/// Writes `v` as JSON (`null` for `None`).
fn json_opt(out: &mut impl Write, v: Option<impl core::fmt::Display>) -> core::fmt::Result {
    match v {
        Some(v) => write!(out, "{v}"),
        None => out.write_str("null"),
    }
}

/// The state payload; `None` for no state to publish (a button). An unknown state is `None`,
/// which Home Assistant shows as unknown (instead of the last retained value).
pub fn state_payload(entity: &dyn Entity) -> Option<String<128>> {
    let mut s = String::new();
    match entity.state() {
        State::None => return None,
        // The JSON light schema takes no bare `None`: an unknown light publishes nothing.
        State::Unknown if matches!(entity.meta().kind, Kind::Light | Kind::Fan | Kind::Climate | Kind::Event) => {
            return None;
        }
        State::Unknown => s.push_str("None").ok()?,
        State::Number(v) => match entity.meta().precision {
            Some(p) => write!(s, "{v:.*}", usize::from(p)).ok()?,
            None => write!(s, "{v}").ok()?,
        },
        State::Bool(on) => s.push_str(if on { "ON" } else { "OFF" }).ok()?,
        State::Light(l) => write!(
            s,
            "{{\"state\":\"{}\",\"brightness\":{},\"color_mode\":\"rgb\",\"color\":{{\"r\":{},\"g\":{},\"b\":{}}}}}",
            if l.on { "ON" } else { "OFF" },
            l.brightness,
            l.rgb[0],
            l.rgb[1],
            l.rgb[2]
        )
        .ok()?,
        State::Option(i) => match entity.spec() {
            Spec::Select { options } => s.push_str(options.get(usize::from(i))?).ok()?,
            _ => return None,
        },
        State::Text => s.push_str(&entity.text()?).ok()?,
        State::Event(i) => match entity.spec() {
            Spec::Event { types } => write!(s, "{{\"event_type\":\"{}\"}}", types.get(usize::from(i))?).ok()?,
            _ => return None,
        },
        State::Cover { state, position } if entity.meta().kind == Kind::Valve => {
            // A valve takes a plain state, or JSON with its position.
            let state = state.map(|st| if st == CoverState::Stopped { CoverState::Open } else { st });
            let reports = matches!(entity.spec(), Spec::Cover { position: true });
            match (position.filter(|_| reports), state) {
                (Some(p), Some(st)) => write!(s, "{{\"state\":\"{}\",\"position\":{p}}}", st.as_str()).ok()?,
                (Some(p), None) => write!(s, "{{\"position\":{p}}}").ok()?,
                (None, Some(st)) => s.push_str(st.as_str()).ok()?,
                (None, None) => s.push_str("None").ok()?,
            }
        }
        State::Cover { state, position } => {
            s.push_str("{\"state\":").ok()?;
            match state {
                Some(st) => write!(s, "\"{}\"", st.as_str()).ok()?,
                None => s.push_str("null").ok()?,
            }
            if let Some(p) = position {
                write!(s, ",\"position\":{p}").ok()?;
            }
            s.push_str("}").ok()?;
        }
        State::Lock(l) => s.push_str(l.as_str()).ok()?,
        State::Fan(f) => {
            let on = |b: bool| if b { "ON" } else { "OFF" };
            write!(s, "{{\"state\":\"{}\",\"percentage\":{},\"preset_mode\":", on(f.on), f.percentage).ok()?;
            let preset = match entity.spec() {
                Spec::Fan { presets, .. } => f.preset.and_then(|i| presets.get(usize::from(i)).copied()),
                _ => None,
            };
            match preset {
                Some(p) => json_str(&mut s, p).ok()?,
                None => s.push_str("null").ok()?,
            }
            write!(s, ",\"oscillation\":\"{}\",\"direction\":\"{}\"}}", on(f.oscillating), f.direction.as_str())
                .ok()?;
        }
        State::Climate(c) => {
            write!(s, "{{\"mode\":\"{}\",\"target\":{},\"current\":", c.mode.as_str(), c.target).ok()?;
            json_opt(&mut s, c.current).ok()?;
            s.push_str(",\"action\":").ok()?;
            match c.action {
                Some(a) => write!(s, "\"{}\"", a.as_str()).ok()?,
                None => s.push_str("null").ok()?,
            }
            s.push_str("}").ok()?;
        }
    }
    Some(s)
}

#[derive(Deserialize)]
struct LightJson<'a> {
    #[serde(borrow)]
    state: Option<&'a str>,
    brightness: Option<u8>,
    color: Option<Rgb>,
}

#[derive(Deserialize)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

/// A command payload for `entity`, on its own command topic (`feature` `""`) or on the topic
/// of one setting.
pub fn parse_command(entity: &dyn Entity, feature: &str, payload: &[u8]) -> Option<Command> {
    let on_off = |p: &[u8]| match p {
        b"ON" => Some(true),
        b"OFF" => Some(false),
        _ => None,
    };
    let text = || core::str::from_utf8(payload).ok().map(str::trim);
    let number = || text()?.parse::<f32>().ok().filter(|v| v.is_finite());
    let index = |options: &[&str]| {
        let t = text()?;
        options.iter().position(|o| *o == t).and_then(|i| u8::try_from(i).ok())
    };
    let percent = || text()?.parse::<u8>().ok().filter(|p| *p <= 100);
    let cover = || match payload {
        b"OPEN" => Some(CoverCommand::Open),
        b"CLOSE" => Some(CoverCommand::Close),
        b"STOP" => Some(CoverCommand::Stop),
        _ => None,
    };
    let kind = entity.meta().kind;
    if !feature.is_empty() {
        return match (entity.spec(), feature) {
            (Spec::Cover { .. }, "position") if kind == Kind::Cover => {
                Some(Command::Cover(CoverCommand::Position(percent()?)))
            }
            (Spec::Fan { presets, .. }, f) => Some(Command::Fan(match f {
                "percentage" => FanCommand::Percentage(percent()?),
                "preset" => FanCommand::Preset(index(presets)?),
                "oscillation" => FanCommand::Oscillate(on_off(payload)?),
                "direction" => FanCommand::Direction(match payload {
                    b"forward" => FanDirection::Forward,
                    b"reverse" => FanDirection::Reverse,
                    _ => return None,
                }),
                _ => return None,
            })),
            (Spec::Climate { .. }, "mode") => {
                let t = text()?;
                Some(Command::Climate(ClimateCommand::Mode(HvacMode::ALL.into_iter().find(|m| m.as_str() == t)?)))
            }
            (Spec::Climate { .. }, "temperature") => Some(Command::Climate(ClimateCommand::Target(number()?))),
            _ => None,
        };
    }
    match kind {
        Kind::Switch => on_off(payload).map(Command::Set),
        Kind::Button => (payload == b"PRESS").then_some(Command::Press),
        Kind::Number => number().map(Command::Number),
        Kind::Select => match entity.spec() {
            Spec::Select { options } => index(options).map(Command::Option),
            _ => None,
        },
        Kind::Cover => cover().map(Command::Cover),
        // A valve with a position gets the position on its own topic.
        Kind::Valve => cover().or_else(|| percent().map(CoverCommand::Position)).map(Command::Cover),
        Kind::Lock => match payload {
            b"LOCK" => Some(Command::Lock(LockCommand::Lock)),
            b"UNLOCK" => Some(Command::Lock(LockCommand::Unlock)),
            b"OPEN" => Some(Command::Lock(LockCommand::Open)),
            _ => None,
        },
        Kind::Fan => on_off(payload).map(|on| Command::Fan(FanCommand::On(on))),
        Kind::Light => {
            let (j, _) = serde_json_core::from_slice::<LightJson>(payload).ok()?;
            let on = match j.state {
                Some(s) => Some(on_off(s.as_bytes())?),
                None => None,
            };
            Some(Command::Light { on, brightness: j.brightness, rgb: j.color.map(|c| [c.r, c.g, c.b]) })
        }
        Kind::Sensor | Kind::BinarySensor | Kind::TextSensor | Kind::Event | Kind::Climate => None,
    }
}

#[cfg(test)]
mod tests {
    use iotkit_model::{BinarySensor, Button, Light, LightState, Sensor, Switch};

    use super::*;

    static TEMP: Sensor = Sensor::new("temperature", "Chip \"temp\"")
        .unit("°C")
        .device_class("temperature")
        .measurement()
        .precision(1)
        .diagnostic();
    static RELAY: Switch = Switch::new("relay", "Relay");
    static LED: Light = Light::new("led", "LED");
    static RESTART: Button = Button::new("restart", "Restart").device_class("restart").diagnostic();
    static DOOR: BinarySensor = BinarySensor::new("door", "Door");
    static ENTITIES: [&dyn Entity; 5] = [&TEMP, &RELAY, &LED, &RESTART, &DOOR];
    static DEVICE: Device =
        Device { name: "Desk", manufacturer: "iotkit", model: "devkit", sw_version: "0.5.0", entities: &ENTITIES };

    fn json(s: &str) -> serde_json_core::heapless::String<0> {
        // Valid JSON? serde-json-core parses into an ignored value via a map of unit.
        #[derive(Deserialize)]
        struct Any {}
        serde_json_core::from_str::<Any>(s).map(|_| ()).unwrap();
        serde_json_core::heapless::String::new()
    }

    #[test]
    fn topics() {
        assert_eq!(state_topic("iotkit-aefc", "led"), "iotkit/iotkit-aefc/led/state");
        assert_eq!(discovery_topic("iotkit-aefc", &LED), "homeassistant/light/iotkit-aefc/led/config");
        assert_eq!(discovery_topic("n", &RESTART), "homeassistant/button/n/restart/config");
        assert_eq!(command_entity("n", "iotkit/n/led/set"), Some(("led", "")));
        assert_eq!(command_entity("n", "iotkit/n/fan/percentage/set"), Some(("fan", "percentage")));
        assert_eq!(command_entity("n", "iotkit/n/fan/a/b/set"), None);
        assert_eq!(command_entity("n", "iotkit/other/led/set"), None);
        assert_eq!(command_entity("n", "iotkit/n/led/state"), None);
        assert_eq!(command_entity("n", "iotkit/n//set"), None);
        assert_eq!(commands_filter("n"), "iotkit/n/+/set");
    }

    #[test]
    fn discovery_payloads() {
        let p: String<1024> = discovery_payload(&DEVICE, "iotkit-aefc", &TEMP).unwrap();
        json(&p);
        for part in [
            "\"name\":\"Chip \\\"temp\\\"\"",
            "\"unique_id\":\"iotkit-aefc_temperature\"",
            "\"state_topic\":\"iotkit/iotkit-aefc/temperature/state\"",
            "\"unit_of_measurement\":\"°C\"",
            "\"state_class\":\"measurement\"",
            "\"suggested_display_precision\":1",
            "\"entity_category\":\"diagnostic\"",
            "\"identifiers\":[\"iotkit-aefc\"]",
            "\"sw_version\":\"0.5.0\"",
        ] {
            assert!(p.contains(part), "{part} in {p}");
        }
        assert!(!p.contains("command_topic"));
        let p: String<1024> = discovery_payload(&DEVICE, "n", &LED).unwrap();
        json(&p);
        assert!(p.contains("\"schema\":\"json\"") && p.contains("\"command_topic\":\"iotkit/n/led/set\""));
        let p: String<1024> = discovery_payload(&DEVICE, "n", &RESTART).unwrap();
        json(&p);
        assert!(!p.contains("state_topic") && p.contains("\"device_class\":\"restart\""));
        assert!(discovery_payload::<64>(&DEVICE, "n", &LED).is_none(), "too small a buffer");
    }

    #[test]
    fn states() {
        assert_eq!(state_payload(&TEMP).unwrap(), "None", "unknown, for Home Assistant");
        TEMP.set(21.46);
        assert_eq!(state_payload(&TEMP).unwrap(), "21.5");
        RELAY.set(true);
        assert_eq!(state_payload(&RELAY).unwrap(), "ON");
        DOOR.set(false);
        assert_eq!(state_payload(&DOOR).unwrap(), "OFF");
        LED.set(LightState { on: true, brightness: 128, rgb: [255, 0, 10] });
        let p = state_payload(&LED).unwrap();
        json(&p);
        assert_eq!(
            p,
            "{\"state\":\"ON\",\"brightness\":128,\"color_mode\":\"rgb\",\"color\":{\"r\":255,\"g\":0,\"b\":10}}"
        );
        assert_eq!(state_payload(&RESTART), None);
    }

    #[test]
    fn commands() {
        assert_eq!(parse_command(&RELAY, "", b"ON"), Some(Command::Set(true)));
        assert_eq!(parse_command(&RELAY, "", b"on"), None);
        assert_eq!(parse_command(&RESTART, "", b"PRESS"), Some(Command::Press));
        assert_eq!(
            parse_command(&LED, "", br#"{"state":"ON","brightness":40,"color":{"r":1,"g":2,"b":3}}"#),
            Some(Command::Light { on: Some(true), brightness: Some(40), rgb: Some([1, 2, 3]) })
        );
        assert_eq!(
            parse_command(&LED, "", br#"{"state":"OFF"}"#),
            Some(Command::Light { on: Some(false), brightness: None, rgb: None })
        );
        assert_eq!(parse_command(&LED, "", br#"{"state":"MAYBE"}"#), None);
        assert_eq!(parse_command(&LED, "", b"ON"), None);
        assert_eq!(parse_command(&TEMP, "", b"1"), None);
    }

    mod newer {
        use iotkit_model::{
            Climate, ClimateState, Cover, CoverState, Event, Fan, FanState, HvacAction, Lock, LockState, Number,
            Select, TextSensor, Valve,
        };

        use super::*;

        static NUMBER: Number = Number::new("setpoint", "Setpoint", 5.0, 30.0, 0.5).unit("°C").slider();
        static SELECT: Select = Select::new("mode", "Mode", &["eco", "comfort"]);
        static TEXT: TextSensor = TextSensor::new("status", "Status");
        static EVENT: Event = Event::new("button", "Button", &["press", "long_press"]);
        static COVER: Cover = Cover::new("blind", "Blind").device_class("blind").position();
        static VALVE: Valve = Valve::new("water", "Water").device_class("water").position();
        static LOCK: Lock = Lock::new("door", "Door").open();
        static FAN: Fan = Fan::new("fan_with_a_long_name", "Fan")
            .speed()
            .presets(&["sleep", "breeze", "turbo"])
            .oscillation()
            .direction();
        static CLIMATE: Climate =
            Climate::new("thermostat_long_id", "Thermostat", &[HvacMode::Heat, HvacMode::Cool, HvacMode::Off]);
        static ALL: [&dyn Entity; 9] = [&NUMBER, &SELECT, &TEXT, &EVENT, &COVER, &VALVE, &LOCK, &FAN, &CLIMATE];
        static DEV: Device =
            Device { name: "Showcase", manufacturer: "iotkit", model: "devkit", sw_version: "1.0.0", entities: &ALL };

        #[test]
        fn discovery_fits_and_parses() {
            for e in ALL {
                // A real host name is 11 characters (`iotkit-aefc`).
                let p: String<1024> = discovery_payload(&DEV, "iotkit-aefc", e)
                    .unwrap_or_else(|| panic!("{} does not fit in 1024 bytes", e.meta().id));
                json(&p);
            }
            let p: String<1024> = discovery_payload(&DEV, "n", &CLIMATE).unwrap();
            assert!(!p.contains("\"state_topic\"") && !p.contains("\"command_topic\""));
            assert!(
                p.contains("\"modes\":[\"heat\",\"cool\",\"off\"]")
                    && p.contains("\"~\":\"iotkit/n/thermostat_long_id\"")
                    && p.contains("\"temp_cmd_t\":\"~/temperature/set\"")
            );
            let p: String<1024> = discovery_payload(&DEV, "n", &FAN).unwrap();
            assert!(p.contains("\"pr_modes\":[\"sleep\",\"breeze\",\"turbo\"]") && p.contains("\"pl_osc_on\":\"ON\""));
            let p: String<1024> = discovery_payload(&DEV, "n", &COVER).unwrap();
            assert!(p.contains("\"set_pos_t\":\"~/position/set\"") && p.contains("{{ value_json.position"));
            assert_eq!(discovery_topic("n", &TEXT), "homeassistant/sensor/n/status/config");
            assert!(has_feature_commands(&DEV) && !has_feature_commands(&super::DEVICE));
        }

        #[test]
        fn newer_states() {
            NUMBER.set(21.5);
            assert_eq!(state_payload(&NUMBER).unwrap(), "21.5");
            SELECT.set(1);
            assert_eq!(state_payload(&SELECT).unwrap(), "comfort");
            TEXT.set("ok");
            assert_eq!(state_payload(&TEXT).unwrap(), "ok");
            assert_eq!(state_payload(&EVENT), None);
            EVENT.trigger(1);
            assert_eq!(state_payload(&EVENT).unwrap(), "{\"event_type\":\"long_press\"}");
            assert!(!retained(&EVENT) && retained(&COVER));
            COVER.set(Some(CoverState::Opening), Some(40));
            assert_eq!(state_payload(&COVER).unwrap(), "{\"state\":\"opening\",\"position\":40}");
            VALVE.set(Some(CoverState::Stopped), None);
            assert_eq!(state_payload(&VALVE).unwrap(), "open");
            VALVE.set(Some(CoverState::Closed), Some(0));
            assert_eq!(state_payload(&VALVE).unwrap(), "{\"state\":\"closed\",\"position\":0}");
            LOCK.set(LockState::Jammed);
            assert_eq!(state_payload(&LOCK).unwrap(), "JAMMED");
            FAN.set(FanState { on: true, percentage: 40, preset: Some(2), ..FanState::OFF });
            let p = state_payload(&FAN).unwrap();
            json(&p);
            assert!(p.contains("\"preset_mode\":\"turbo\"") && p.contains("\"percentage\":40"));
            CLIMATE.set(ClimateState {
                mode: HvacMode::Heat,
                target: 21.5,
                current: None,
                action: Some(HvacAction::Heating),
            });
            assert_eq!(
                state_payload(&CLIMATE).unwrap(),
                "{\"mode\":\"heat\",\"target\":21.5,\"current\":null,\"action\":\"heating\"}"
            );
        }

        #[test]
        fn newer_commands() {
            use iotkit_model::{ClimateCommand as C, CoverCommand as V, FanCommand as F, LockCommand as L};
            assert_eq!(parse_command(&NUMBER, "", b"22.5"), Some(Command::Number(22.5)));
            assert_eq!(parse_command(&NUMBER, "", b"warm"), None);
            assert_eq!(parse_command(&SELECT, "", b"comfort"), Some(Command::Option(1)));
            assert_eq!(parse_command(&SELECT, "", b"party"), None);
            assert_eq!(parse_command(&COVER, "", b"STOP"), Some(Command::Cover(V::Stop)));
            assert_eq!(parse_command(&COVER, "position", b"30"), Some(Command::Cover(V::Position(30))));
            assert_eq!(parse_command(&COVER, "position", b"130"), None);
            assert_eq!(parse_command(&VALVE, "", b"70"), Some(Command::Cover(V::Position(70))));
            assert_eq!(parse_command(&LOCK, "", b"OPEN"), Some(Command::Lock(L::Open)));
            assert_eq!(parse_command(&FAN, "", b"OFF"), Some(Command::Fan(F::On(false))));
            assert_eq!(parse_command(&FAN, "preset", b"breeze"), Some(Command::Fan(F::Preset(1))));
            assert_eq!(
                parse_command(&FAN, "direction", b"reverse"),
                Some(Command::Fan(F::Direction(FanDirection::Reverse)))
            );
            assert_eq!(parse_command(&CLIMATE, "mode", b"cool"), Some(Command::Climate(C::Mode(HvacMode::Cool))));
            assert_eq!(parse_command(&CLIMATE, "temperature", b"19.5"), Some(Command::Climate(C::Target(19.5))));
            assert_eq!(parse_command(&CLIMATE, "", b"ON"), None);
            assert_eq!(parse_command(&TEXT, "", b"x"), None);
        }
    }
}
