use super::*;
use crate::model::{
    Button, ClimateState, Cover, DeviceClass, Event, Fan, FanState, Light, LightState, Lock, Number, Select, Sensor,
    Switch, TextSensor,
};

static TEMPERATURE: Sensor = Sensor::temperature("temperature", "Temperature");
static RELAY: Switch = Switch::new("relay", "Relay");
static LAMP: Light = Light::new("lamp", "Lamp");
static MODE: Select = Select::new("mode", "Mode", &["eco", "boost"]);
static SETPOINT: Number = Number::new("setpoint", "Setpoint", 5.0, 30.0, 0.5);
static BLIND: Cover = Cover::new("blind", "Blind").position();
static DOOR: Lock = Lock::new("door", "Door");
static FAN: Fan = Fan::new("fan", "Fan").speed().presets(&["sleep"]).oscillation();
static RESTART: Button = Button::new("restart", "Restart").device_class(DeviceClass::RESTART);
static PRESS: Event = Event::new("press", "Press", &["single", "double"]);
static STATUS: TextSensor = TextSensor::new("status", "Status");
static THERMOSTAT: crate::model::Climate =
    crate::model::Climate::new("thermostat", "Thermostat", &[HvacMode::Off, HvacMode::Heat]);
static DEVICE: Device = Device {
    name: "iotkit-aefc",
    manufacturer: "iotkit",
    model: "Test",
    sw_version: "1.2.3",
    entities: &[
        &TEMPERATURE,
        &RELAY,
        &LAMP,
        &MODE,
        &SETPOINT,
        &BLIND,
        &DOOR,
        &FAN,
        &RESTART,
        &PRESS,
        &STATUS,
        &THERMOSTAT,
    ],
};

