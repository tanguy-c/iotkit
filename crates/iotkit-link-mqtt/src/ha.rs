//! Home Assistant MQTT discovery: topics, discovery payloads, states and commands.
//!
//! Topics, for a device `iotkit-aefc` and an entity `led`:
//! - `iotkit/iotkit-aefc/status`: `online` / `offline` (retained, last will);
//! - `iotkit/iotkit-aefc/led/state`: the state (retained);
//! - `iotkit/iotkit-aefc/led/set`: commands;
//! - `homeassistant/light/iotkit-aefc/led/config`: discovery (retained).

use core::fmt::Write;

use heapless::String;
use iotkit_model::{Category, Command, Device, Entity, Kind, State};
use serde::Deserialize;

pub const PREFIX: &str = "homeassistant";
/// Home Assistant publishes `online` here when it (re)starts: send discovery again.
pub const HA_STATUS: &str = "homeassistant/status";

pub type Topic = String<128>;

fn component(kind: Kind) -> &'static str {
    match kind {
        Kind::Sensor => "sensor",
        Kind::BinarySensor => "binary_sensor",
        Kind::Switch => "switch",
        Kind::Light => "light",
        Kind::Button => "button",
    }
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

pub fn discovery_topic(node: &str, entity: &dyn Entity) -> Topic {
    let m = entity.meta();
    topic(format_args!("{PREFIX}/{}/{node}/{}/config", component(m.kind), m.id))
}

/// The entity id in a command topic of `node`.
pub fn command_entity<'t>(node: &str, topic: &'t str) -> Option<&'t str> {
    let rest = topic.strip_prefix("iotkit/")?.strip_prefix(node)?.strip_prefix('/')?;
    let id = rest.strip_suffix("/set")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
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
        if m.kind != Kind::Button {
            write!(s, ",\"state_topic\":\"{}\"", state_topic(node, m.id))?;
        }
        if matches!(m.kind, Kind::Switch | Kind::Light | Kind::Button) {
            write!(s, ",\"command_topic\":\"{}\"", command_topic(node, m.id))?;
        }
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

/// The state payload; `None` for no state to publish (a button). An unknown state is `None`,
/// which Home Assistant shows as unknown (instead of the last retained value).
pub fn state_payload(entity: &dyn Entity) -> Option<String<128>> {
    let mut s = String::new();
    match entity.state() {
        State::None => return None,
        // The JSON light schema takes no bare `None`: an unknown light publishes nothing.
        State::Unknown if entity.meta().kind == Kind::Light => return None,
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

/// A command payload for an entity of `kind`.
pub fn parse_command(kind: Kind, payload: &[u8]) -> Option<Command> {
    let on_off = |p: &[u8]| match p {
        b"ON" => Some(true),
        b"OFF" => Some(false),
        _ => None,
    };
    match kind {
        Kind::Switch => on_off(payload).map(Command::Set),
        Kind::Button => (payload == b"PRESS").then_some(Command::Press),
        Kind::Light => {
            let (j, _) = serde_json_core::from_slice::<LightJson>(payload).ok()?;
            let on = match j.state {
                Some(s) => Some(on_off(s.as_bytes())?),
                None => None,
            };
            Some(Command::Light { on, brightness: j.brightness, rgb: j.color.map(|c| [c.r, c.g, c.b]) })
        }
        Kind::Sensor | Kind::BinarySensor => None,
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
        assert_eq!(command_entity("n", "iotkit/n/led/set"), Some("led"));
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
        assert_eq!(parse_command(Kind::Switch, b"ON"), Some(Command::Set(true)));
        assert_eq!(parse_command(Kind::Switch, b"on"), None);
        assert_eq!(parse_command(Kind::Button, b"PRESS"), Some(Command::Press));
        assert_eq!(
            parse_command(Kind::Light, br#"{"state":"ON","brightness":40,"color":{"r":1,"g":2,"b":3}}"#),
            Some(Command::Light { on: Some(true), brightness: Some(40), rgb: Some([1, 2, 3]) })
        );
        assert_eq!(
            parse_command(Kind::Light, br#"{"state":"OFF"}"#),
            Some(Command::Light { on: Some(false), brightness: None, rgb: None })
        );
        assert_eq!(parse_command(Kind::Light, br#"{"state":"MAYBE"}"#), None);
        assert_eq!(parse_command(Kind::Light, b"ON"), None);
        assert_eq!(parse_command(Kind::Sensor, b"1"), None);
    }
}
