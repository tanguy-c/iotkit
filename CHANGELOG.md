# Changelog

This file lists the notable changes to iotkit, newest first. It follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/): before 1.0, a breaking change raises the minor version
(0.5 to 0.6). Each breaking change comes with its steps in [Upgrading](docs/src/upgrading.md).

## [Unreleased]

### Changed

- The guide lists the boards iotkit was tested on, with the last version tested and what was
  covered ([Tested on hardware](docs/src/introduction.md#tested-on-hardware)).
- The hardware notes for contributors moved to
  [Working on iotkit](docs/src/contributing.md#hardware-notes), and the ones that users meet to
  [Troubleshooting](docs/src/troubleshooting.md).

### Removed

- The roadmap page.

## [0.5.1] - 2026-10-31

Update if a Wi-Fi device sometimes fails to join its network, or a Thread device is slow to come
back after a reset. You don't need to change anything in your project.

### Fixed

- Wi-Fi: some access points ignored the device's join requests, and a third of the wake-ups of a
  battery device needed a retry (`AuthenticationExpired`). The station now transmits at 13 dBm
  instead of esp-radio's 5 dBm. On a XIAO ESP32-C6 waking every 10 to 30 s, no wake-up needs a
  retry now.
- Matter over Thread: after a reset, the device's parent kept a stale entry for it. The device's
  802.15.4 address now comes from the chip instead of a new random draw at each boot, so its
  parent recognizes it.

## [0.5.0] - 2026-10-30

The values you build in now reach the device at each flash, and seven I²C sensors are
ready-made. Two breaking changes: the console is opt-in, and the update URL is built in only
([Upgrading](docs/src/upgrading.md)).

### Changed

- **Breaking:** a value you build in (`.cargo/config.toml`, `.cargo/secrets.toml`) now reaches the
  device at each flash or update. Before, the first boot copied it to flash, and later builds'
  values were ignored. A value given on the device (setup window, console) still wins until a
  factory reset. To erase those values as you flash, run `cargo iotkit flash --factory-reset`
  ([where settings come from](docs/src/getting-started.md#where-settings-come-from)).
- **Breaking:** the update URL is built in only, with `OTA_MANIFEST_URL`. The `update-url` console
  command and `ota::UpdateSource` are gone.
- **Breaking:** the USB console is opt-in. To keep it, add the `console` feature. You no longer
  need it to set up a device:
  - the setup page takes the MQTT broker (*MQTT broker*);
  - an app can set the broker with `mqtt::esp::set_broker`, or show the ESPHome API's key with
    `esphome::api_key()`;
  - the device logs the broker it uses at boot.
- Less RAM and flash:
  - The settings store does its flash work in one place, instead of in every task that loads or
    saves a setting. A Wi-Fi device saves about 4.5 KB of RAM and 13 KB of code, a Thread device
    2 KB and 16 KB.
  - Without Wi-Fi, `init!`'s heap is only the 64 KB reclaimed from the bootloader (`DEFAULT_HEAP`
    is 0, and `init!(p, heap: ...)` adds more). The Matter stack's allocations are sized to their
    measured use, so Matter over Thread on the C6 has about 100 KB more stack.
- Battery devices: the ready-made parts read once per wake-up (the I²C chips, the contact, leak,
  smoke and motion inputs, the soil probe). A Matter ICD reads its sensors every
  `report_every_min` instead of every 30 s, because each report wakes its radio.
- `cargo iotkit new`:
  - A project has one module per part (`src/parts/<entity id>.rs`) and the board's LED in
    `src/led.rs`. `iotkit.toml` keeps your answers, `.cargo/config.toml` the broker and update
    URL, and `.cargo/secrets.toml` only the secrets.
  - The console is a question, and the default answer is no.

### Added

- Ready-made I²C sensor chips: BH1750, SHT4x, SHT3x, AHT20, BME280, BMP280 and SCD4x
  (`components::i2c`, one reading per call).
- `cargo iotkit add [<component>] [--name] [--pin [ROLE=]GPIO]` adds a part to a project, and
  `cargo iotkit components` lists the parts. Each part is one file in [`components/`](components/),
  like the boards, and can have several pins (`pins = { sda = 4, scl = 5 }`). The wizard asks for
  the I²C bus once and shares it, and refuses two chips at the same address.

### Fixed

- The setup page closed by itself on a phone after about 30 s. Opening the window restarts the
  radio, and the station then searched every channel, taking the access point off its channel
  each time. It now searches the access point's channel first.
- Wi-Fi: a device stayed on a network that never gave it an address. If no address comes within
  30 s, it now leaves and joins again, backing off like after a failed join.
- ESPHome API: Home Assistant mistook a device for another one ("Unexpected device found",
  "Invalid encryption key") and did not match it with its discovery. The handshake sent the MAC
  with colons, which Home Assistant cuts to 16 characters. It now sends it as ESPHome does
  (`58e6c500fd20`).
- Matter over Thread: when a controller removed a device's last pairing, the device kept joining
  the Thread network while pairing over Bluetooth on the same radio. That busy loop also ignored
  the BOOT button. The device now forgets the network.
- Console: a typed `I` (the end of a base64 key) was taken as the start of an Improv frame and
  lost. It is passed on when no frame follows.
- Adafruit Feather ESP32-C6: GPIO20 is set high at boot, which powers the NeoPixel and the
  STEMMA QT port.

## [0.4.0] - 2026-10-27

Updates can install by themselves, Matter over Thread fits 4 MB boards and the ESP32-H2, and
`cargo iotkit new` knows 23 more boards. If you build without default features, MQTT is now a
feature to add ([Upgrading](docs/src/upgrading.md)).

### Changed

- **Breaking:** MQTT is the `mqtt` feature, on by default. If your project sets
  `default-features = false` and uses MQTT, add the feature.
- **Breaking:** `provision::esp::{dhcp_task, dns_task, portal_task}` are gone. `window_task` runs
  them, only while a setup window is open.
- Less RAM: downloads, update installs and the setup access point share one work area, which
  frees 11 KB. While a setup window is open, the device refuses updates (`ota push` gets HTTP
  503).
- Matter updates: a provider that stops answering, after a lost network for example, no longer
  blocks every later check. Each exchange with it gives up after 2 minutes.
- `cargo iotkit new` asks about updates (none for a smaller firmware, or automatic), the Wi-Fi
  setup methods and the console, and leaves out the features you don't want. It warns when a
  Matter project has no part, and names no ecosystem.

### Added

- Automatic updates on every link, with `IOTKIT_AUTO_UPDATE` (`on` or `off`)
  ([Updates](docs/src/updates.md#without-a-click)):
  - Always-on Wi-Fi devices get an *Auto-update* switch, and an *Update time* over the ESPHome
    API. Turning *Auto-update* on or changing *Update time* checks at once. Battery devices used
    to wait for their next check, up to 6 hours.
  - Over Matter, `off` stops the daily checks with the provider.
- `IOTKIT_UPDATE_TIME` over Matter: an update found by the daily check downloads at once and
  installs at that time, in the controller's time zone. The time comes from the controller (Time
  Synchronization, which Home Assistant's Matter Server sends with `time_sync`). Without it after
  a restart, the update installs an hour later. Home Assistant sets no default provider, so its
  updates install at once, when you click.
- Matter over Thread fits 4 MB boards, and builds on the ESP32-H2 with updates. On the H2, the
  default heap beyond the reclaimed 64 KB is 32 KB (`DEFAULT_HEAP`); other chips keep 96 KB.
- The status shows on the board's RGB LED, on top of the light (`components::rgb_status_light`,
  or `components::Overlay` for your own LED), and follows the links live. A short press of the
  setup button closes an open setup or Matter pairing window.
- `mem` on the console and `diag::esp::memory()` show heap and stack use.
- `cargo iotkit new` reads its boards from [`boards/`](boards/), one file per board, and searches
  them as you type. It knows 23 new boards from Espressif, Seeed, M5Stack, Adafruit, Waveshare,
  LOLIN, Unexpected Maker, Arduino and Olimex. The status LED defaults to the board's LED.
- The guide has a [Building a Matter device](docs/src/matter.md) page: Wi-Fi or Thread, pairing
  with any Matter controller, updates, and what certification takes.

### Fixed

- An MQTT number command with a huge negative exponent (`1.1e-50000000000017`) restarted the
  device.

## [0.3.0] - 2026-10-26

Switches, lights, numbers and selects can keep their state across restarts. The breaking change
only affects an app that defines its own entity type ([Upgrading](docs/src/upgrading.md)).

### Added

- Entities that keep their state across restarts (`remember` feature). A switch, light, number or
  select built with `.remember()` is saved to flash once it settles, and given back to the app as
  its first command ([Entities](docs/src/entities.md#keeping-their-state)). Projects from
  `cargo iotkit new` remember their status LED and set it before `start`, so it no longer shows a
  reset's green for two seconds. `apps/thread-light` and `apps/showcase` use it.
- The guide explains why the C6's LP core cannot be used with iotkit yet: it overwrites iotkit's
  RTC records
  ([Battery devices](docs/src/battery.md#the-c6s-low-power-core-not-with-iotkit-yet)).

### Changed

- **Breaking**, only if your app defines its own entity type: `model::Meta` has a `remember`
  field.
- MQTT number commands no longer link core's float parser, which saves about 17 KB of code and
  constants.
- A connected station waits for esp-radio's disconnect event instead of checking its link every
  2 s. It still checks every 30 s in case it missed an event, and reads the signal strength then.

### Fixed

- Matter over Thread: an update over Matter was rolled back after 30 minutes, because a paired
  device could fail its network health check (the attach could wake another task). The check now
  passes, and the update is kept.

### Security

- Matter's crypto: `crypto-bigint` 0.7.9 and `ctutils` 0.4.6 fix operations that were meant to be
  constant-time and were not (RUSTSEC-2026-0387, RUSTSEC-2026-0385). Matter over Wi-Fi grows by
  about 2.3 KB.

## [0.2.0] - 2026-10-25

Matter over Thread, updates over Matter, battery devices over Thread and the ESPHome API, and more
bad updates rolled back. Breaking: `init!` takes the flash ([Upgrading](docs/src/upgrading.md)).

### Changed

- **Breaking:** `init!` takes the flash, and `start` needs it. `Board` has no `flash` field.
- **Breaking:** `matter_task` takes the product name and the `Firmware` instead of a `Product`.
- More bad updates are rolled back: one never healthy after `IOTKIT_UPDATE_TIMEOUT` (30 min), one
  that cannot start Wi-Fi or Thread, and one that crashes right after `init!`. The health check
  follows the network (`iotkit::health`): Thread, a Matter controller, or Home Assistant over the
  ESPHome API. While an update is on trial, the device refuses other installs, and a battery
  device stays awake ([Updates](docs/src/updates.md#what-the-device-does)).
- `apps/thread-light` is now the output of `cargo iotkit new`. `apps/ble-beacon` assembles the
  parts by hand.
- The setup button is interrupt-driven.
- Over the ESPHome API, the firmware shows as "1.2.0 (ESPHome iotkit 0.2.0)".

### Added

- Matter over Thread: `iotkit::start` with `matter-thread` and its own `Board`.
  `cargo iotkit new` makes Thread projects for the ESP32-C6 or an 8 MB ESP32-H2.
- Updates over Matter (`matter-ota`), without Wi-Fi: signed `.ota` files (`cargo iotkit ota
  build`) from a Matter OTA provider. `matter-ota-unsigned` takes bare images, checked by SHA-256
  ([Updates](docs/src/updates.md#over-matter-thread-wi-fi)).
- Battery devices ([Battery devices](docs/src/battery.md)):
  - deep sleep over the ESPHome API;
  - Matter over Thread as a sleepy end device (`matter-icd`), with the receiver on between polls
    for now;
  - `power = "battery"` for both in `cargo iotkit new`;
  - a pairing mode, at power-on until linked or after a 3 s hold, and wake-up on a button;
  - over the ESPHome API, updates installed at a set time (`IOTKIT_UPDATE_TIME`, 02:00 by
    default).
- Status LEDs: `iotkit::status`, driven by `components::status_led` or `status_rgb` (WS2812)
  ([Components](docs/src/components.md#status-led)). `cargo iotkit new` asks for one, and for the
  button's pin.

### Fixed

- A second setup window panicked.
- Home Assistant's ESPHome integration failed to add a device at the first try, and showed
  "Update available" with no version before the first check.
- A crash before the boot overwrote the previous crash record.

## [0.1.1] - 2026-10-21

Update if you use `ha-update` without `ota-push`: updates from Home Assistant now install.

### Fixed

- With `ha-update` and without `ota-push`, updates from Home Assistant waited forever. They now
  install.
- The console's `ota URL` says when the firmware has no update feature.

## [0.1.0] - 2026-10-21

The first release. Install it from git: it is not on crates.io yet. Tested on an
ESP32-C6-DevKitC-1.

### Added

- One crate for devices, `iotkit`, plus `iotkit-core` (no chip, tested on the host),
  `iotkit-image` (signed images, shared with the CLI) and `iotkit-build`. They replace
  `iotkit-app` and ten layer crates:
  - `iotkit_app::` is `iotkit::`;
  - the layers are modules: `iotkit::net`, `iotkit::ota`, `iotkit::mqtt` (was
    `iotkit_link_mqtt`), `iotkit::matter` (was `iotkit_link_matter`), and so on;
  - Matter over Thread is `matter-thread` with `default-features = false`.
- Opt-out features (`improv-ble`, `softap`, `improv-serial`, `console`, `ota-push`), and
  `IOTKIT_HOSTNAME` and `IOTKIT_TOPIC_PREFIX`.
- `components`: ready-made parts (relay, binary input, button, dimmer, fan, RGB light, polled
  sensors with filters). `cargo iotkit new` uses the relay, input, button and polled ones.
- `init!`, `Config::new` with builders, typed `DeviceClass` and `Unit`, sensor shortcuts.
- `cargo iotkit new`: `TODO(<id>)` hints, and an optional `AGENTS.md` for AI coding agents.
- `tls`: TLS 1.3 for MQTT (port 8883) and `https://` downloads, with the server checked against a
  built-in CA.
- `esphome-api`: Home Assistant's ESPHome integration over the native API (TCP 6053, Noise
  encryption, mDNS discovery), with every entity kind and no broker. `model::Binding::allocate`
  (`extra-bindings`) for transports outside iotkit.
- The original ESP32: MQTT, with the console and Improv over UART0. No Bluetooth setup or deep
  sleep yet.
- Fuzz targets for the parsers that face the network (`fuzz/`), and `tools/hil/run.py`, a hardware
  test (flash, healthy, OTA kept, OTA rolled back) with a nightly workflow for a self-hosted
  runner.
- The guide (`docs/`, on GitHub Pages), with a "Coming from ESPHome" page, and a short README.
- Install from the browser: `cargo iotkit build` writes an ESP Web Tools page and a merged image
  (`dist/<package>/web/`), and `cargo iotkit web` serves it.

### Changed

- Licensed under MIT or Apache-2.0 (was Apache-2.0).

[Unreleased]: https://github.com/tanguy-c/iotkit/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/tanguy-c/iotkit/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/tanguy-c/iotkit/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/tanguy-c/iotkit/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/tanguy-c/iotkit/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/tanguy-c/iotkit/compare/v0.1.0...v0.2.0
[0.1.1]: https://github.com/tanguy-c/iotkit/compare/v0.1.0...1218359
[0.1.0]: https://github.com/tanguy-c/iotkit/releases/tag/v0.1.0
