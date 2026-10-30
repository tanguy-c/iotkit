# Changelog

The API and the settings' flash layout may still change before 1.0. Breaking changes come with
the steps to apply them in [Upgrading](docs/src/upgrading.md).

## Unreleased

- Less RAM without Wi-Fi: `init!`'s heap is only the 64 KB reclaimed from the bootloader
  (`DEFAULT_HEAP` is 0 without `wifi`; Bluetooth and Thread fit), and the Matter stack's
  allocations are sized to their measured use: Matter over Thread on the C6 has about 100 KB more
  stack.
- Matter over Thread: a device whose last pairing a controller removes forgets the Thread network,
  instead of joining it while pairing over Bluetooth on the same radio (a busy loop that also
  ignored the BOOT button).
- Adafruit Feather ESP32-C6 (`cargo iotkit new`): GPIO20 high at boot, powering the NeoPixel and
  the STEMMA QT port.
- `cargo iotkit new` writes one module per part, `src/parts/<entity id>.rs`, and the board's LED in
  `src/led.rs`; `src/main.rs` keeps the setup and the lists of parts. `iotkit.toml` keeps the
  answers (without the secrets).
- `cargo iotkit add [<component>] [--name] [--pin [ROLE=]GPIO]` adds a part to such a project, and
  `cargo iotkit components` lists them. The parts are files in `components/`, one each, like
  `boards/`: a part with several pins (`pins = { sda = 4, scl = 5 }` in an answers file) fits.
- I²C sensor chips, ready-made: BH1750, SHT4x, SHT3x, AHT20, BME280, BMP280 and SCD4x
  (`components::i2c`, one reading per call). The project's I²C bus is asked once (`[i2c]`, `sda`
  and `scl`) and shared; a chip measuring several values is one part with an entity each; two
  chips at one address are refused (`address = 0x45`).
- On a battery device, ready-made readings too: once per wake-up, the I²C chips, the contact, leak,
  smoke and motion inputs, and the soil probe (on the ADC after the battery level).
- A Matter ICD (Thread on battery) reads its sensors every `report_every_min` (asked now), not
  every 30 s: each report wakes its radio.

## 0.4.0 (2026-10-27)

- **Breaking:** MQTT is the `mqtt` feature (on by default): a project with
  `default-features = false` that uses it adds it. `provision::esp::{dhcp_task, dns_task,
  portal_task}` are gone: `window_task` runs them, only while a setup window is open
  ([Upgrading](docs/src/upgrading.md)).
