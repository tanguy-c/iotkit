# Only what the device uses

## Features

`iotkit`'s services are features, all on by default. A device that does not need one builds
without it: `default-features = false`, then list the ones to keep.

| Feature | What | Without it |
| --- | --- | --- |
| `mqtt` | Home Assistant over MQTT | about 30 KB less flash and 9 KB less RAM; the ESPHome API or Matter as the link |
| `improv-ble` | Wi-Fi setup over Bluetooth (Improv) | no Bluetooth stack: about 300 KB less flash; no `Board::bt` |
| `softap` | the setup access point and its page | no portal, DHCP or DNS server |
| `improv-serial` | Wi-Fi setup over USB (ESP Web Tools) | |
| `ota-push` | updates pushed with `cargo iotkit ota push` | updates only from Home Assistant (`ha-update`), or none |

Opt-in features: `console` ([the USB console](console.md)), `ha-update`, `deep-sleep`, `matter-wifi`, `matter-thread`, `smartled`, `tls`,
`esphome-api` ([ESPHome API](home-assistant.md#esphome-api)), `panic-handler`.

```toml
iotkit = { git = "https://github.com/tanguy-c/iotkit", default-features = false, features = ["esp32c6", "wifi", "mqtt", "improv-serial", "ota-push"] }
```

With no setup path left (`improv-ble`, `softap`, `improv-serial`), the build needs `WIFI_SSID` at
build time. `apps/lean` keeps `mqtt`, `improv-serial` and `ota-push`: about 350 KB less flash than
`apps/devkit`. Unused components and entity kinds are not linked either; the repository's
`tools/link-check.sh` checks it.

A download, an update's install and the setup access point share one work area, reserved once at
the size of the largest (about 21 KB; 41 KB with `tls`). An update asked for while a setup window
is open is refused (`ota push` answers 503: try again once the window closes); a window opened
during an update opens once it is over.

## TLS

The `tls` feature encrypts MQTT on port 8883 and `https://` downloads (manifests, images) with
TLS 1.3 (AES-128-GCM). The server's certificate (ECDSA P-256) must chain to one CA built into the
firmware, and carry the name or address the device connects to:

```toml
[env]
IOTKIT_TLS_CA = { value = "certs/ca.der", relative = true }   # the CA, DER
```

It costs about 125 KB of flash and 21 KB of RAM per connection (MQTT, downloads). The device
has no clock yet, so certificate expiry is not checked. The repository's
`test-server/certs/make.sh` makes a test CA and certificate for its broker (8883) and update
server (8443).

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
for settings, `iotkit::provision` for Improv, `iotkit::diag` for crash records, `iotkit::health`
for the health check and its supervisor (which keeps an update on trial, when there is one).
`apps/ble-beacon` builds its firmware from them (`default-features = false`, `panic-handler`).
With `panic-handler`, call `iotkit::diag::esp::boot()` first on every boot, as `start` does: it
takes the crash record, and until it is taken, later crashes are not recorded (of crashes in a
row, the first is kept).
