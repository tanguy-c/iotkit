# iotkit: notes for coding agents

iotkit is a `no_std` Rust firmware kit for ESP32 chips, built on esp-hal 1.2.2 and Embassy. Users
start with [README.md](README.md), then read the guide in `docs/` (an mdBook indexed by
`docs/src/llms.txt`).

Before you change code, read the [project rules](CONTRIBUTING.md#project-rules). They apply to
every change, and reviews check them. Before you flash a board, or change radio, crypto or RTC
code, read the [hardware notes](docs/src/contributing.md#hardware-notes).

## Layout

- `crates/iotkit-core`: the code that needs no chip, tested on the host.
  - `model`: entities.
  - `config`: the settings store.
  - `image`: re-exports `iotkit-image`.
  - `runtime`: the health check.
  - `remember`: saved entity states.
- `crates/iotkit-image`: signed update images and manifests. The device verifies them and the CLI
  signs them.
- `crates/iotkit`: the crate apps depend on. Its modules are re-exported at the root.
  - `app.rs`: `Config` and `init!`. `start` and `Board` are in `app/wifi.rs` or `app/thread.rs`.
  - `config::esp`, `diag`, `net`, `provision`.
  - `health`: the checks, and the supervisor that keeps or rolls back an update.
  - `ota`: the engine in `esp.rs`, HTTP in `esp/http.rs`.
  - `mqtt`: Home Assistant discovery in `ha.rs`.
  - `esphome`: the ESPHome native API in `api.rs`.
  - `matter`: the OTA Requestor in `ota.rs` (`matter-ota`). Battery devices in `icd.rs`
    (`matter-icd`), with the sleepy Thread driver in `icd/thread.rs`.
  - `remember`: restores and saves `.remember()` states.
  - `scratch`: one work area that downloads, update installs and the setup access point take in
    turn.
  - `components`: ready-made parts over embedded-hal.
- Chip code: each module keeps it in its `esp.rs`, behind `feature = "_esp"`. The Wi-Fi side uses
  `cfg(esp_wifi)` (a chip and `wifi`), and `ota::esp` uses `cfg(esp_ota)` (a chip and `wifi` or
  `matter-ota`). `build.rs` sets both. Everything else builds on the host.
- `crates/iotkit-build`: the helper that apps call from their `build.rs`.
- `apps/`:
  - `devkit`: the reference firmware, and the workspace's default member.
  - `showcase`: most entity kinds.
  - `sleepy`: deep sleep.
  - `lean`: opt-out features.
  - `thread-light`: Matter over Thread with Matter OTA. It is the output of
    `cargo iotkit new --answers xtask/answers/thread.toml`: when you change one, change the other.
  - `ble-beacon`: BTHome over Bluetooth, with no network or OTA, its crates assembled by hand.
- `xtask/`: the `cargo iotkit` CLI (`cargo xtask` in this repository). The `new` wizard and `add`
  are in `xtask/src/new/`, with the chips' pin rules in `boards.rs`.
- `boards/`: the boards the wizard knows, one TOML file each ([its README](boards/README.md)).
- `components/`: the parts the wizard knows, one TOML file each, with their code as Liquid
  templates ([its README](components/README.md)).
- `template/`: the cargo-generate project.
- `docs/`: the user guide. Explain each user-facing topic there, on one page, and keep the README
  a short landing page.
- `test-server/`: Home Assistant, Mosquitto, an update server and a Matter controller, in Docker
  Compose.
- `tools/`: the size and link checks, the hardware test and the release script.

## Commands

- On a fresh clone, run `cargo xtask keygen` first: firmware embeds `keys/ota.pub`.
- Build one app at a time: `cargo build -p devkit`, or `cargo build -p thread-light` (it needs
  clang and cmake). A `--workspace` build would unify devkit's Wi-Fi and thread-light's 802.15.4
  into one esp-radio build, which fails.
- Run the host tests with `cargo test-host` (`iotkit-image`, `iotkit-core`, and `iotkit` without a
  chip). For the CLI, run `cargo test --manifest-path xtask/Cargo.toml --target host-tuple`.
- Before you push, run the lines of [ci.yml](.github/workflows/ci.yml) (quick) and
  [full.yml](.github/workflows/full.yml) (other chips, templates, fuzz) that match your change.
  They are the reference for clippy, per-chip builds and template checks.
- When a change can reach the C6 build, run `tools/size-check.sh [base]` before you commit.
- Run `tools/link-check.sh` when you add or move optional code.

## Writing for people

The guide, the README, the CHANGELOG and CONTRIBUTING are read by people, often new to iotkit.
Write them this way:

- Address the reader as "you", in the active voice.
- Write short sentences, one idea each. Use a list for more than two items.
- Put the condition before the instruction: "To flash a 4 MB board, use `partitions-4mb.csv`."
- Start with what changes for the reader, then explain the mechanism.
- Use code font for what the reader types or sees in code: features, commands, items, files.

When a change is visible to users, in the same pull request:

- update its page in the guide;
- add an entry under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md), in the section that fits
  (Added, Changed, Removed, Fixed, Security);
- for a breaking change, start the entry with **Breaking:** and add the steps to
  [docs/src/upgrading.md](docs/src/upgrading.md).

## Code style

- Match the surrounding code: naming, module layout, error handling. `cargo fmt` uses
  [rustfmt.toml](rustfmt.toml).
- Keep comments short and say why, not what.
- State each fact once and link to it, in code, docs and CI alike.

## Skills

`.claude/skills/` has step lists for common changes: `size-check`, `add-entity-kind`, `add-chip`
and `add-component`.
