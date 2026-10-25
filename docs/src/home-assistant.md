# Home Assistant and Matter

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
  set, MQTT stays off.
- One client at a time (Home Assistant's), on TCP port 6053; logs, services and Bluetooth proxy
  are not served.
- With `matter-wifi`, Matter's mDNS does not advertise the API: add the device by its address.

Another transport can be added outside iotkit the same way: `model::Binding::allocate`
(`iotkit-core`'s `extra-bindings` feature) gives it its own change flags.

## Matter over Wi-Fi

Feature `matter-wifi` (ESP32-C6, 8 MB flash), next to MQTT. The device's pairing code is in the
boot log, on the console (`matter`), or fixed for a product by `cargo iotkit new` (printed as a
QR label). In Home Assistant: *Settings > Devices > Add > Matter*, with the code; there is no
Bluetooth step, the device is already on the network.

| Entities | Over Matter |
| --- | --- |
| `Light` | Extended Color Light |
| `Switch` | On/Off Plug |
| temperature, humidity, pressure, illuminance, air quality sensors, in a unit Matter takes | the matching sensor |
| battery sensor | the device's power source |
| binary sensors: motion/occupancy/presence, leak, smoke, CO/gas; others as contact | the matching sensor or alarm |
| `Event` with presses | Generic Switch |
| `Cover`, `Valve`, `Lock`, `Fan`, `Climate`, `Select` | Window Covering, Water Valve, Door Lock, Fan, Thermostat, Mode Select |

Other entities are not on Matter (MQTT and the ESPHome API only). `matter pair` lets another controller pair for 15 minutes;
`matter reset` forgets every pairing. Matter uses the test vendor's attestation: fine for
development, not for selling devices.

## Matter over Thread

`cargo iotkit new` with *Thread* as the network (an ESP32-C6, or an ESP32-H2 board with 8 MB of
flash) makes one: `iotkit` with `default-features = false`, `matter-thread` and `matter-ota`, and
`iotkit::start` with the Thread `Board`
(the 802.15.4 radio, Bluetooth, the RNG and ADC1 for its entropy, so no analog parts yet).
Building needs `clang` and `cmake`. It pairs over Bluetooth, then joins the controller's Thread
network; holding BOOT 3 s opens a pairing window for another controller. Updates come over Matter
([Updates](updates.md#over-matter-thread-wi-fi)). `apps/thread-light` is this output for
`xtask/answers/thread.toml`; it boots and advertises for Bluetooth commissioning, and is untested
end to end (it needs a Thread border router).

## A test setup on the PC

`test-server/compose.yaml` runs a broker, a Home Assistant (`http://localhost:8123`) and a Matter
controller. Its container does not see mDNS: add an ESPHome API device by its address
(*Add integration > ESPHome*).

```bash
docker compose -f test-server/compose.yaml up -d mosquitto
mosquitto_sub -h localhost -v -t 'iotkit/#' -t 'homeassistant/#'
mosquitto_pub -h localhost -t iotkit/iotkit-XXXX/led/set -m '{"state":"ON","color":{"r":255,"g":0,"b":0}}'
```

The Matter controller needs the PC's network interface towards the device:
`MATTER_INTERFACE=<interface> docker compose -f test-server/compose.yaml up -d matter-server`.
