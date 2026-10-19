# iotkit

A `no_std` firmware kit for the ESP32-C6 (esp-hal + Embassy): Wi-Fi setup, signed OTA updates
with automatic rollback, crash records and watchdogs, and Home Assistant over MQTT and Matter.
A device's `main` describes its entities and hardware, and calls `iotkit_app::start` for the rest.

## Chips

| Chip | Wi-Fi, MQTT, OTA, setup | Matter | Tested on hardware |
| --- | --- | --- | --- |
| ESP32-C6 | yes | Wi-Fi (`matter-wifi`), Thread (`apps/thread-light`) | yes (ESP32-C6-DevKitC-1, 8 MB) |
| ESP32-C3 | yes | no: Matter needs about 24 KB more RAM than the C3 has | build only |
| ESP32-C5 | yes | no: esp-hal 1.2 has no true RNG on the C5 yet | build only |
| ESP32-S3 | yes | no: does not fit in internal RAM (PSRAM support later) | build only |
| ESP32-H2 | no Wi-Fi | Thread (`apps/thread-light`) | build only |

The ESP32-C6 is the reference: support for the other chips is added without changing it.
`cargo iotkit ... --chip esp32c3` builds a firmware for another chip (`--flash-size 4mb` for
4 MB boards); a generated project records its chip in `[package.metadata.iotkit]`.

## 1. Prerequisites

```bash
rustup target add riscv32imac-unknown-none-elf riscv32imc-unknown-none-elf   # C6/C5/H2, C3
cargo install espflash cargo-generate
# ESP32-S3 only (Xtensa): Espressif's toolchain, then `. ~/export-esp.sh` in each shell.
# Use 1.98.1.0: 1.99 fails to link esp-hal 1.2.2.
cargo install espup && espup install --toolchain-version 1.98.1.0
# The iotkit CLI (signed builds, flashing, OTA), from this checkout:
cargo install --path xtask
```

Linux: add yourself to the serial group (`sudo usermod -aG dialout $USER`), then log out and in.

## 2. Create a project

```bash
cargo generate --path /path/to/iotkit/template --name my-device
# asks for the chip, and the absolute path of this checkout (the crates are used from there)
cd my-device
cargo iotkit keygen        # keys/ota.key (secret, git-ignored) and keys/ota.pub (in the firmware)
```

The generated `src/main.rs` shows the RGB LED as a light, toggled by a short press of BOOT:

```rust
static LED: Light = Light::new("led", "LED");
static ENTITIES: [&dyn Entity; 1] = [&LED];
static CONFIG: Config = Config {
    firmware: iotkit_app::firmware!(&OTA_KEY),   // package name + version from Cargo.toml
    manufacturer: "Me", model: "ESP32-C6-DevKitC-1", chip: "ESP32-C6", product_name: "My device",
    entities: &ENTITIES, watches: &[], checks: &[], console: None,
};
// in main, after esp_hal::init, the heap and esp_rtos::start:
iotkit_app::start(spawner, Board { flash: p.FLASH, wifi: p.WIFI, .. }, &CONFIG).await;
```

Entities are `Sensor`, `BinarySensor`, `Switch`, `Light` and `Button`: set their state from any
task (`TEMP.set(21.5)`), wait for commands (`LED.wait_command().await`), and report the result
with `set`. `iotkit_app::App::load`/`save` keep settings in flash (`SaveLater` debounces them).
`apps/devkit` is a fuller example: watched tasks, its own health check, an extra console command.

## 3. First flash (USB)

```bash
cargo iotkit flash --release      # or `cargo run --release`: flash, then the serial monitor
```

If the board is not detected, hold **BOOT**, tap **RESET**, release **BOOT**. Every USB flash
boots `ota_0` (otadata is erased). `partitions.csv` is for 8 MB of flash (two 3.9 MB app slots,
needed for Matter); `partitions-4mb.csv` fits 4 MB boards without Matter.

## 4. Join Wi-Fi

On first boot with no stored network the device opens a setup window. Give it your network any
of three ways:

- **Setup access point:** join the open network `iotkit-XXXX`; the phone shows the setup page
  (or open `http://192.168.4.1/`). Keep the page open: it reloads and says when the device is
  connected (the access point restarts once while it joins).
