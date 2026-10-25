# Examples

The repository's apps, each a complete firmware:

| App | What it shows |
| --- | --- |
| [devkit](https://github.com/tanguy-c/iotkit/tree/main/apps/devkit) | the reference firmware: RGB LED, chip temperature, Wi-Fi signal, watched tasks, its own health check and console command; every Wi-Fi chip |
| [showcase](https://github.com/tanguy-c/iotkit/tree/main/apps/showcase) | most entity kinds over MQTT and Matter, simulated; the board's LED through `components::rgb_light` |
| [sleepy](https://github.com/tanguy-c/iotkit/tree/main/apps/sleepy) | a deep-sleeping sensor, over MQTT (with a *Stay awake* switch) or the ESPHome API (`esphome-api`) |
| [lean](https://github.com/tanguy-c/iotkit/tree/main/apps/lean) | a sensor without Bluetooth, access point or console commands |
| [thread-light](https://github.com/tanguy-c/iotkit/tree/main/apps/thread-light) | Matter over Thread with `start`, updated over Matter: `cargo iotkit new`'s output for `xtask/answers/thread.toml`; ESP32-C6 and H2 |
| [ble-beacon](https://github.com/tanguy-c/iotkit/tree/main/apps/ble-beacon) | The chip temperature in BTHome advertisements: the store, boot records and health check assembled by hand, without `start`, a network or updates |

`cargo iotkit new` writes a project for your own parts: a good start for anything else.

Built something with iotkit? Open an issue or a pull request to add it here, with a photo and the
wiring.
