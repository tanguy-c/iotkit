# Console, logs, editor

## USB console

A tool for development, off by default: add the `console` feature (or answer yes to the console
question of `cargo iotkit new`).

```toml
iotkit = { git = "https://github.com/tanguy-c/iotkit", features = ["esp32c6", "console"] }
```

`cargo iotkit flash` opens it after flashing. Type `help`:

| Command | |
| --- | --- |
| `info` | version, uptime, boot and crash counters, last crash, last update |
| `net` | host name and address |
| `mem` | heap and stack use, now and at their fullest since boot |
| `provision` | open a 10-minute setup window |
| `ota URL` | install the update whose manifest is at URL, once (nothing is stored) |
| `mqtt [HOST[:PORT] [USER PASS]]` | the broker, or set it\* (then restarts) (`mqtt`) |
| `api-key [KEY]` | the ESPHome API's encryption key, or set it\* (then restarts) (`esphome-api`) |
| `matter [pair \| reset]` | pairing codes, a pairing window, forget pairings (`matter-wifi`) |
| `panic`, `hang`, `reboot`, `factory-reset` | failures on purpose, restart, erase everything |

\* Stored on the device: it wins over the built-in value until a factory reset
([where settings come from](getting-started.md#where-settings-come-from)).

Without the console and `improv-serial`, the app keeps the USB port (`Board::usb`).

Apps add their own commands with `Config::console`. `mem` reads
`iotkit::diag::esp::memory()` (with `console`), which an app can publish too: the stack's deepest use is found
from a pattern painted at boot, the heap's fullest sampled every second.

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
