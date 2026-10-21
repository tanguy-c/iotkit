# Contributing to iotkit

Thanks for helping. iotkit is early: bug reports, ideas, docs fixes and code are all welcome.

- **A bug or an idea**: open an issue; the templates ask for what helps. Security problems go
  through [SECURITY.md](SECURITY.md), not an issue.
- **A first change**: issues labelled `good first issue`, most of them a ready-made component
  for a common part. The [roadmap](docs/src/roadmap.md) lists the bigger pieces.
- **Questions**: an issue is fine.

## Setting up

[Working on iotkit](docs/src/contributing.md) has the layout and the first commands. An
ESP32-C6-DevKitC-1 is the reference board; other boards help most by testing on hardware.

## What reviews check

[AGENTS.md](AGENTS.md) is written for coding agents and holds the project's rules for everyone,
once: the ESP32-C6 build does not change for another chip or feature, nothing is linked unless
the app uses it, the size and link checks, the style, and the hardware gotchas. The quick CI
runs on every pull request; the other chips and generated projects run nightly (`full.yml`), or on
your pull request with the `full-ci` label.

The skills in [`.claude/skills/`](.claude/skills) are step lists for the common changes, as
useful to read as to hand to an agent:

| Skill | For |
| --- | --- |
| [add-component](.claude/skills/add-component/SKILL.md) | a ready-made part (sensor, actuator, input) and its wizard entry |
| [add-entity-kind](.claude/skills/add-entity-kind/SKILL.md) | a new entity kind through the model, MQTT, Matter and the wizard |
| [add-chip](.claude/skills/add-chip/SKILL.md) | another ESP32 chip |
| [size-check](.claude/skills/size-check/SKILL.md) | comparing the C6 firmware sizes before committing |

## Pull requests

- One topic per pull request, with CI green. A C6 size change the PR means gets the `size-ok`
  label.
- Say what you tested on hardware, and on which board; "host tests only" is a fine answer.
- User-facing changes: the guide in `docs/` and a line in [CHANGELOG.md](CHANGELOG.md).

## Labels

| Label | For |
| --- | --- |
| `good first issue` | small and well-described; most are components |
| `component` | a ready-made part (`iotkit::components`) |
| `chip` | a chip or board |
| `hardware` | needs testing on a device |
| `size-ok` | (pull requests) the C6 size change is meant |

## License

Contributions are dual-licensed like the project, MIT or Apache-2.0 (see the
[README](README.md#license)).