- Updates without a click, on every link: `IOTKIT_AUTO_UPDATE` (`on`/`off`). Always-on Wi-Fi
  devices get an *Auto-update* switch, and *Update time* over the ESPHome API; turning
  *Auto-update* on or changing *Update time* checks at once (battery devices included, which
  waited for the next check, up to 6 hours). Over Matter, `off` turns the daily provider checks
  off ([Updates](docs/src/updates.md#without-a-click)).
- `IOTKIT_UPDATE_TIME` over Matter: the daily checks' updates download at once and install at
  that time, in the controller's time and time zone (Time Synchronization, which Home Assistant's
  Matter Server sends with `time_sync`); without it after a restart, an hour later. Home Assistant
  sets no default provider: its updates install when clicked, at once.
- Matter updates: a provider that stops answering (a lost network) no longer holds every later
  check; each exchange with it gives up after 2 minutes.
- Less RAM: downloads, update installs and the setup access point share one work area, 11 KB more
  free. An update asked for during a setup window is refused (`ota push`: HTTP 503). `mem` on the
  console and `diag::esp::memory()` show heap and stack use.
- Matter over Thread fits 4 MB boards, and links on the ESP32-H2, updates included: the H2's
  default heap past the reclaimed 64 KB is 32 KB (`DEFAULT_HEAP`; the other chips keep 96 KB).
- The status shows on the board's RGB LED, over the light (`components::rgb_status_light`, or
  `components::Overlay` for an app's own LED), and follows the links live. A short press of the
  setup button closes an open setup or Matter pairing window.
- `cargo iotkit new`:
  - reads its boards from [`boards/`](boards/), one file per board: 23 new boards (Espressif,
    Seeed, M5Stack, Adafruit, Waveshare, LOLIN, Unexpected Maker, Arduino, Olimex), searched as
    you type; the status default is the board's LED;
  - asks about updates (none, for a smaller firmware; by themselves), the Wi-Fi setup ways and the
    console, and leaves out the features not wanted;
  - warns when a Matter project has no part, and names no ecosystem.
- Docs: [Building a Matter device](docs/src/matter.md) (Wi-Fi or Thread, pairing with any Matter
  controller, updates, what certification takes); tested on a XIAO ESP32-C6 and an
  ESP32-S3-DevKitC-1 too.
- Fixed: an MQTT number command with a huge negative exponent (`1.1e-50000000000017`) restarted
  the device.

## 0.3.0 (2026-10-26)

- **Breaking**, only for an app with its own entity type: `model::Meta` has a `remember` field
  ([Upgrading](docs/src/upgrading.md)).
- Entities that keep their state across restarts (`remember` feature): a switch, light, number
  or select built with `.remember()` is saved to flash once it settles and handed back to the app
  as its first command ([Entities](docs/src/entities.md#keeping-their-state)). `cargo iotkit new`
  projects remember their status LED, and write it before `start` (it no longer shows a reset's
  green for two seconds, then the default colour); `apps/thread-light` and `apps/showcase` use it.
- MQTT number commands no longer link core's float parser: about 17 KB less code and constants.
- A connected station waits for esp-radio's disconnect event instead of checking its link every
  2 s (every 30 s, for a missed event; the signal strength too).
- Matter over Thread: a paired device now passes its network health check (the attach could
  wake another task instead), so an update over Matter is kept, not rolled back after 30 minutes.
- Docs: the C6's LP core overwrites iotkit's RTC records; not supported yet
  ([Battery devices](docs/src/battery.md#the-c6s-low-power-core-not-with-iotkit-yet)).
- Matter's crypto: `crypto-bigint` 0.7.9 and `ctutils` 0.4.6, for operations meant to be
  constant-time that were not (RUSTSEC-2026-0387, RUSTSEC-2026-0385). Matter over Wi-Fi grows by
  about 2.3 KB.

## 0.2.0 (2026-10-25)

- **Breaking:** `init!` takes the flash (`Board` has no `flash` field) and `start` needs it;
  `matter_task` takes the product name and the `Firmware` instead of a `Product`
  ([Upgrading](docs/src/upgrading.md)).
- Matter over Thread: `iotkit::start` (`matter-thread`, its own `Board`), and `cargo iotkit new`
  makes Thread projects (ESP32-C6, or an 8 MB ESP32-H2). `apps/thread-light` is now its output;
  `apps/ble-beacon` assembles the parts by hand.
- Updates over Matter (`matter-ota`), without Wi-Fi: signed `.ota` files (`cargo iotkit ota
  build`) from a Matter OTA provider; `matter-ota-unsigned` takes bare images, checked by SHA-256
  ([Updates](docs/src/updates.md#over-matter-thread-wi-fi)).
- More updates are rolled back: one never healthy after `IOTKIT_UPDATE_TIMEOUT` (30 min), one
  that cannot start Wi-Fi or Thread, one that crashes right after `init!`. The health check
  follows the network (`iotkit::health`): Thread, a Matter controller, or Home Assistant over the
  ESPHome API. While an update is on trial, other installs are refused and a battery device stays
  awake ([Updates](docs/src/updates.md#what-the-device-does)).
- Battery devices ([Battery devices](docs/src/battery.md)): deep sleep over the ESPHome API,
  Matter over Thread as a sleepy end device (`matter-icd`; the receiver stays on between polls for
  now), and `power = "battery"` for both in `cargo iotkit new`. A pairing mode (at power-on until
  linked, or a 3 s hold), wake-up on a button, and, over the ESPHome API, updates installed at a
  set time (`IOTKIT_UPDATE_TIME`, 02:00).
- Status LEDs: `iotkit::status`, driven by `components::status_led` or `status_rgb` (WS2812)
  ([Components](docs/src/components.md#status-led)); `cargo iotkit new` asks for one and for the
  button's pin.
- Fixed: a second setup window panicked; Home Assistant's ESPHome integration failed to add a
  device at the first try, and showed "Update available" with no version before the first check;
  a crash before the boot overwrote the previous crash record. The setup button is
  interrupt-driven, and over the ESPHome API the firmware shows as "1.2.0 (ESPHome iotkit 0.2.0)".

## 0.1.1 (2026-10-21)

- `ha-update` without `ota-push`: updates from Home Assistant now install (they waited forever);
  the console's `ota URL` says when no update feature is built in.

## 0.1.0 (2026-10-21)

The first release. Install from git (not on
crates.io yet, see the [roadmap](docs/src/roadmap.md)); tested on an ESP32-C6-DevKitC-1.

- One crate for devices, `iotkit` (was `iotkit-app` and ten layer crates), plus `iotkit-core`
  (chip-independent, host-tested), `iotkit-image` (signed images, shared with the CLI) and
  `iotkit-build`. Paths: `iotkit_app::` is `iotkit::`, the
  layers are modules (`iotkit::net`, `iotkit::ota`, `iotkit::mqtt` (was `iotkit_link_mqtt`),
  `iotkit::matter` (was `iotkit_link_matter`), ...). Matter over Thread: `matter-thread` with
  `default-features = false`.
- Licensed under MIT or Apache-2.0 (was Apache-2.0).
- Opt-out features (`improv-ble`, `softap`, `improv-serial`, `console`, `ota-push`), and
  `IOTKIT_HOSTNAME` / `IOTKIT_TOPIC_PREFIX`.
- `components`: ready-made parts (relay, binary input, button, dimmer, fan, RGB light, polled
  sensors with filters); `cargo iotkit new` uses the relay, input, button and polled ones.
- `init!`, `Config::new` with builders, typed `DeviceClass` / `Unit`, sensor shortcuts.
- `cargo iotkit new`: `TODO(<id>)` hints, optional `AGENTS.md` for AI coding agents.
- `tls`: TLS 1.3 for MQTT (port 8883) and `https://` downloads, with the server checked against a
  built-in CA.
- `esphome-api`: Home Assistant's ESPHome integration over the native API (TCP 6053, Noise
  encryption, mDNS discovery), every entity kind, no broker. `model::Binding::allocate`
  (`extra-bindings`) for transports outside iotkit.
- The original ESP32 (MQTT, console and Improv over UART0; no Bluetooth setup or deep sleep yet).
- Fuzz targets for the network-facing parsers (`fuzz/`), and `tools/hil/run.py`, a hardware test
  (flash, healthy, OTA kept, OTA rolled back) with a nightly workflow for a self-hosted runner.
- The guide (`docs/`, GitHub Pages) with a "Coming from ESPHome" page; a short README.
- Install from the browser: `cargo iotkit build` writes an ESP Web Tools page and merged image
  (`dist/<package>/web/`); `cargo iotkit web` serves it.
