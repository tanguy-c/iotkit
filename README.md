# iotkit

Rust firmware for ESP32 smart devices: Wi-Fi setup from a phone, signed OTA updates with
automatic rollback, Home Assistant over its ESPHome API, MQTT or Matter, deep sleep. A `no_std`
kit on esp-hal and Embassy for when ESPHome's YAML gets in the way.

**[Guide](https://tanguy-c.github.io/iotkit/)** ·
[Coming from ESPHome](https://tanguy-c.github.io/iotkit/coming-from-esphome.html) ·
[Changelog](CHANGELOG.md)

```rust
static PUMP: Switch = Switch::new("pump", "Pump");
static TEMPERATURE: Sensor = Sensor::temperature("temperature", "Temperature");
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY)).entities(&[&PUMP, &TEMPERATURE]);

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit::init!(p);
    iotkit::start(spawner, Board { wifi: p.WIFI, .. }, &CONFIG).await;
    spawner.spawn(pump_task(Output::new(p.GPIO4, Level::Low, OutputConfig::default())).unwrap());
    // ...
}

#[embassy_executor::task]
async fn pump_task(pin: Output<'static>) {
    components::relay(&PUMP, pin, Active::High).await   // Home Assistant switches it
}
```

## Quick start

With an ESP32-C6 DevKit plugged in:

```bash
rustup target add riscv32imac-unknown-none-elf
cargo install espflash
cargo install --locked --git https://github.com/tanguy-c/iotkit iotkit-cli
cargo iotkit new my-light --defaults && cd my-light
cargo iotkit flash --release
```

Join the `iotkit-XXXX` network from a phone to give it your Wi-Fi, then type `mqtt <your broker>`
on the console: the board's LED shows up in Home Assistant. `cargo iotkit new` without
`--defaults` asks about your own board and parts.

## Status

Early (0.x): the API and the settings' flash layout may still change. Tested on an
ESP32-C6-DevKitC-1, a XIAO ESP32-C6 and an ESP32-S3-DevKitC-1; the C3, C5 and ESP32 are
build-tested, the H2's Thread build does not link yet ([chips](https://tanguy-c.github.io/iotkit/introduction.html#chips)).
Matter uses test attestation credentials: for development, not for selling devices. What comes
next: the [roadmap](docs/src/roadmap.md).

## Contributing

Issues and pull requests are welcome: [CONTRIBUTING.md](CONTRIBUTING.md).

## License

At your option, [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT).

Unless you state otherwise, any contribution you submit for inclusion in iotkit is licensed as
above, without any additional terms or conditions.
