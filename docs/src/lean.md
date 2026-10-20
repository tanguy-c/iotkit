# Only what the device uses

## Features

`iotkit`'s services are features, all on by default. A device that does not need one builds
without it: `default-features = false`, then list the ones to keep.

| Feature | What | Without it |
| --- | --- | --- |
| `improv-ble` | Wi-Fi setup over Bluetooth (Improv) | no Bluetooth stack: about 300 KB less flash; no `Board::bt` |
| `softap` | the setup access point and its page | no portal, DHCP or DNS server |
| `improv-serial` | Wi-Fi setup over USB (ESP Web Tools) | |
| `console` | the USB console | with `improv-serial` off too, the app keeps the USB port (no `Board::usb`) |
| `ota-push` | updates pushed with `cargo iotkit ota push` | updates only from Home Assistant (`ha-update`), or none |

Opt-in features: `ha-update`, `deep-sleep`, `matter-wifi`, `matter-thread`, `smartled`.

```toml
iotkit = { git = "https://github.com/tanguy-c/iotkit", default-features = false, features = ["esp32c6", "wifi", "improv-serial", "ota-push"] }
```

With no setup path left (`improv-ble`, `softap`, `improv-serial`), the build needs `WIFI_SSID` at
build time. `apps/lean` keeps `improv-serial` and `ota-push`: about 350 KB less flash than
`apps/devkit`. Unused components and entity kinds are not linked either; the repository's
`tools/link-check.sh` checks it.

## Names

Two names are set at build time, in the project's `.cargo/config.toml`:

```toml
[env]
IOTKIT_HOSTNAME     = "garden"   # garden-aefc.local, the setup access point, the device in Home Assistant
IOTKIT_TOPIC_PREFIX = "home"     # home/<host>/... MQTT topics
```

Both default to `iotkit`. The host name prefix takes 1 to 10 of `[a-z0-9-]`; the last two bytes
of the MAC address make each device's name unique.

## Without `start`

The modules work on their own: `iotkit::ota` for signed updates with rollback, `iotkit::config`
for settings, `iotkit::provision` for Improv, `iotkit::diag` for crash records. `apps/thread-light`
builds its firmware from them (`default-features = false`, `matter-thread`).
