//! MQTT command payloads (anyone who can publish to the broker), for every entity kind.
#![no_main]

use iotkit::{
    model::*,
    mqtt::ha::{command_entity, parse_command},
};
use libfuzzer_sys::fuzz_target;

static SWITCH: Switch = Switch::new("s", "S");
static LIGHT: Light = Light::new("l", "L");
static BUTTON: Button = Button::new("b", "B");
static NUMBER: Number = Number::new("n", "N", 0.0, 100.0, 0.5);
static SELECT: Select = Select::new("o", "O", &["a", "b"]);
static COVER: Cover = Cover::new("c", "C").position();
static VALVE: Valve = Valve::new("v", "V").position();
static LOCK: Lock = Lock::new("k", "K");
static FAN: Fan = Fan::new("f", "F").speed().presets(&["eco"]).oscillation().direction();
static CLIMATE: Climate = Climate::new("t", "T", &[HvacMode::Heat, HvacMode::Off]);
static ENTITIES: [&dyn Entity; 10] = [&SWITCH, &LIGHT, &BUTTON, &NUMBER, &SELECT, &COVER, &VALVE, &LOCK, &FAN, &CLIMATE];

fuzz_target!(|data: &[u8]| {
    // First byte: the feature (`""`, `percentage`, ...); the rest: the payload.
    let Some((&pick, payload)) = data.split_first() else { return };
    const FEATURES: [&str; 8] = ["", "percentage", "preset", "oscillation", "direction", "mode", "temperature", "position"];
    let feature = FEATURES[usize::from(pick) % FEATURES.len()];
    for entity in ENTITIES {
        if let Some(command) = parse_command(entity, feature, payload) {
            entity.command(command);
        }
    }
    if let Ok(topic) = core::str::from_utf8(payload) {
        let _ = command_entity("iotkit-aefc", topic);
    }
});
