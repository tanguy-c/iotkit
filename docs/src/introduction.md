# iotkit

iotkit is a `no_std` Rust firmware kit for ESP32 smart devices, on esp-hal and Embassy. A
device's `main` lists its entities and its own hardware, and calls `iotkit::start` for the rest:

- Wi-Fi setup from a phone (setup access point, Improv over Bluetooth or USB);
- signed OTA updates, with automatic rollback when a new firmware is not healthy;
- Matter over Wi-Fi or Thread, with any Matter controller; Home Assistant over its ESPHome
  integration (native API) or MQTT (discovery), and any MQTT broker;
- crash records, watchdogs, a USB console;
- deep sleep for battery devices.

```rust,ignore
static LED: Light = Light::new("led", "LED");
static TEMPERATURE: Sensor = Sensor::temperature("temperature", "Temperature");
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY)).entities(&[&LED, &TEMPERATURE]);

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit::init!(p);
    iotkit::start(spawner, Board { wifi: p.WIFI, /* bt, usb, ... */ }, &CONFIG).await;
    // the device's own hardware and tasks
}
```

## iotkit and ESPHome

[ESPHome](https://esphome.io) is the easiest way to a Home Assistant device, and has hundreds of
components. iotkit is for the next step: when YAML and lambdas get in the way, or a product needs
code you own.

| | ESPHome | iotkit |
| --- | --- | --- |
| A device is | YAML, C++ lambdas | Rust: entities as `static`s, async tasks |
| Parts | hundreds of components | [ready-made parts](components.md), and any embedded-hal driver crate |
| Home Assistant | native API | native API (`esphome-api`), MQTT discovery; Matter |
| Updates | OTA with a password | Ed25519-signed, rolled back if the new firmware is not healthy |
| Setup | captive portal, Improv | setup access point, Improv over Bluetooth and USB |
| Chips | the ESP32 family and more | ESP32-C6 (reference), C3, C5, S3, ESP32; H2 for Thread |
| Memory safety | C++ | Rust |

[Coming from ESPHome](coming-from-esphome.md) maps each YAML block to its iotkit code.

## Status

Early (0.x): the API and the settings' flash layout may still change.

- Tested on an ESP32-C6-DevKitC-1: Wi-Fi setup (access point, Improv over Bluetooth and serial),
  signed OTA with rollback, MQTT with Home Assistant, Matter over Wi-Fi with Home Assistant's
  Matter server, deep sleep, updates from Home Assistant, installing from the browser, Home
  Assistant's ESPHome integration, MQTT and updates over TLS, Matter over Thread through a Thread
  border router (paired with one controller, shared with Home Assistant's Matter server, updated
  over Matter), the status LED and the BOOT button's gestures.
- Tested on a Seeed Studio XIAO ESP32-C6 (4 MB): Wi-Fi (its antenna switch), MQTT, OTA, the
  ESPHome API, Matter over Thread, deep sleep over MQTT and the ESPHome API.
- Tested on an ESP32-S3-DevKitC-1 (v1.1, 8 MB): Wi-Fi, MQTT, OTA, the setup window, the ESPHome
  API, the status LED.
- Built but not tested on hardware: the C3, C5, ESP32 and H2, most entity kinds over Matter.
- Matter uses the test attestation credentials: for development, not for selling devices.

## Chips

| Chip | Wi-Fi, MQTT, OTA, setup | Matter | Tested on hardware |
| --- | --- | --- | --- |
| ESP32-C6 | yes | Wi-Fi (`matter-wifi`, 8 MB), Thread (`matter-thread`, 4 MB is enough) | yes (ESP32-C6-DevKitC-1, 8 MB; XIAO ESP32-C6, 4 MB) |
| ESP32-C3 | yes | no: Matter needs about 24 KB more RAM than the C3 has | build only |
| ESP32-C5 | yes | no: esp-hal 1.2 has no true RNG on the C5 yet | build only |
| ESP32-S3 | yes | no: does not fit in internal RAM (PSRAM support later) | yes (ESP32-S3-DevKitC-1 v1.1, 8 MB) |
| ESP32 | yes, except setup over Bluetooth; console and Improv on UART0; no deep sleep yet | no: not tried (RAM, rs-matter support) | build only |
| ESP32-H2 | no Wi-Fi | Thread (`matter-thread`, `matter-ota` included; its `init!` heap 32 KB smaller) | build only |

The ESP32-C6 is the reference: support for other chips is added without changing it.
