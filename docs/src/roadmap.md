# Roadmap

What 0.1 does not have yet, and what comes after. [Status](introduction.md#status) says what works
today.

## Not in 0.1

- **Hardware tests** beyond the ESP32-C6: the C3, C5, S3 and original ESP32; Matter over Thread
  with a border router; the current drawn by Matter on battery (`matter-icd`).
- **Publishing on crates.io**, once rs-matter-embassy is released (iotkit depends on it from git).

## Next

- **Chips**: Bluetooth setup and deep sleep on the original ESP32 (esp-radio); Matter on the S3
  with PSRAM.
- **TLS**: a clock (SNTP), so certificates are checked for expiry.
- **ESPHome API**: setting the key from Home Assistant, logs over the API.
- **Matter**: a product's own attestation credentials, for selling devices; long idle time ICDs
  (Check-In, rs-matter's ICD Management cluster), and a battery level over Thread; Matter over
  Wi-Fi on battery (`matter-icd`), with the esp-radio release that keeps power save working on
  the C6.
- **Components**: drivers for common sensors (SHT4x, BME280, SCD4x, BH1750, DS18B20), each a good
  first issue ([wrapping a driver](components.md)).
