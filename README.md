# iotkit: Getting Started

A `no_std` ESP32 device template (esp-hal + Embassy) with Wi-Fi provisioning,
OTA with rollback, and Home Assistant / Matter connectivity.

> `iotkit-*` crates and `cargo xtask` commands are the design target for this
> project. Adapt names to what you actually build.

## 1. Prerequisites

```bash
# Rust (stable is enough for RISC-V chips: C3, C6, H2)
rustup default stable
rustup target add riscv32imac-unknown-none-elf   # C6/H2
rustup target add riscv32imc-unknown-none-elf    # C3

# Xtensa chips only (ESP32, S2, S3)
cargo install espup && espup install && source ~/export-esp.sh

# Tooling
cargo install espflash esp-generate probe-rs-tools cargo-generate
```

Linux: add yourself to the serial group, then log out and in.

```bash
sudo usermod -aG dialout $USER
```

## 2. Create a project

```bash
cargo generate --git https://github.com/you/iotkit-template --name my-device
# or start bare:
esp-generate --chip esp32c6 my-device
cd my-device
```

## 3. First flash (USB)

Plug the board in via its USB port (C3/C6/S3 expose a built-in USB-Serial-JTAG).
If the board is not detected, hold **BOOT**, tap **RESET**, release **BOOT**.

```bash
# Flash bootloader + partition table + app, then open the serial monitor
cargo run --release
```

`.cargo/config.toml` provides the runner:

```toml
[target.riscv32imac-unknown-none-elf]
runner = "espflash flash --monitor --partition-table partitions.csv"
```

The partition table must contain two OTA slots. This is what makes wireless
updates possible later:

```csv
# Name,   Type, SubType, Offset,   Size
nvs,      data, nvs,     0x9000,   0x4000
otadata,  data, ota,     0xd000,   0x2000
phy_init, data, phy,     0xf000,   0x1000
config,   data, undefined, 0x10000, 0x10000
ota_0,    app,  ota_0,   0x20000,  0x1E0000
ota_1,    app,  ota_1,   0x200000, 0x1E0000
```

Adjust sizes to your flash (this example assumes 4 MB). To start from a clean
state: `espflash erase-flash`.

## 4. Join Wi-Fi (provisioning)

On first boot with no stored network the device opens a setup window. Give it
your network any of three ways:

- **Setup access point:** join the open network `iotkit-XXXX`; the phone shows
  the setup page (captive portal; otherwise open `http://192.168.4.1/`).
  Enter your network and password, keep the page open: it reloads by itself
  and says when the device is connected (the access point restarts once while
  it joins, so the phone may rejoin it).
