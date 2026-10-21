//! What an ESPHome API client sends: the handshake, encrypted frames, and the command messages
//! (anyone with the key), for every entity kind.
#![no_main]

use iotkit::{
    esphome::{ApiKey, api, noise, proto::Fields},
    model::*,
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
static DEVICE: Device = Device {
    name: "iotkit-aefc",
    manufacturer: "iotkit",
    model: "Fuzz",
    sw_version: "1.0.0",
    entities: &[&SWITCH, &LIGHT, &BUTTON, &NUMBER, &SELECT, &COVER, &VALVE, &LOCK, &FAN, &CLIMATE],
};

fuzz_target!(|data: &[u8]| {
    // A command message for each command id, its key replaced by an entity's (most random keys
    // match nothing).
    for (i, id) in [30u16, 31, 32, 33, 48, 51, 54, 60, 62, 111, 118].into_iter().enumerate() {
        let mut body = [0u8; 256];
        let n = data.len().min(body.len() - 5);
        let entity = DEVICE.entities[(i + data.first().map_or(0, |&b| usize::from(b))) % DEVICE.entities.len()];
        body[0] = 0x0d;
        body[1..5].copy_from_slice(&api::key(entity).to_le_bytes());
        body[5..5 + n].copy_from_slice(&data[..n]);
        api::command(&DEVICE, id, &body[..5 + n], |e, c| {
            e.command(c);
        });
        api::command(&DEVICE, id, data, |e, c| {
            e.command(c);
        });
    }
    let _ = Fields::new(data).count();
    if let Ok(text) = core::str::from_utf8(data) {
        let _ = ApiKey::parse(text);
    }
    // A handshake, then a frame with the session's keys.
    let mut reply = [0; noise::HANDSHAKE];
    if let Some((mut rx, _)) = noise::respond(&[7; 32], [9; 32], data, &mut reply) {
        let mut frame = [0u8; 64];
        let n = data.len().min(64);
        frame[..n].copy_from_slice(&data[..n]);
        let _ = rx.decrypt(&mut frame[..n]);
    }
});