/// A message written by `f`: its id, and its fields.
fn message(f: impl FnOnce(&mut Writer) -> Option<u16>) -> Option<(u16, heapless::Vec<(u32, Value<'static>), 16>)> {
    // Leaked per call: the fields borrow it, and tests are short.
    let buf: &'static mut [u8; 256] = Box::leak(Box::new([0; 256]));
    let mut w = Writer::new(buf);
    let id = f(&mut w)?;
    let n = w.finish().unwrap();
    Some((id, Fields::new(&buf[..n]).collect()))
}

fn field(fields: &[(u32, Value<'static>)], n: u32) -> Option<Value<'static>> {
    fields.iter().find(|(f, _)| *f == n).map(|(_, v)| *v)
}

extern crate std;
use std::boxed::Box;

#[test]
fn hello_and_device_info() {
    let (id, f) = message(|w| Some(hello(w, "iotkit-aefc"))).unwrap();
    assert_eq!(id, id::HELLO_RESPONSE);
    assert_eq!((field(&f, 1).unwrap().uint(), field(&f, 2).unwrap().uint()), VERSION);
    assert_eq!(field(&f, 4).unwrap().str(), "iotkit-aefc");
    let (id, f) = message(|w| Some(device_info(w, &DEVICE, "AA:BB:CC:DD:EE:FF"))).unwrap();
    assert_eq!(id, id::DEVICE_INFO_RESPONSE);
    assert_eq!(field(&f, 3).unwrap().str(), "AA:BB:CC:DD:EE:FF");
    assert_eq!(field(&f, 9).unwrap().str(), "1.2.3");
    assert_eq!(field(&f, 1), None, "no password");
}

#[test]
fn entity_list() {
    let (id, f) = message(|w| Some(list_entity(w, &TEMPERATURE))).unwrap();
    assert_eq!(id, 16);
    assert_eq!(field(&f, 1).unwrap().str(), "temperature");
    assert_eq!(field(&f, 2).unwrap().fixed32(), key(&TEMPERATURE));
    assert_eq!(field(&f, 6).unwrap().str(), "°C");
    assert_eq!(field(&f, 9).unwrap().str(), "temperature");
    assert_eq!(field(&f, 10).unwrap().uint(), 1, "measurement");
    let (id, f) = message(|w| Some(list_entity(w, &MODE))).unwrap();
    assert_eq!(id, 52);
    let options: heapless::Vec<&str, 4> = f.iter().filter(|(n, _)| *n == 6).map(|(_, v)| v.str()).collect();
    assert_eq!(options, ["eco", "boost"]);
    let (_, f) = message(|w| Some(list_entity(w, &RESTART))).unwrap();
    assert_eq!(field(&f, 8).unwrap().str(), "restart");
    let (_, f) = message(|w| Some(list_entity(w, &THERMOSTAT))).unwrap();
    let modes: heapless::Vec<u32, 4> = f.iter().filter(|(n, _)| *n == 7).map(|(_, v)| v.uint()).collect();
    assert_eq!(modes, [0, 3], "off is written although 0");
    // Every kind has a message, with a distinct key.
    let ids: heapless::Vec<u16, 16> =
        DEVICE.entities.iter().map(|e| message(|w| Some(list_entity(w, *e))).unwrap().0).collect();
    assert!(ids.iter().all(|&i| i != 0));
    let keys: heapless::Vec<u32, 16> = DEVICE.entities.iter().map(|e| key(*e)).collect();
    assert!(keys.iter().enumerate().all(|(i, k)| !keys[..i].contains(k)));
}

#[test]
fn states() {
    let (id, f) = message(|w| state(w, &TEMPERATURE)).unwrap();
    assert_eq!(id, 25);
    assert!(field(&f, 3).unwrap().bool(), "missing before the first reading");
    TEMPERATURE.set(21.5);
    let (_, f) = message(|w| state(w, &TEMPERATURE)).unwrap();
    assert_eq!((field(&f, 2).unwrap().float(), field(&f, 3)), (21.5, None));
    assert!(message(|w| state(w, &LAMP)).is_none(), "a light never set");
    LAMP.set(LightState { on: true, brightness: 255, rgb: [255, 0, 0] });
    let (id, f) = message(|w| state(w, &LAMP)).unwrap();
    assert_eq!((id, field(&f, 3).unwrap().float(), field(&f, 4).unwrap().float(), field(&f, 5)), (24, 1.0, 1.0, None));
    MODE.set(1);
    assert_eq!(field(&message(|w| state(w, &MODE)).unwrap().1, 2).unwrap().str(), "boost");
    BLIND.set(Some(CoverState::Opening), Some(40));
    let (_, f) = message(|w| state(w, &BLIND)).unwrap();
    assert_eq!((field(&f, 3).unwrap().float(), field(&f, 5).unwrap().uint()), (0.4, 1));
    DOOR.set(LockState::Locked);
    assert_eq!(field(&message(|w| state(w, &DOOR)).unwrap().1, 2).unwrap().uint(), 1);
    FAN.set(FanState { on: true, percentage: 60, preset: Some(0), ..FanState::OFF });
    let (_, f) = message(|w| state(w, &FAN)).unwrap();
    assert_eq!((field(&f, 6).unwrap().uint(), field(&f, 7).unwrap().str()), (60, "sleep"));
    STATUS.set("ok");
    assert_eq!(field(&message(|w| state(w, &STATUS)).unwrap().1, 2).unwrap().str(), "ok");
    THERMOSTAT.set(ClimateState {
        mode: HvacMode::Heat,
        target: 20.0,
        current: None,
        action: Some(HvacAction::Heating),
    });
    let (_, f) = message(|w| state(w, &THERMOSTAT)).unwrap();
    assert_eq!((field(&f, 2).unwrap().uint(), field(&f, 8).unwrap().uint()), (3, 3));
    assert!(field(&f, 3).unwrap().float().is_nan(), "no measured temperature");
    PRESS.trigger(1);
    assert_eq!(field(&message(|w| state(w, &PRESS)).unwrap().1, 2).unwrap().str(), "double");
    assert!(message(|w| state(w, &RESTART)).is_none(), "buttons have no state");
}

/// A command message for `entity` with `fields` after its key.
fn send(
    id: u16,
    entity: &dyn Entity,
    fields: impl FnOnce(&mut Writer),
) -> (Option<&'static str>, heapless::Vec<Command, 4>) {
    let mut buf = [0; 64];
    let mut w = Writer::new(&mut buf);
    w.fixed32(1, key(entity));
    fields(&mut w);
    let n = w.finish().unwrap();
    let mut commands = heapless::Vec::new();
    let to = command(&DEVICE, id, &buf[..n], |_, c| commands.push(c).unwrap()).map(|e| e.meta().id);
    (to, commands)
}

#[test]
fn commands() {
    assert_eq!(
        send(33, &RELAY, |w| w.bool(2, true)),
        (Some("relay"), heapless::Vec::from_slice(&[Command::Set(true)]).unwrap())
    );
    let (_, c) = send(32, &LAMP, |w| {
        w.bool(2, true);
        w.bool(3, true);
        w.bool(4, true);
        w.float(5, 0.5);
    });
    assert_eq!(c[..], [Command::Light { on: Some(true), brightness: Some(128), rgb: None }]);
    assert_eq!(send(54, &MODE, |w| w.str(2, "eco")).1[..], [Command::Option(0)]);
    assert_eq!(send(54, &MODE, |w| w.str(2, "turbo")).0, None, "not an option");
    assert_eq!(send(51, &SETPOINT, |w| w.float(2, 21.5)).1[..], [Command::Number(21.5)]);
    assert_eq!(send(62, &RESTART, |_| {}).1[..], [Command::Press]);
    let position = |p: f32| {
        send(30, &BLIND, |w| {
            w.bool(4, true);
            w.float(5, p);
        })
        .1
    };
    assert_eq!(position(1.0)[..], [Command::Cover(CoverCommand::Open)]);
    assert_eq!(position(0.0)[..], [Command::Cover(CoverCommand::Close)]);
    assert_eq!(position(0.25)[..], [Command::Cover(CoverCommand::Position(25))]);
    assert_eq!(send(30, &BLIND, |w| w.bool(8, true)).1[..], [Command::Cover(CoverCommand::Stop)]);
    assert_eq!(send(60, &DOOR, |w| w.uint(2, 1)).1[..], [Command::Lock(LockCommand::Lock)]);
    let (_, c) = send(31, &FAN, |w| {
        w.bool(2, true);
        w.bool(3, true);
        w.bool(10, true);
        w.int(11, 40);
        w.bool(12, true);
        w.str(13, "sleep");
    });
    assert_eq!(
        c[..],
        [
            Command::Fan(FanCommand::On(true)),
            Command::Fan(FanCommand::Percentage(40)),
            Command::Fan(FanCommand::Preset(0))
        ]
    );
    let (_, c) = send(48, &THERMOSTAT, |w| {
        w.bool(2, true);
        w.uint(3, 3);
        w.bool(4, true);
        w.float(5, 19.0);
    });
    assert_eq!(
        c[..],
        [Command::Climate(ClimateCommand::Mode(HvacMode::Heat)), Command::Climate(ClimateCommand::Target(19.0))]
    );
    // The wrong message for the entity, an unknown key, garbage.
    assert_eq!(send(33, &LAMP, |w| w.bool(2, true)).0, None);
    assert_eq!(command(&DEVICE, 33, &[0x0d, 1, 2, 3, 4], |_, _| {}).map(|e| e.meta().id), None);
    assert_eq!(command(&DEVICE, 33, &[0xff], |_, _| {}).map(|e| e.meta().id), None);
}
