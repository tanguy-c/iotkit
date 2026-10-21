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
| `docs/` | this guide (`mdbook serve docs`) |
| `test-server/` | broker, Home Assistant, update server and Matter controller for tests |
| `fuzz/`, `tools/` | fuzz targets for the network parsers; size, link and hardware checks |

```bash
cargo xtask keygen           # once: the firmware embeds keys/ota.pub (git-ignored)
cargo test-host              # iotkit-image, iotkit-core, and iotkit without a chip
cargo build -p devkit        # one app at a time
```

`cargo build --workspace` fails: devkit's Wi-Fi and thread-light's 802.15.4 would share one
esp-radio build. [AGENTS.md](https://github.com/tanguy-c/iotkit/blob/main/AGENTS.md) has the
rules (the ESP32-C6 build must not change for another chip or feature), the size and link checks,
and the hardware gotchas; the CI workflows (`ci.yml`, `full.yml`) are the reference for every
check.
[CONTRIBUTING.md](https://github.com/tanguy-c/iotkit/blob/main/CONTRIBUTING.md) covers issues
and pull requests.
