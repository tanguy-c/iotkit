# Changelog

iotkit is not released yet: the API and the settings' flash layout may still change.

## Unreleased

- One crate for devices, `iotkit` (was `iotkit-app` and ten layer crates), plus `iotkit-core`
  (chip-independent, host-tested) and `iotkit-build`. Paths: `iotkit_app::` is `iotkit::`, the
  layers are modules (`iotkit::net`, `iotkit::ota`, `iotkit::mqtt` (was `iotkit_link_mqtt`),
  `iotkit::matter` (was `iotkit_link_matter`), ...). Matter over Thread: `matter-thread` with
  `default-features = false`.
- Licensed under MIT or Apache-2.0 (was Apache-2.0).
- Opt-out features (`improv-ble`, `softap`, `improv-serial`, `console`, `ota-push`), and
  `IOTKIT_HOSTNAME` / `IOTKIT_TOPIC_PREFIX`.
- `components`: ready-made parts (relay, binary input, button, dimmer, fan, RGB light, polled
  sensors with filters), used by `cargo iotkit new`.
- `init!`, `Config::new` with builders, typed `DeviceClass` / `Unit`, sensor shortcuts.
- `cargo iotkit new`: `TODO(<id>)` hints, optional `AGENTS.md` for AI coding agents.
