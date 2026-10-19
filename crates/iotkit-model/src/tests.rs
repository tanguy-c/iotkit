use embassy_futures::block_on;

use super::*;

static TEMP: Sensor = Sensor::new("temperature", "Temperature").unit("°C").device_class("temperature").measurement();
static RELAY: Switch = Switch::new("relay", "Relay");
static LAMP: Light = Light::new("led", "LED");
static RESTART: Button = Button::new("restart", "Restart").device_class("restart").diagnostic();
static DOOR: BinarySensor = BinarySensor::new("door", "Door").device_class("door");
static ENTITIES: [&dyn Entity; 5] = [&TEMP, &RELAY, &LAMP, &RESTART, &DOOR];
static DEVICE: Device =
    Device { name: "Test", manufacturer: "iotkit", model: "devkit", sw_version: "1.0.0", entities: &ENTITIES };

#[test]
fn metadata() {
    let m = TEMP.meta();
    assert_eq!((m.kind, m.unit, m.device_class, m.measurement), (Kind::Sensor, Some("°C"), Some("temperature"), true));
    assert_eq!(RESTART.meta().category, Category::Diagnostic);
    assert_eq!(DEVICE.entity("relay").map(|e| e.meta().kind), Some(Kind::Switch));
    assert!(DEVICE.entity("nope").is_none());
}

#[test]
fn sensors_report_changes_only() {
    static S: Sensor = Sensor::new("s", "S");
    assert_eq!(S.state(), State::Unknown);
    assert!(!S.take_dirty(Binding::MQTT));
    S.set(1.5);
    assert!(S.take_dirty(Binding::MQTT) && !S.take_dirty(Binding::MQTT));
    // Each binding sees the change once.
    assert!(S.take_dirty(Binding::MATTER) && !S.take_dirty(Binding::MATTER));
    S.set(1.5);
    assert!(!S.take_dirty(Binding::MQTT), "same value: nothing to publish");
    S.mark_dirty(Binding::MATTER);
    assert!(!S.take_dirty(Binding::MQTT) && S.take_dirty(Binding::MATTER), "marked for one binding only");
    assert_eq!(S.state(), State::Number(1.5));
    S.set(f32::NAN);
    assert_eq!(S.state(), State::Unknown);
    DOOR.set(true);
    assert_eq!(DOOR.state(), State::Bool(true));
}

#[test]
fn switch_commands_reach_the_app() {
    assert!(RELAY.command(Command::Set(true)));
    assert!(block_on(RELAY.wait_command()));
    assert_eq!(RELAY.state(), State::Unknown, "a command is not a state");
    RELAY.set(true);
    assert_eq!(RELAY.state(), State::Bool(true));
    assert!(!RELAY.command(Command::Press));
}

#[test]
fn light_commands_merge_with_the_state() {
    LAMP.set(LightState { on: false, brightness: 100, rgb: [255, 0, 0] });
    LAMP.command(Command::Light { on: Some(true), brightness: None, rgb: None });
    assert_eq!(block_on(LAMP.wait_command()), LightState { on: true, brightness: 100, rgb: [255, 0, 0] });
    LAMP.command(Command::Light { on: None, brightness: None, rgb: Some([0, 0, 255]) });
    assert_eq!(block_on(LAMP.wait_command()), LightState { on: true, brightness: 100, rgb: [0, 0, 255] });
    LAMP.command(Command::Set(false));
    assert!(!block_on(LAMP.wait_command()).on);
}

#[test]
fn commands_before_the_app_applies_them_add_up() {
    // Matter sends level and colour as separate commands; the app sees only the latest signal.
    static L: Light = Light::new("l", "L");
    L.set(LightState { on: false, brightness: 100, rgb: [255, 0, 0] });
    L.command(Command::Light { on: Some(true), brightness: Some(40), rgb: None });
    L.command(Command::Light { on: None, brightness: None, rgb: Some([0, 0, 255]) });
    let wanted = LightState { on: true, brightness: 40, rgb: [0, 0, 255] };
    assert_eq!(L.target(), State::Light(wanted));
    assert_eq!(L.state(), State::Light(LightState { on: false, brightness: 100, rgb: [255, 0, 0] }));
    assert_eq!(block_on(L.wait_command()), wanted);
    // Taken but not applied yet: the next command still builds on it.
    L.command(Command::Set(false));
    assert_eq!(block_on(L.wait_command()), LightState { on: false, ..wanted });
    // Reported: the state is the base again.
    L.set(LightState { on: true, brightness: 7, rgb: [1, 1, 1] });
    assert_eq!(L.target(), L.state());

    static S: Switch = Switch::new("s", "S");
    S.set(false);
    for _ in 0..2 {
        // A toggle: the opposite of the target.
        let on = S.target() != State::Bool(true);
        S.command(Command::Set(on));
    }
    assert_eq!(S.target(), State::Bool(false), "two toggles in a row cancel out");
    S.set(true);
    assert_eq!(S.target(), State::Bool(true));
}

#[test]
fn light_state_rules() {
    let off = LightState { on: false, brightness: 0, rgb: [1, 2, 3] };
    assert_eq!(off.apply(Some(true), None, None).brightness, 255, "on at 0 means full");
    assert!(!off.apply(None, Some(0), None).on, "brightness 0 is off");
    assert_eq!(off.apply(None, Some(10), None), LightState { on: true, brightness: 10, rgb: [1, 2, 3] });
    assert!(!off.apply(Some(false), Some(10), None).on, "explicit off wins");
}

#[test]
fn buttons() {
    assert!(RESTART.command(Command::Press));
    block_on(RESTART.wait_press());
    assert_eq!(RESTART.state(), State::None);
}
