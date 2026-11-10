# Working on iotkit

| Path | What |
| --- | --- |
| `crates/iotkit` | what a device depends on: `start` and the modules (`net`, `ota`, `provision`, `mqtt`, `esphome`, `matter`, `components`, `diag`, `config`) |
| `crates/iotkit-core` | the chip-independent part, re-exported by `iotkit`: `model`, `config` (the store), `image` (re-exported from `iotkit-image`), `runtime` (health) |
| `crates/iotkit-image` | signed update images and manifests, shared by the device and the CLI |
| `crates/iotkit-build` | the apps' `build.rs` helper |
| `apps/` | the [examples](examples.md); `cargo xtask ...` here is `cargo iotkit` |
| `template/` | the `cargo generate` template; `check-sync.sh` keeps its copies in step |
| `xtask/` | the `cargo iotkit` CLI |
| `boards/` | what `cargo iotkit new` knows about each board, one file per board ([its README](https://github.com/tanguy-c/iotkit/tree/main/boards)): a new board is a new file |
| `docs/` | this guide (`mdbook serve docs`) |
| `test-server/` | broker, Home Assistant, update server and Matter controller for tests |
| `fuzz/`, `tools/` | fuzz targets for the network parsers; size, link and hardware checks |

```bash
cargo xtask keygen           # once: the firmware embeds keys/ota.pub (git-ignored)
cargo test-host              # iotkit-image, iotkit-core, and iotkit without a chip
cargo build -p devkit        # one app at a time
```

Build one app at a time: `cargo build --workspace` fails, because devkit's Wi-Fi and
thread-light's 802.15.4 would share one esp-radio build.

[CONTRIBUTING.md](https://github.com/tanguy-c/iotkit/blob/main/CONTRIBUTING.md) has the project
rules, and covers issues and pull requests. The CI workflows (`ci.yml`, `full.yml`) are the
reference for every check.

## Hardware notes

These notes save you a reset loop or a firmware that no longer fits. Users meet some of them too:
[Troubleshooting](troubleshooting.md) has those.

- **The C6's USB serial port:** leave DTR and RTS alone. Toggling them resets the chip, so a tool
  that toggles them each time it opens the port keeps the board in a reset loop.
- **The C6's LP core:** keep it unused (`LpCore`, `load_lp_code!`). It overwrites iotkit's RTC
  records: the early rollback, the crash record and the deep-sleep state
  ([why](battery.md#the-c6s-low-power-core-not-with-iotkit-yet)).
- **Ed25519 verification:** `ed25519-compact` and `iotkit-image` build at opt-level `z` (the root
  `Cargo.toml`). At `s`, the verification grows to about 1.2 MB.
- **Bluetooth setup:** keep esp-radio's `sta_disconnected_pm` off. It starves Bluetooth, and
  Improv setup over Bluetooth stops working (`crates/iotkit/src/net/esp.rs`).
- **The original ESP32:** it has no USB serial port, so the console and Improv use UART0
  (`Board::uart`). `improv-ble` does not link there yet (esp-radio's Bluetooth ROM symbols), so it
  builds without default features.
- **Xtensa chips (S3, ESP32):** use Espressif's toolchain 1.98.1.0. Version 1.99 fails to link
  esp-hal 1.2.2 ([getting started](getting-started.md)).