- **Bluetooth ([Improv](https://www.improv-wifi.com)):** the Home Assistant app offers the device
  under *Settings > Devices > Add*; improv-wifi.com works from a desktop browser.
- **USB serial (Improv):** ESP Web Tools, or improv-wifi.com with Web Serial.

The device tries the network before keeping it. The network must be 2.4 GHz, and the password
empty or 8 to 63 characters. To change it later: hold **BOOT** 3 s (the devkit's LED turns
blue) and release, or type `provision` on the USB console; the window stays open 10 minutes.
Hold **BOOT** 10 s (red) to factory-reset: every setting and Matter pairing is erased.

> During a setup window anyone in range can give the device a network (open access point,
> Improv without a button press). Open it only when you need it.

For development, `.cargo/secrets.toml` (from `secrets.toml.example`, git-ignored) can set
`WIFI_SSID` / `WIFI_PASS` (and `MQTT_HOST`): the first boot stores them and skips the window.

```bash
ping iotkit-XXXX.local     # mDNS
```

## 5. Wireless updates (OTA)

```bash
cargo iotkit build --release                         # dist/<package>/: image + signed manifest
cargo iotkit verify                                  # optional: check it like a device would
cargo iotkit ota push --host iotkit-XXXX.local --release   # build, serve, have the device install it
```

`ota push` serves `dist/` from this machine, so the device must reach it (same network, port
8000). If the host name does not resolve (no mDNS in the host's resolver), use the device's
address (`net` on the console). Bump `version` in `Cargo.toml` for each update.

What happens on the device:

1. The manifest's Ed25519 signature is checked, and the version must be newer than the running
   one (`--min-version` sets the oldest allowed to update).
2. The image streams into the spare slot; its header must name the same product and version. A
   dropped connection resumes with an HTTP `Range` request.
3. Its size and SHA-256 must match the manifest; then the slot is selected and the device restarts.
4. The new firmware gets 3 trial boots to pass the health check (boot recorded, Wi-Fi up, broker
   reached, plus the app's own checks); then it is kept.
5. If it never gets there, the firmware switches back to the previous slot by itself (no
   bootloader support needed). An update that crashes before the kit counts its boots is caught
   too, after 6 boots, by a counter in RTC memory.

The OTA endpoint (port 80) answers anyone on the network, but installs only images signed with
your key, and never an older version.

The repository's devkit has two test switches: `DEVKIT_PANIC_AT_BOOT=1 cargo xtask ota push ...`
pushes a build that never gets healthy, and `IOTKIT_PANIC_EARLY=1` one that crashes before the
trial is counted; `push` then reports the rollback.

## 6. Home Assistant

- **MQTT:** install the Mosquitto add-on and the MQTT integration, then point the device at the
  broker: `mqtt 192.168.1.10` on the USB console (or `MQTT_HOST` in `.cargo/secrets.toml`). The
  entities appear by themselves (MQTT discovery), and the device shows as unavailable when it
  goes offline (last will, or `offline` before a planned restart).
- **Matter over Wi-Fi** (`--features matter-wifi`, 8 MB flash): runs next to MQTT. Each device
  draws its own pairing code at first boot (boot log, or `matter` on the console). In Home
  Assistant: *Settings > Devices > Add > Matter*, with the code (no Bluetooth step: the device is
  already on the network). Lights, switches (as plugs), sensors (temperature, humidity, pressure,
  illuminance, air quality, battery), binary sensors (contact, motion, leak, smoke/CO), buttons
  (as switches with events), covers, valves, locks, fans, thermostats and selects appear, named
  after the entities;
  other entities stay MQTT-only. `matter pair` lets another controller pair for 15 minutes;
  `matter reset` forgets every pairing. Matter uses the test vendor's attestation: fine for
  development, not for selling devices.
- **Matter over Thread:** `apps/thread-light` (needs `clang` and `cmake`) boots and advertises
  over Bluetooth, but is untested end to end (it needs a Thread border router).

Topics: `iotkit/<host>/status` (`online`/`offline`), `iotkit/<host>/<entity>/state`,
`iotkit/<host>/<entity>/set`, and discovery under `homeassistant/`.

Without Home Assistant, `test-server/compose.yaml` runs a broker, a Home Assistant
(`http://localhost:8123`, about 1.7 GB) and a Matter controller on the PC:

```bash
docker compose -f test-server/compose.yaml up -d mosquitto
mosquitto_sub -h localhost -v -t 'iotkit/#' -t 'homeassistant/#'
mosquitto_pub -h localhost -t iotkit/iotkit-XXXX/led/set -m '{"state":"ON","color":{"r":255,"g":0,"b":0}}'
```

## 7. USB console

Type `help`: `info` (version, boot counters, last crash, last update), `net`, `provision`,
`ota URL`, `mqtt [HOST[:PORT] [USER PASS]]`, `matter [pair|reset]`, and test commands (`panic`,
`hang`, `reboot`, `factory-reset`). Apps add their own with `Config::console`.

## 8. Logs

`ESP_LOG` in `.cargo/config.toml` sets the level at build time (`info` by default). Release builds
compile out debug and trace logs (they would cost RAM): for those, build without `--release`.

## 9. VSCode

Accept the recommended extensions (rust-analyzer, probe-rs, Even Better TOML, Error Lens). Tasks
(`Ctrl+Shift+B`): **Flash USB**, **Monitor**, **Build signed**, **OTA push**. Debug (`F5`) uses
probe-rs over the built-in USB-JTAG.

## This repository

| Path | What |
| --- | --- |
| `crates/iotkit-app` | `start`: the kit in one call, the console |
| `crates/iotkit-*` | the layers: config (settings in flash), diag, runtime (health), net, ota, image (manifest), provision, model, link-mqtt, link-matter, build (`build.rs` helper) |
| `apps/devkit` | reference firmware; `cargo xtask ...` here is the same CLI as `cargo iotkit` |
| `template/` | the `cargo generate` template; `check-sync.sh` keeps its copied files in step |
| `xtask/` | the `cargo iotkit` CLI |
| `test-server/` | broker, Home Assistant and Matter controller for tests |

Host tests: `cargo test-host --features iotkit-image/sign`. Build apps one at a time
(`cargo build -p devkit`): `cargo build --workspace` fails, because devkit (Wi-Fi) and
thread-light (802.15.4) would share one esp-radio build.

## Troubleshooting

| Problem | Fix |
| --- | --- |
| `Permission denied` on `/dev/ttyACM0` | Add yourself to `dialout`, log in again |
| `espflash` hangs on "Connecting" | Another program holds the serial port (a monitor); close it |
| Build fails: "No OTA key" | `cargo iotkit keygen` |
| `ota push`: name not resolved | Use the device's IP address (`net` on the console) |
| `ota push`: connection timed out | Device not on Wi-Fi yet, or on another network |
| OTA refused | Key mismatch, version not newer, or image too big for the slot |
| Device keeps rolling back | The health check never passes: read the log over USB |
| Setup page cannot join | Wrong password, a 5 GHz-only network; some phone hotspots refuse for a minute after a wrong password |
