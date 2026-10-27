# Roadmap

What iotkit does not do yet. [Status](introduction.md#status) says what works today, the
[changelog](https://github.com/tanguy-c/iotkit/blob/main/CHANGELOG.md) what changed.

## Before 1.0

- **A stable API and settings layout**: until then a release may break both, with the steps in
  [Upgrading](upgrading.md).
- **Publishing on crates.io**, once rs-matter-embassy is released (iotkit depends on it from git).
- **Hardware tests** beyond the C6 and S3: the C3, C5, H2 and original ESP32; most entity kinds
  over Matter; the current drawn by Matter on battery (`matter-icd`).

## Next

- **Chips**: Bluetooth setup (`improv-ble` does not link yet) and deep sleep on the original
  ESP32; Matter on the S3 with PSRAM, and on the C5 once esp-hal has its true RNG.
- **TLS**: certificates checked for expiry, with a clock on every link (SNTP; today the time only
  comes from Home Assistant over the ESPHome API, or from a Matter controller for the update time).
- **ESPHome API**: setting the key from Home Assistant, logs over the API.
- **Matter**:
  - a product's own vendor and product IDs and attestation credentials built in, so its maker
    can certify it ([Before selling a device](matter.md#before-selling-a-device));
  - months on a battery over Thread: the receiver off between polls, long idle time ICDs
    (Check-In, rs-matter's ICD Management cluster), a battery level;
  - Matter over Wi-Fi on battery (`matter-icd`), with the esp-radio release that keeps power save
    working on the C6.
- **The C6's LP core**: iotkit's RTC records moved out of the LP program's way, so an app can
  count pulses or watch a sensor while the chip sleeps ([why not yet](battery.md#the-c6s-low-power-core-not-with-iotkit-yet)).
- **Components**: ready-made parts for common sensors (SHT4x, BME280, SCD4x, BH1750, DS18B20);
  today `cargo iotkit new` names a driver crate to wrap. Each is a good first issue
  ([wrapping a driver](components.md)).
