# Changelog

The API and the settings' flash layout may still change before 1.0. Breaking changes come with
the steps to apply them in [Upgrading](docs/src/upgrading.md).

## Unreleased

- MQTT number commands no longer link core's float parser: about 17 KB less code and constants.
- A connected station waits for esp-radio's disconnect event instead of checking its link every
  2 s (every 30 s, for a missed event; the signal strength too).
- Matter over Thread: a paired device now passes its network health check (the attach could
  wake another task instead), so an update over Matter is kept, not rolled back after 30 minutes.
- Docs: the C6's LP core overwrites iotkit's RTC records; not supported yet
  ([Battery devices](docs/src/battery.md#the-c6s-low-power-core-not-with-iotkit-yet)).

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
