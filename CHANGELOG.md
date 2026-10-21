# Changelog

The API and the settings' flash layout may still change before 1.0.

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
