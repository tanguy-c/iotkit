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

iotkit is early (0.x): the API and the settings' flash layout may still change before 1.0. When a
release breaks something, [Upgrading](upgrading.md) gives the steps.

Matter uses the test attestation credentials. Use it for development, not to sell devices
([why](matter.md#before-selling-a-device)).

## Chips

| Chip | Wi-Fi, MQTT, OTA, setup | Matter |
| --- | --- | --- |
| ESP32-C6 | yes | Wi-Fi (`matter-wifi`, 8 MB), Thread (`matter-thread`, 4 MB is enough) |
| ESP32-C3 | yes | no: Matter needs about 24 KB more RAM than the C3 has |
| ESP32-C5 | yes | no: esp-hal 1.2 has no true RNG on the C5 yet |
| ESP32-S3 | yes | no: does not fit in internal RAM (PSRAM support later) |
| ESP32 | yes, except setup over Bluetooth; console and Improv on UART0; no deep sleep yet | no: not tried (RAM, rs-matter support) |
| ESP32-H2 | no Wi-Fi | Thread (`matter-thread`, `matter-ota` included; without Wi-Fi, `init!`'s heap is the reclaimed 64 KB only) |

The ESP32-C6 is the reference: support for other chips is added without changing it.

### Tested on hardware

Every chip builds in CI. This table lists the boards someone ran iotkit on, and the last version
they tested. A release updates the rows it tested. To add your board, open an issue or a pull
request.

| Board | Last tested | What was tested |
| --- | --- | --- |
| ESP32-C6-DevKitC-1 (8 MB) | 0.4.0 | Wi-Fi setup (access point, Improv over Bluetooth and serial); signed OTA with rollback; MQTT and the ESPHome API with Home Assistant, over TLS too; updates from Home Assistant; Matter over Wi-Fi; Matter over Thread through a border router (pairing, multi-admin, updates over Matter); deep sleep; install from the browser; status LED and BOOT button |
| Seeed Studio XIAO ESP32-C6 (4 MB) | 0.5.1 | Wi-Fi (its antenna switch), MQTT, OTA, the ESPHome API; Matter over Thread with Google Home; deep sleep over MQTT and the ESPHome API (about 110 wake-ups) |
| ESP32-S3-DevKitC-1 v1.1 (8 MB) | 0.4.0 | Wi-Fi, MQTT, OTA, the setup window, the ESPHome API, status LED |
| ESP32-C3, C5, ESP32, H2 | not yet | build only |

Most entity kinds over Matter are not tested on hardware yet.
