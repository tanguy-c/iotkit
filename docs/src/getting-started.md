# Getting started

## Tools

```bash
rustup target add riscv32imac-unknown-none-elf riscv32imc-unknown-none-elf   # C6/C5/H2, C3
cargo install espflash
cargo install --locked --git https://github.com/tanguy-c/iotkit iotkit-cli   # `cargo iotkit`
```

- ESP32-S3 and ESP32 (Xtensa): Espressif's toolchain, `cargo install espup && espup install
  --toolchain-version 1.98.1.0` (1.99 fails to link esp-hal 1.2.2), then `. ~/export-esp.sh` in
  each shell.
- Linux: `sudo usermod -aG dialout $USER`, then log out and in, for the serial port.

## Create a project

```bash
cargo iotkit new garden-sensor
```

It asks, in plain words:

- **what it is** (its name in Home Assistant) and your name or brand;
- **the board**: type to search (a maker, a chip, a name). Espressif's C6, C3, C5 and S3 DevKits,
  the ESP32-DevKitC, Seeed's XIAO ESP32-C6, C3 and S3, or another board with one of these chips.
  Each board's LEDs and quirks come with it (the XIAO ESP32-C6's antenna switch is set in `main`);
  [`boards/`](https://github.com/tanguy-c/iotkit/tree/main/boards) has one file per board, and
  another board is one more file;
- **the power**: plugged in, or on battery (wakes up, reports, sleeps; with the battery level if
  you want);
- **connectivity**: your Wi-Fi now or later from a phone; how Home Assistant reaches it, over
  its MQTT broker or the ESPHome API (no broker; one encryption key built in, or a key per
  device); Matter (C6) with one fixed pairing code printed as a QR label (or a code per device);
  where you publish updates. Battery devices use MQTT, without Matter;
- **the parts**, each with its pin: temperature, humidity, pressure, light level, CO2, fine dust,
  soil moisture; motion, door, leak, smoke; relay, light, blind, valve, lock, fan, thermostat,
  settings; push buttons; any other reading or mode. Pins are checked for the chip (no flash or USB pin, an ADC pin for
  analog parts);
- **the LEDs**: the board's RGB LED as a light, and what shows the status (pairing, connecting,
  updates): that LED (over the light, by default when plugged in), a LED on a pin, or nothing;
- whether to add an `AGENTS.md` for AI coding agents.

You get one entity per part, a [ready-made part](components.md) where there is one and a
`TODO(<entity id>)` with hints elsewhere, a README with the wiring table, `.cargo/secrets.toml`
with what you gave, and the update signing keys. `--defaults` asks nothing (a plugged-in C6 with
its LED as a light that shows the status); `--answers FILE` reads the answers from TOML (scripts, CI).

The project depends on iotkit from GitHub; `Cargo.lock` pins the commit and `cargo update` moves
it on. `--iotkit-path DIR` uses a local checkout instead.

Without the questions: `cargo generate --git https://github.com/tanguy-c/iotkit template --name
my-device`, then `cargo iotkit keygen`. It makes the DevKit's RGB LED a light, toggled by a short
press of BOOT, that shows the device's status too (blue blinking while it waits to be set up).

## First flash

```bash
cargo iotkit flash --release      # or `cargo run --release`: flash, then the serial monitor
```

If the board is not detected, hold **BOOT**, tap **RESET**, release **BOOT**. A USB flash boots
the first app slot. `partitions.csv` is for 8 MB of flash (two 3.9 MB app slots, needed for
Matter); `partitions-4mb.csv` fits 4 MB boards without Matter. Or [install from the
browser](install-from-browser.md).

## Join Wi-Fi

With no network stored, the device opens a setup window. Give it your network any of three ways:

- **Setup access point:** join the open network `iotkit-XXXX`; the phone shows the setup page (or
  open `http://192.168.4.1/`). Keep it open: it says when the device is connected (the access
  point restarts once while it joins).
- **Bluetooth ([Improv](https://www.improv-wifi.com)):** the Home Assistant app offers the device
  under *Settings > Devices > Add*; improv-wifi.com works from a desktop browser.
- **USB (Improv):** ESP Web Tools, or improv-wifi.com with Web Serial.

The device tries the network before keeping it: 2.4 GHz, and a password that is empty or 8 to 63
characters. To change it later, hold **BOOT** 3 s, or type `provision` on the USB console;
the window stays open 10 minutes, or until a short press of **BOOT** closes it (once a network is
stored). Hold **BOOT** 10 s to erase every setting and Matter pairing.

> During a setup window anyone in range can give the device a network. Open it only when you
> need it.

For development, `.cargo/secrets.toml` (git-ignored) can set `WIFI_SSID`, `WIFI_PASS` and
`MQTT_HOST`: the first boot stores them and skips the window. `secrets.toml.example` lists the
other keys (broker login, update URL, Matter code, ESPHome API key).

Then [connect it to Home Assistant](home-assistant.md).
