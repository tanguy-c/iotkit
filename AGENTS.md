# iotkit: notes for coding agents

no_std Rust firmware kit for ESP32 (esp-hal 1.2.2, Embassy). Users start at [README.md](README.md),
then the guide in `docs/` (mdBook; `docs/src/llms.txt` indexes it).

## Layout

- `crates/iotkit-core`: no chip, host-tested: `model`, `config` (the store), `image` (the
  re-exported `iotkit-image`), `runtime` (health check), `remember` (saved entity states).
- `crates/iotkit-image`: signed update images and manifests, shared by the device and the CLI
  (which signs with it).
- `crates/iotkit`: what apps depend on. Modules `config::esp`, `diag`, `health` (the checks and
  the supervisor that keeps or rolls back an update), `net`, `ota` (the engine in `esp.rs`, HTTP in
  `esp/http.rs`), `provision`, `mqtt` (Home Assistant discovery, `ha.rs`), `esphome` (the ESPHome
  native API, `api.rs`), `matter` (`ota.rs`: the OTA Requestor, `matter-ota`; `icd.rs`: on
  battery, `matter-icd`, its sleepy Thread driver in `icd/thread.rs`), `remember` (restores
  and saves `.remember()` states, `remember`), `scratch` (the work area that downloads, update
  installs and the setup access point share, one at a time), `components` (ready-made
  parts over embedded-hal); `app.rs` (`Config`, `init!`; `start` and `Board` in
  `app/wifi.rs` or `app/thread.rs`), re-exported at the root. Each module's chip code is in its
  `esp.rs`, behind `feature = "_esp"` or, on the Wi-Fi side, `cfg(esp_wifi)` (a chip and `wifi`),
  `cfg(esp_ota)` for `ota::esp` (a chip and `wifi` or `matter-ota`), both set by `build.rs`; the
  rest builds on the host.
- `crates/iotkit-build`: the apps' build.rs helper.
- `docs/`: the user guide. User-facing explanations live there, once; the README stays a short
  landing page.
- `apps/`: `devkit` (reference firmware, the default member), `showcase` (most entity kinds),
  `sleepy` (deep sleep), `lean` (opt-out features), `thread-light` (Matter over Thread with
  Matter OTA: `cargo iotkit new --answers xtask/answers/thread.toml`'s output, keep them in step),
  `ble-beacon` (BTHome over Bluetooth, no network or OTA: the crates assembled by hand).
- `template/`: the cargo-generate project; `xtask/`: the `cargo iotkit` CLI (`cargo xtask` here),
  its `new` wizard in `xtask/src/new/`; `test-server/`: Home Assistant, Mosquitto, an update server
  and a Matter controller in compose.

## Commands

- Fresh clone: `cargo xtask keygen` first (firmware embeds `keys/ota.pub`).
- One app at a time: `cargo build -p devkit`, `cargo build -p thread-light` (needs clang, cmake).
  Never `--workspace`: devkit's Wi-Fi and thread-light's 802.15.4 would unify into one esp-radio
  build.
- Host tests: `cargo test-host` (`iotkit-image`, `iotkit-core`, and `iotkit` without a chip);
  CLI: `cargo test --manifest-path xtask/Cargo.toml --target host-tuple`.
- [.github/workflows/ci.yml](.github/workflows/ci.yml) (quick) and
  [full.yml](.github/workflows/full.yml) (other chips, templates, fuzz; nightly or the `full-ci`
  label) are the reference for clippy, per-chip builds and template checks: run the matching lines before pushing.
- `tools/size-check.sh [base]` before committing anything that touches the C6 build.
- `tools/link-check.sh`: code an app does not use must not be linked (no registries, no lists of
  every component, no `#[used]`). A new opt-in feature adds a rule there.

## Rules

- The ESP32-C6 is the primary chip. Never change its integration, dependencies, optimization
  settings or RAM/flash budget for another chip or feature: new work is additive, behind features
  or `cfg`. Drop a chip rather than alter the C6 path.
- Refuse unsupported combinations at build time with `compile_error!`, as in
  [crates/iotkit/src/lib.rs](crates/iotkit/src/lib.rs).
- The C3 has no atomics: `portable-atomic` is an opt-in feature its `esp32c3` features enable; the
  C6 keeps core atomics.
- No heap growth: `iotkit::init!`'s heap (64 + 96 KB; ESP32: 96 KB) stays as is; use
  static or stack buffers (`heapless`, `static_cell`).
- `template/check-sync.sh` fails when the template's copies drift from the root files.
- `xtask/answers/*.toml` are CI inputs: a new part kind or board option belongs in them.

## Hardware gotchas

- Never toggle DTR/RTS on the C6's USB serial: it resets the chip (reset loops).
- The C6's LP core (`LpCore`, `load_lp_code!`) overwrites iotkit's RTC records (early rollback,
  crash record, deep-sleep state): not supported yet (`docs/src/battery.md`).
- Never run espflash while a monitor holds `/dev/ttyACM0`: it hangs on "Connecting".
- `partitions.csv` is 8 MB: flash with `--flash-size 8mb`. 4 MB boards use `partitions-4mb.csv`,
  without Matter.
- `ed25519-compact` and `iotkit-image` build at opt-level `z` ([Cargo.toml](Cargo.toml)); at `s`
  the Ed25519 verification grows to about 1.2 MB.
- esp-radio's `sta_disconnected_pm` starves BLE: keep it off (Improv setup over BLE,
  `crates/iotkit/src/net/esp.rs`).
- The USB serial port disappears during deep sleep: wait for it to come back before espflash.
- ESP32-S3 and ESP32 (Xtensa): esp toolchain 1.98.1.0 (`espup install --toolchain-version 1.98.1.0`);
  1.99 fails to link esp-hal 1.2.2.
- The original ESP32 has no USB serial: the console and Improv use UART0 (`Board::uart`), and
  `improv-ble` does not link yet (esp-radio's BT ROM symbols), so it builds without default features.

## Style

- Concise comments: say why, not what. DRY: link to code or CI rather than copying it.
- Match the surrounding code (naming, module layout, error handling); `cargo fmt` uses
  [rustfmt.toml](rustfmt.toml).
- Skills in `.claude/skills/`: `size-check`, `add-entity-kind`, `add-chip`, `add-component`.
