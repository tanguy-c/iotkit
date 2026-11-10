# Contributing to iotkit

Thanks for helping. iotkit is early, and bug reports, ideas, docs fixes and code are all welcome.

- **To report a bug or share an idea**, open an issue. The templates ask for what helps. For a
  security problem, follow [SECURITY.md](SECURITY.md) instead.
- **For a first change**, try a ready-made part for a common sensor, a board file in
  [`boards/`](boards/), or a docs fix.
- **If you own a board other than the ESP32-C6**, testing on it helps most: most chips are only
  build-tested ([chips](docs/src/introduction.md#chips)).
- **To ask a question**, open an issue.

## Setting up

[Working on iotkit](docs/src/contributing.md) has the layout, the first commands and the hardware
notes. The reference board is an ESP32-C6-DevKitC-1.

## Project rules

These rules protect the reference chip and the firmware size. Reviews check them, for people and
coding agents alike.

- **The ESP32-C6 comes first.** Add another chip or feature behind a Cargo feature or a `cfg`, and
  leave the C6's integration, dependencies, optimization settings and RAM and flash budget as
  they are. If a chip can only work by changing the C6 path, drop that chip.
- **Refuse unsupported combinations at build time.** Use `compile_error!` and give the reason, as
  in [crates/iotkit/src/lib.rs](crates/iotkit/src/lib.rs).
- **Link only what an app uses.** Reach optional code through features and the app's own calls,
  not through registries, lists of every component or `#[used]`. `tools/link-check.sh` checks
  this. A new opt-in feature adds a rule there.
- **Keep the heap as it is.** `iotkit::init!`'s heap is 64 + 96 KB with Wi-Fi, 64 KB without, and
  96 KB on the original ESP32 (`DEFAULT_HEAP`). For new data, use static or stack buffers
  (`heapless`, `static_cell`).
- **Keep core atomics on the C6.** The C3 has no atomic instructions, so its `esp32c3` features
  turn on the opt-in `portable-atomic` feature.
- **Keep the template in step.** `template/check-sync.sh` fails when the template's copies differ
  from the root files.
- **Cover new wizard options in CI.** CI generates projects from `xtask/answers/*.toml`: add a new
  part kind or board option there.

The quick CI runs on every pull request. The other chips and the generated projects run nightly
([full.yml](.github/workflows/full.yml)), or on your pull request when it has the `full-ci`
label.

## Step lists for common changes

The skills in [`.claude/skills/`](.claude/skills) list the steps for common changes. Read them
yourself or hand them to a coding agent. [AGENTS.md](AGENTS.md) is the entry point for agents.

| Skill | For |
| --- | --- |
| [add-component](.claude/skills/add-component/SKILL.md) | a ready-made part (sensor, actuator, input) and its wizard entry |
| [add-entity-kind](.claude/skills/add-entity-kind/SKILL.md) | a new entity kind through the model, MQTT, the ESPHome API, Matter and the wizard |
| [add-chip](.claude/skills/add-chip/SKILL.md) | another ESP32 chip |
| [size-check](.claude/skills/size-check/SKILL.md) | comparing the C6 firmware sizes before you commit |

## Pull requests

- Keep one topic per pull request, with CI green. If your PR changes the C6 size on purpose, add
  the `size-ok` label.
- Say what you tested on hardware, and on which board. "Host tests only" is a fine answer.
- If users can see the change, update its page in `docs/` and add an entry under `## Unreleased`
  in [CHANGELOG.md](CHANGELOG.md).

## Labels

| Label | For |
| --- | --- |
| `component` | a ready-made part (`iotkit::components`) |
| `chip` | a chip or board |
| `hardware` | needs testing on a device |
| `size-ok` | (pull requests) the C6 size change is meant |

## License

Contributions are dual-licensed like the project, under MIT or Apache-2.0 (see the
[README](README.md#license)).
