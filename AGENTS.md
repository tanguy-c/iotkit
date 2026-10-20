# iotkit: notes for coding agents

no_std Rust firmware kit for ESP32 (esp-hal 1.2.2, Embassy). Users start at [README.md](README.md).

## Layout

- `crates/iotkit-*`, from the bottom up:
  - `config`, `runtime`, `image`, `model`: no iotkit dependencies (`build` is the apps' build.rs helper);
  - `diag`, `net` (on `config`, `runtime`), `ota`, `provision`;
  - `link-mqtt` (Home Assistant discovery, `ha.rs`), `link-matter`: both map `model` entities;
  - `app`: `iotkit_app::start` wires everything for an app.
- `apps/`: `devkit` (reference firmware, the default member), `showcase` (one entity of each kind),
  `sleepy` (deep sleep), `thread-light` (Matter over Thread, uses the crates directly).
- `template/`: the cargo-generate project; `xtask/`: the `cargo iotkit` CLI (`cargo xtask` here),
  its `new` wizard in `xtask/src/new/`; `test-server/`: Home Assistant and Mosquitto in compose.

## Commands

- Fresh clone: `cargo xtask keygen` first (firmware embeds `keys/ota.pub`).
- One app at a time: `cargo build -p devkit`, `cargo build -p thread-light` (needs clang, cmake).
  Never `--workspace`: devkit's Wi-Fi and thread-light's 802.15.4 would unify into one esp-radio
  build.
- Host tests: `cargo test-host --features iotkit-image/sign`;
  CLI: `cargo test --manifest-path xtask/Cargo.toml --target host-tuple`.
- [.github/workflows/ci.yml](.github/workflows/ci.yml) is the reference for clippy, per-chip builds
  and template checks: run the matching lines before pushing.
- `tools/size-check.sh [base]` before committing anything that touches the C6 build.
- `tools/link-check.sh`: code an app does not use must not be linked (no registries, no lists of
  every component, no `#[used]`). A new opt-in feature adds a rule there.

## Rules

- The ESP32-C6 is the primary chip. Never change its integration, dependencies, optimization
  settings or RAM/flash budget for another chip or feature: new work is additive, behind features
  or `cfg`. Drop a chip rather than alter the C6 path.
- Refuse unsupported combinations at build time with `compile_error!`, as in
  [crates/iotkit-app/src/lib.rs](crates/iotkit-app/src/lib.rs).
- The C3 has no atomics: `portable-atomic` is an opt-in feature its `esp32c3` features enable; the
  C6 keeps core atomics.
- No heap growth: `iotkit_app::init!`'s heap (64 + 96 KB) stays as is; use
  static or stack buffers (`heapless`, `static_cell`).
- `template/check-sync.sh` fails when the template's copies drift from the root files.
- `xtask/answers/*.toml` are CI inputs: a new part kind or board option belongs in them.

## Hardware gotchas

- Never toggle DTR/RTS on the C6's USB serial: it resets the chip (reset loops).
- Never run espflash while a monitor holds `/dev/ttyACM0`: it hangs on "Connecting".
- `partitions.csv` is 8 MB: flash with `--flash-size 8mb`. 4 MB boards use `partitions-4mb.csv`,
  without Matter.
- `ed25519-compact` and `iotkit-image` build at opt-level `z` ([Cargo.toml](Cargo.toml)); at `s`
  the Ed25519 verification grows to about 1.2 MB.
- esp-radio's `sta_disconnected_pm` starves BLE: keep it off (Improv setup over BLE,
  `crates/iotkit-net/src/esp.rs`).
- The USB serial port disappears during deep sleep: wait for it to come back before espflash.
- ESP32-S3 (Xtensa): esp toolchain 1.98.1.0 (`espup install --toolchain-version 1.98.1.0`); 1.99
  fails to link esp-hal 1.2.2.

## Style

- Concise comments: say why, not what. DRY: link to code or CI rather than copying it.
- Match the surrounding code (naming, module layout, error handling); `cargo fmt` uses
  [rustfmt.toml](rustfmt.toml).
- Skills in `.claude/skills/`: `size-check`, `add-entity-kind`, `add-chip`.
