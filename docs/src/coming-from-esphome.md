# Coming from ESPHome

An ESPHome YAML file describes a device; an iotkit device is a small Rust program that does the
same. `cargo iotkit new` writes most of it from the same questions YAML answers.

## The same device, both ways

A pump relay, a door contact and a soil probe, over MQTT:

```yaml
esphome:
  name: garden
esp32:
  board: esp32-c6-devkitc-1
wifi:
  ap: {}
captive_portal:
mqtt:
  broker: 192.168.1.10
ota:
  - platform: esphome
switch:
  - platform: gpio
    pin: GPIO4
    name: Pump
binary_sensor:
  - platform: gpio
    pin: { number: GPIO5, mode: INPUT_PULLUP }
    name: Door
    device_class: door
sensor:
  - platform: adc
    pin: GPIO2
    name: Soil moisture
    update_interval: 30s
    filters:
      - calibrate_linear: [2.4 -> 0, 1.0 -> 100]
```

```rust,ignore
static PUMP: Switch = Switch::new("pump", "Pump");
static DOOR: BinarySensor = BinarySensor::new("door", "Door").device_class(DeviceClass::DOOR);
static SOIL: Sensor = Sensor::new("soil_moisture", "Soil moisture").unit(Unit::PERCENT).measurement();

static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY)).entities(&[&PUMP, &DOOR, &SOIL]);

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit::init!(p);
    let board = Board { wifi: p.WIFI, bt: p.BT, timg1: p.TIMG1, usb: p.USB_DEVICE, boot_button: Some(p.GPIO9.into()) };
    iotkit::start(spawner, board, &CONFIG).await;

    spawner.spawn(pump_task(Output::new(p.GPIO4, Level::Low, OutputConfig::default())).unwrap());
    spawner.spawn(door_task(Input::new(p.GPIO5, InputConfig::default().with_pull(Pull::Up))).unwrap());
    let mut adc_config = AdcConfig::new();
    let soil_pin = adc_config.enable_pin(p.GPIO2, Attenuation::_11dB);
    spawner.spawn(soil_task(Adc::new(p.ADC1, adc_config).into_async(), soil_pin).unwrap());
    loop {
        Timer::after_secs(60).await;
    }
}

#[embassy_executor::task]
async fn pump_task(pin: Output<'static>) {
    components::relay(&PUMP, pin, Active::High).await
}

#[embassy_executor::task]
async fn door_task(pin: Input<'static>) {
    components::binary_input(&DOOR, pin, Active::High, Duration::from_millis(50)).await
}

#[embassy_executor::task]
async fn soil_task(mut adc: Adc<'static, ADC1<'static>, Async>, mut pin: AdcPin<GPIO2<'static>, ADC1<'static>>) {
    // Raw 12-bit readings: 3000 dry, 1200 in water.
    let percent = Linear::new((3000.0, 0.0), (1200.0, 100.0)).then(Clamp::new(0.0, 100.0));
    components::poll(&SOIL, Duration::from_secs(30), percent, async || {
        Some(f32::from(adc.read_oneshot(&mut pin).await))
    })
    .await
}
```

Wi-Fi setup, MQTT, signed OTA, mDNS, crash records and the console come with `start`; the broker
is set once on the console (`mqtt 192.168.1.10`) or at build time (`MQTT_HOST`).

## Block by block

| ESPHome | iotkit |
| --- | --- |
| `esphome: name:` | `IOTKIT_HOSTNAME`, the host name prefix and the device in Home Assistant ([names](lean.md#names)); the package name for updates and Matter |
| `esp32: board:` | the board question of `cargo iotkit new`; a chip feature (`esp32c6`, ...) |
| `wifi:`, `ap:`, `captive_portal:` | the setup window of `start` (feature `softap`) |
| `esp32_improv:`, `improv_serial:` | features `improv-ble`, `improv-serial` |
| `api:` + `encryption:` | the `esphome-api` feature; or MQTT discovery, or Matter ([Home Assistant](home-assistant.md)) |
| `mqtt:` | `mqtt HOST` on the console, or `MQTT_HOST` at build time |
| `ota:` | `cargo iotkit ota push`, signed; or updates from Home Assistant ([updates](updates.md)) |
| `logger:` | `ESP_LOG` at build time, the USB console ([console](console.md)) |
| `switch: platform: gpio` | `components::relay` |
| `binary_sensor: platform: gpio` | `components::binary_input` (debounced) |
| `binary_sensor:` `on_multi_click:` | `components::button`: press, double, triple, long |
| `sensor: platform: <chip>` | `components::poll` with the driver crate ([drivers](components.md#any-embedded-hal-driver)) |
| `filters:` | `components::filter`: `Linear`, `Clamp`, `MovingAverage`, `Delta`, `Offset`, `Multiply`, closures |
| `light: platform: monochromatic` | `components::dimmer` on a LEDC channel |
| `light: platform: esp32_rmt_led_strip` | `components::rgb_light` (feature `smartled`) |
| `fan: platform: speed` | `components::fan` |
| `interval:` | a task with a `Ticker` |
| `on_boot:` | code in `main` after `start` |
| `globals:` with `restore_value` | a [`Setting`](entities.md#settings), `App::load` / `App::save` |
| `substitutions:` | `const`s |
| `lambda:` | plain Rust, anywhere |
| `deep_sleep:` | feature `deep-sleep`, `start_sleepy` ([battery devices](battery.md)) |
| `packages:` | modules and crates |
| `web_server:` | none: Home Assistant and the console |

## What is different

- **Types instead of strings.** Device classes and units are constants (`DeviceClass::DOOR`,
  `Unit::PERCENT`): a typo does not build.
- **Tasks instead of triggers.** Behaviour is async code: wait for a command or an edge, do
  something, report the state. Automations that belong to the device live there; the rest stays
  in Home Assistant.
- **Updates are signed.** A device installs only images signed with your key, and goes back to
  the previous firmware by itself if the new one does not become healthy.
- **What you leave out is not in the firmware.** Unused parts and services are not linked
  ([only what the device uses](lean.md)).
