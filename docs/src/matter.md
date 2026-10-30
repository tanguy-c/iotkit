# Building a Matter device

Matter is a standard, not one vendor's ecosystem: an iotkit device pairs with any Matter
controller (Home Assistant's Matter server, a smart-home hub, a phone app), and with several at
once. It uses Matter's test attestation credentials, fine for development: a controller may
warn before pairing it ([below](#before-selling-a-device)).

## Wi-Fi or Thread

| | Matter over Wi-Fi | Matter over Thread |
| --- | --- | --- |
| Feature | `matter-wifi`, next to MQTT or the ESPHome API | `matter-thread`, with `default-features = false` |
| Chips | ESP32-C6, 8 MB of flash | ESP32-C6 or ESP32-H2 |
| Needs | the Wi-Fi network | a Thread border router, e.g. Home Assistant's OpenThread Border Router add-on or a hub with one |
| Pairing | no Bluetooth step: the device is already on the network | over Bluetooth, then it joins the controller's Thread network |
| Updates | over the network, or over Matter with `matter-ota` | over Matter (`matter-ota`) |
| On battery | not yet | `matter-icd` ([Battery devices](battery.md#matter-on-battery-matter-icd)) |

Why the other chips have no Matter yet: [Chips](introduction.md#chips).

## Start a project

`cargo iotkit new` asks for the network on a board that has Thread; on Wi-Fi it asks whether the
device also works with Matter. Then the pairing code: one fixed code printed as a QR label
(`matter = "fixed"` in an answers file), or a code per device (`"per-device"`).

A Thread project has `iotkit` with `default-features = false`, `matter-thread` and `matter-ota`,
and `iotkit::start` with the Thread `Board` (the 802.15.4 radio, Bluetooth, the RNG and ADC1 for
its entropy, so no analog parts yet). Building needs `clang` and `cmake`.

`apps/thread-light` is `cargo iotkit new`'s output for `xtask/answers/thread.toml`;
`xtask/answers/h2.toml` is the same on an ESP32-H2-DevKitM-1, and `xtask/answers/c6.toml` is
Matter over Wi-Fi with one part of every kind.

## Entities on Matter

| Entities | Over Matter |
| --- | --- |
| `Light` | Extended Color Light |
| `Switch` | On/Off Plug |
| temperature, humidity, pressure, illuminance, air quality sensors, in a unit Matter takes | the matching sensor |
| battery sensor | the device's power source |
| binary sensors: motion/occupancy/presence, leak, smoke, CO/gas; others as contact | the matching sensor or alarm |
| `Event` with presses | Generic Switch |
| `Cover`, `Valve`, `Lock`, `Fan`, `Climate`, `Select` | Window Covering, Water Valve, Door Lock, Fan, Thermostat, Mode Select |

Other entities are not on Matter (MQTT and the ESPHome API only).

## Pairing

The pairing code is in the boot log, on the [console](console.md) (`matter`), or on the QR label
`cargo iotkit new` printed. Add the device in the controller with that code; in Home Assistant:
*Settings > Devices > Add > Matter*.

A second controller (multi-admin): `matter pair` on the console (the `console` feature) opens a
pairing window for 15 minutes; on Thread, holding BOOT 3 s opens one and a short press closes it. Most controllers can
also share a device they paired, which gives a code for the next one. `matter reset` forgets every
pairing; holding BOOT 10 s erases every setting and pairing.

Tested on hardware: Matter over Wi-Fi with Home Assistant's Matter server; Matter over Thread
through a Thread border router, paired with one controller, shared with Home Assistant's Matter
server, driven and updated over Matter by it.

## Updates

With `matter-ota` the device is a Matter OTA Requestor: a Matter OTA provider offers it signed
updates, so a Thread device updates without Wi-Fi ([Updates](updates.md#over-matter-thread-wi-fi)).
With `IOTKIT_UPDATE_TIME`, what its daily check finds installs at that time, in the time a
controller gives ([Without a click](updates.md#without-a-click)).

## A test controller on the PC

`test-server/compose.yaml` runs a Matter controller next to Home Assistant
([A test setup on the PC](home-assistant.md#a-test-setup-on-the-pc)). It needs the PC's network
interface towards the device:
`MATTER_INTERFACE=<interface> docker compose -f test-server/compose.yaml up -d matter-server`.

## Before selling a device

Certification is the product's, not the kit's: its maker gets a vendor ID from the Connectivity
Standards Alliance, attestation credentials for the product, and certifies it. iotkit builds every
device with the test vendor's IDs and credentials; building in a product's own is on the
[roadmap](roadmap.md).