- **Bluetooth ([Improv](https://www.improv-wifi.com)):** the Home Assistant
  companion app offers the device under *Settings > Devices > Add*;
  improv-wifi.com works from a desktop browser with Bluetooth.
- **USB serial (Improv):** ESP Web Tools, or improv-wifi.com in a browser with
  Web Serial.

The device tries the network before keeping it and stores it in the `config`
partition; the window then closes. To change it later: hold **BOOT** 3 s (the
LED turns blue) and release, or type `provision` on the USB console; the window
stays open 10 minutes. Hold **BOOT** 10 s (red) to factory-reset. The network
must be 2.4 GHz.

> For development, `.cargo/secrets.toml` (from `secrets.toml.example`, git-ignored)
> can set `WIFI_SSID` / `WIFI_PASS`: the first boot stores that network and skips
> the setup window.

Check it is on the network:

```bash
ping iotkit-XXXX.local     # mDNS
```

## 5. Wireless flash (OTA)

Every build produces a signed firmware image and a manifest.

```bash
# Once: create the signing key pair (keys/ota.key stays secret and git-ignored)
cargo xtask keygen

# Build + sign: dist/<package>/manifest.json and the app image
cargo xtask build --release
cargo xtask verify      # optional: check it like a device would

# Dev: build, sign, serve dist/ from this machine and have the device install it
cargo xtask ota push --host iotkit-XXXX.local --release

# Prod: publish to your server; devices poll the manifest and pull
cargo xtask ota publish --server https://updates.example.com
```

What happens on the device:

1. The manifest's Ed25519 signature is checked, and the version must be newer
   than the running one (`--min-version` sets the oldest allowed to update).
2. The image streams into the inactive slot (`ota_1` if running `ota_0`). Its
   header must name the same product and version; a dropped connection resumes
   with an HTTP `Range` request.
3. Its size and SHA-256 must match the manifest; then the slot is selected and
   the device restarts.
4. The new firmware gets 3 trial boots to pass its health check (Wi-Fi up, and
   later broker or Matter reachable); then it is kept.
5. If it never gets there (it crashes, the watchdog fires, it cannot connect),
   the firmware itself switches back to the previous slot. This needs no
   bootloader support.

Test rollback on purpose: `DEVKIT_PANIC_AT_BOOT=1 cargo xtask ota push ...`
pushes a devkit build that panics at boot; `push` then reports the rollback.

The device needs about 15 s for an update of 900 KB over a phone hotspot.

## 6. Connect to Home Assistant

- **MQTT:** install the Mosquitto broker add-on and the MQTT integration, then
  point the device at the broker: `mqtt 192.168.1.10` on the USB console (or
  `MQTT_HOST` in `.cargo/secrets.toml`). The device and its entities appear by
  themselves (MQTT discovery): for the devkit, the RGB LED as a light, a
  "Color cycle" switch, and chip temperature, uptime, Wi-Fi signal and Restart
  under diagnostics. It shows as unavailable when it goes offline (last will).
- **Matter over Wi-Fi** (devkit with `--features matter-wifi`, 8 MB flash):
  runs next to MQTT on the same Wi-Fi. Each device draws its own pairing code
  at first boot (boot log, or `matter` on the console). In Home Assistant:
  Settings > Devices > Add > Matter, with the code (the device is already on
  the network, so no Bluetooth step). The light, switches, temperature and
  contact sensors appear; buttons and other sensors stay MQTT-only.
  `matter pair` lets another controller pair for 15 minutes; `matter reset`
  forgets every pairing.
- **Matter over Thread** (`apps/thread-light`, needs `clang` and `cmake`):
  no Wi-Fi; paired over Bluetooth with the code in the boot log, then on the
  controller's Thread network. Needs a Thread border router.

Matter uses the test vendor's attestation: fine for development, not for
selling devices.

Without Home Assistant, `test-server/compose.yaml` runs a broker (and, with
`up -d matter-server`, a Matter controller) on the PC:

```bash
docker compose -f test-server/compose.yaml up -d mosquitto
mosquitto_sub -h localhost -v -t 'iotkit/#' -t 'homeassistant/#'
mosquitto_pub -h localhost -t iotkit/iotkit-XXXX/led/set -m '{"state":"ON","color":{"r":255,"g":0,"b":0}}'
```

`docker compose -f test-server/compose.yaml up -d homeassistant` adds a Home
Assistant on `http://localhost:8123` (about 1.7 GB to download); in its MQTT
integration the broker is `mosquitto`, port 1883. Its state lives in
`test-server/ha-config/` (git-ignored; delete it to start over).

Topics: `iotkit/<host>/status` (`online`/`offline`), `iotkit/<host>/<entity>/state`,
`iotkit/<host>/<entity>/set`, and discovery under `homeassistant/`.

## 7. VSCode

Open the folder and accept the recommended extensions
(`.vscode/extensions.json`):

- rust-analyzer
- probe-rs Debugger
- Even Better TOML
- Error Lens
- Wokwi Simulator (optional)

Tasks (`Ctrl+Shift+B`): **Flash USB**, **Monitor**, **OTA push**.
Debug (`F5`): launches probe-rs over the built-in USB-JTAG.

## Troubleshooting

| Problem | Fix |
|---|---|
| `Permission denied` on `/dev/ttyACM0` | Add yourself to `dialout`, relog |
| Flash hangs on connect | BOOT + RESET sequence, try another cable |
| rust-analyzer errors on `std` | Set `rust-analyzer.cargo.target` (see `.vscode/settings.json`) |
| OTA rejected | Check signature key matches, version is newer, image fits the slot |
| Device keeps rolling back | Health check never reached; read logs over USB |
| `ota push`: connection timed out | Device not on Wi-Fi yet, or another network (`net` on the console shows its address) |
| `espflash` hangs on "Connecting" | Another program holds the serial port (a monitor); close it |
| Setup page says it cannot join | Wrong password, or a 5 GHz-only network; some phones' hotspots also refuse for a minute after a wrong password |
