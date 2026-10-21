# Console, logs, editor

## USB console

`cargo iotkit flash` opens it after flashing. Type `help`:

| Command | |
| --- | --- |
| `info` | version, uptime, boot and crash counters, last crash, last update |
| `net` | host name and address |
| `provision` | open a 10-minute setup window |
| `ota URL` | install the update whose manifest is at URL |
| `mqtt [HOST[:PORT] [USER PASS]]` | the broker, or set it (then restarts) |
| `update-url [URL]` | where updates from Home Assistant come from (`ha-update`) |
| `api-key [KEY]` | the ESPHome API's encryption key, or set it (then restarts) (`esphome-api`) |
| `matter [pair \| reset]` | pairing codes, a pairing window, forget pairings (`matter-wifi`) |
| `panic`, `hang`, `reboot`, `factory-reset` | failures on purpose, restart, erase everything |

Apps add their own commands with `Config::console`.

## Logs

`ESP_LOG` in `.cargo/config.toml` sets the level at build time (`info` by default). Release
builds compile out debug and trace logs (they would cost RAM): for those, build without
`--release`.

## VS Code

Accept the recommended extensions (rust-analyzer, probe-rs, Even Better TOML, Error Lens,
Dependi). Tasks (`Ctrl+Shift+B`): *Flash USB*, *Monitor*, *Build signed*, *OTA push*, *Build
debug*. Debugging (`F5`) uses probe-rs over the chip's built-in USB-JTAG (not on the original
ESP32, which has none).

## AI coding agents

`cargo iotkit new` can add an `AGENTS.md`: the device, the commands, the rules of a `no_std`
firmware, and where to look up iotkit's and esp-hal's API. The generated `TODO(<entity id>)`s
carry the bus setup and checked driver crates.
