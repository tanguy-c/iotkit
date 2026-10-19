The getting-started README is in `README.md`: USB first flash, Wi-Fi provisioning, OTA, Home Assistant, VSCode. The architecture and parts list follow.

## Architecture

```
┌───────────────────────── Application (your device) ─────────────────────────┐
│  sensors / actuators  →  declares entities (sensor, switch, light…)         │
├─────────────────────────────────────────────────────────────────────────────┤
│  iotkit-model   transport-agnostic entities + state/commands (traits)       │
├───────────────┬──────────────────────┬──────────────────────────────────────┤
│ link-mqtt     │ link-matter          │ link-http (local API, OTA push)      │
│ (HA discovery)│ (rs-matter-embassy)  │ (picoserve / edge-http)              │
├───────────────┴──────────────────────┴──────────────────────────────────────┤
│  iotkit-net        Wi-Fi supervisor, DHCP, DNS, SNTP, mDNS                  │
│  iotkit-provision  BLE or SoftAP + captive portal, factory reset            │
│  iotkit-ota        slots, streaming download, verify, rollback              │
│  iotkit-config     versioned typed config in flash                          │
│  iotkit-diag       logging, panic/crash record, reset reason, watchdog      │
├─────────────────────────────────────────────────────────────────────────────┤
│  iotkit-runtime    boot sequence, task wiring, health check → mark valid    │
├─────────────────────────────────────────────────────────────────────────────┤
│  esp-hal · esp-radio · esp-rtos/Embassy · esp-storage · esp-bootloader-…    │
└─────────────────────────────────────────────────────────────────────────────┘
```

The key design choice is `iotkit-model`. The device describes *what it is* once, and MQTT and Matter are just two bindings to that description. Without it you end up with two parallel implementations.

## Parts to create

| Crate | Role | Notes |
|---|---|---|
| `iotkit-model` | Entity traits, state/command channels | Pure `no_std`, host-testable |
| `iotkit-config` | Typed, versioned settings in flash | `sequential-storage` + `postcard`, with schema migration |
| `iotkit-net` | Wi-Fi connect/reconnect with backoff, time sync | Wraps `esp-radio` + `embassy-net` |
| `iotkit-provision` | First-boot credentials flow | BLE (`trouble`) and SoftAP (`edge-captive`, `picoserve`) |
| `iotkit-ota` | Download, verify, switch slot, rollback | Biggest piece, see below |
| `iotkit-link-mqtt` | MQTT client + HA discovery payloads | Retained availability topic, last will |
| `iotkit-link-matter` | `rs-matter-embassy` glue to the model | Commissioning, persistence, OTA-requestor later |
| `iotkit-diag` | Logging setup, crash record, metrics, watchdog | Store last panic in flash and report it after reboot |
| `iotkit-runtime` | Boot flow and supervisor | Starts tasks, runs the health check, marks image valid |
| `iotkit-template` | `cargo-generate` template | Partitions, `.cargo/config.toml`, `.vscode/`, CI |
| `xtask` / `iotkit-cli` | Build, sign, manifest, OTA push/publish | Host-side |
| Test server | Minimal update server + MQTT broker compose file | For dev and CI |

`iotkit-ota` has the following parts:
- A partition and slot state machine, using `esp-bootloader-esp-idf` for otadata.
- A streaming writer with flash-erase alignment and resume.
- SHA-256 plus an Ed25519 signature check.
- A version and anti-downgrade policy.
- A pull client (manifest over HTTPS) and a push endpoint for dev.
- The health-check contract that decides when a new image is marked valid.

## Parts to use or consider

**Core:** `esp-hal`, `esp-radio`, `esp-rtos` (successor to `esp-hal-embassy`), `esp-alloc`, `esp-storage`, `esp-bootloader-esp-idf`, `esp-backtrace`, `embassy-executor`, `embassy-time`, `embassy-sync`, `embassy-futures`, `embassy-net`, `static_cell`, `heapless`.

**Network and protocols:** `edge-net` (`edge-http`, `edge-mdns`, `edge-dhcp`, `edge-captive`), `picoserve`, `reqwless`, `rust-mqtt` or `minimq`, `rs-matter`, `rs-matter-embassy`, `trouble`.

**Security:** `esp-mbedtls` (hardware-accelerated) or `embedded-tls`, `sha2`, `ed25519-compact` or `salty`, plus ESP secure boot and flash encryption (a one-way eFuse decision, so plan it early).

**Storage and data:** `sequential-storage`, `ekv`, `embedded-storage`, `serde` with `postcard` or `serde-json-core`.

**Observability:** `defmt` (small binaries) or `log` with `esp-println`.

**Tooling and testing:** `espflash`, `esp-generate`, `cargo-generate`, `probe-rs`, `embedded-test` (on-device tests), `cargo-nextest` (host tests), `cargo-bloat`, `cargo-deny`, `cargo-audit`, Wokwi CLI for CI simulation, `chip-tool` for Matter testing, a Mosquitto container for MQTT.

**Worth considering later:** `esp-ieee802154` for Thread on C6/H2, `esp-now` for sensor mesh, deep-sleep helpers for battery devices, delta OTA (`bidiff`, `qbsdiff`), a fleet dashboard.

## VSCode

**Extensions:** rust-analyzer, probe-rs Debugger, Even Better TOML, Error Lens, Wokwi Simulator (optional), Dependi or crates for versions.

**`.vscode/settings.json`:**
```json
{
  "rust-analyzer.cargo.target": "riscv32imac-unknown-none-elf",
  "rust-analyzer.check.allTargets": false,
  "rust-analyzer.cargo.allTargets": false,
  "rust-analyzer.cargo.features": ["wifi"]
}
```

For Xtensa chips, also set `"rust-analyzer.server.extraEnv": { "RUSTUP_TOOLCHAIN": "esp" }`.

**`.vscode/tasks.json`:** one task each for `cargo run --release` (flash over USB), `espflash monitor`, and `cargo xtask ota push`.

**`.vscode/launch.json`:** a `probe-rs-debug` config. C3, C6 and S3 have built-in USB-JTAG, so debugging works over the same USB cable with no extra probe.

**Tip:** pick a RISC-V chip such as the C6 as the primary target. It uses the stable toolchain (no `espup`), and it is the best fit for Matter over Wi-Fi or Thread.

## Suggested build order

1. `iotkit-config` and `iotkit-diag`, the foundations.
2. `iotkit-net`, then `iotkit-ota`, so wireless updates work as early as possible.
3. `iotkit-provision`.
4. `iotkit-model` with `link-mqtt`, for a first working Home Assistant device.
5. `link-matter` last, since it is the heaviest on RAM and the most volatile API.

Check the current `esp-bootloader-esp-idf` OTA API and the `rs-matter-embassy` release state before committing to either. Both change quickly, and I can look up their current versions.
