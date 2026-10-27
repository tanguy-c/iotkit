# Home Assistant

## MQTT

Install the Mosquitto add-on and the MQTT integration, then point the device at the broker:
`mqtt 192.168.1.10` on the USB console (or `MQTT_HOST` at build time; `mqtt HOST USER PASS` with
a login). The entities appear by themselves (MQTT discovery), and the device shows as unavailable
when it goes offline (last will, or `offline` before a planned restart).

With the [`tls` feature](lean.md#tls), a broker on port 8883 is reached over TLS 1.3
(`mqtt 192.168.1.10:8883`).

Topics: `iotkit/<host>/status` (`online` / `offline`), `iotkit/<host>/<entity>/state`,
`iotkit/<host>/<entity>/set`, `iotkit/<host>/<entity>/<setting>/set` (one setting of a fan,
thermostat or cover), and discovery under `homeassistant/`. The `iotkit` prefix is
`IOTKIT_TOPIC_PREFIX` ([names](lean.md#names)).

## ESPHome API

With the `esphome-api` feature, Home Assistant's ESPHome integration finds the device by itself
(*Settings > Devices*, discovered) and controls it directly, without a broker. It asks for the
encryption key: `api-key` on the console shows it (made at first start; `ESPHOME_API_KEY` at
build time sets it instead, `api-key KEY` changes it). Every entity kind is served.
`cargo iotkit new` sets this up when you answer "ESPHome API" (`esphome = "fixed"` or
`"per-device"` in an answers file).

- Use the API or MQTT, not both: Home Assistant would show the device twice. Without a broker
  set, MQTT stays off; without the `mqtt` feature it is not built in at all (about 30 KB of flash
  and 9 KB of RAM less), as `cargo iotkit new` does for the API.
- One client at a time (Home Assistant's), on TCP port 6053; logs, services and Bluetooth proxy
  are not served.
- With `matter-wifi`, Matter's mDNS does not advertise the API: add the device by its address.

Another transport can be added outside iotkit the same way: `model::Binding::allocate`
(`iotkit-core`'s `extra-bindings` feature) gives it its own change flags.

## Matter

Home Assistant's Matter server is one Matter controller among others: pairing, the entities on
Matter, Wi-Fi or Thread: [Building a Matter device](matter.md).

## A test setup on the PC

`test-server/compose.yaml` runs a broker, a Home Assistant (`http://localhost:8123`) and a Matter
controller. Its container does not see mDNS: add an ESPHome API device by its address
(*Add integration > ESPHome*).

```bash
docker compose -f test-server/compose.yaml up -d mosquitto
mosquitto_sub -h localhost -v -t 'iotkit/#' -t 'homeassistant/#'
mosquitto_pub -h localhost -t iotkit/iotkit-XXXX/led/set -m '{"state":"ON","color":{"r":255,"g":0,"b":0}}'
```

The Matter controller: [A test controller on the PC](matter.md#a-test-controller-on-the-pc).
