# Home Assistant and Matter

## MQTT

Install the Mosquitto add-on and the MQTT integration, then point the device at the broker:
`mqtt 192.168.1.10` on the USB console (or `MQTT_HOST` at build time; `mqtt HOST USER PASS` with
a login). The entities appear by themselves (MQTT discovery), and the device shows as unavailable
when it goes offline (last will, or `offline` before a planned restart).

With the [`tls` feature](lean.md#tls), a broker on port 8883 is reached over TLS 1.3
(`mqtt 192.168.1.10:8883`).

Topics: `iotkit/<host>/status` (`online` / `offline`), `iotkit/<host>/<entity>/state`,
`iotkit/<host>/<entity>/set`, and discovery under `homeassistant/`. The `iotkit` prefix is
`IOTKIT_TOPIC_PREFIX` ([names](lean.md#names)).

## Matter over Wi-Fi

Feature `matter-wifi` (ESP32-C6, 8 MB flash), next to MQTT. The device's pairing code is in the
boot log, on the console (`matter`), or fixed for a product by `cargo iotkit new` (printed as a
QR label). In Home Assistant: *Settings > Devices > Add > Matter*, with the code; there is no
Bluetooth step, the device is already on the network.

| Entities | Over Matter |
| --- | --- |
| `Light` | Extended Color Light |
| `Switch` | On/Off Plug |
| temperature, humidity, pressure, illuminance, battery, air quality sensors | the matching sensor |
| contact, motion, leak, smoke/CO binary sensors | the matching sensor or alarm |
| `Event` with presses | Generic Switch |
| `Cover`, `Valve`, `Lock`, `Fan`, `Climate`, `Select` | Window Covering, Water Valve, Door Lock, Fan, Thermostat, Mode Select |

Other entities stay MQTT-only. `matter pair` lets another controller pair for 15 minutes;
`matter reset` forgets every pairing. Matter uses the test vendor's attestation: fine for
development, not for selling devices.

## Matter over Thread

`iotkit` with `default-features = false` and `matter-thread` (ESP32-C6 or H2; building needs
`clang` and `cmake`). `apps/thread-light` boots and advertises for Bluetooth commissioning; it is
untested end to end (it needs a Thread border router).

## A test setup on the PC

`test-server/compose.yaml` runs a broker, a Home Assistant (`http://localhost:8123`) and a Matter
controller:

```bash
docker compose -f test-server/compose.yaml up -d mosquitto
mosquitto_sub -h localhost -v -t 'iotkit/#' -t 'homeassistant/#'
mosquitto_pub -h localhost -t iotkit/iotkit-XXXX/led/set -m '{"state":"ON","color":{"r":255,"g":0,"b":0}}'
```

The Matter controller needs the PC's network interface towards the device:
`MATTER_INTERFACE=<interface> docker compose -f test-server/compose.yaml up -d matter-server`.
