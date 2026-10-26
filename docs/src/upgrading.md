# Upgrading

What to change in a project when a release breaks the API, newest first. The
[changelog](https://github.com/tanguy-c/iotkit/blob/main/CHANGELOG.md) lists everything else.

## To 0.4 (from 0.3)

### MQTT is a feature

`mqtt`, on by default. A project with `default-features = false` that uses MQTT adds it:

```toml
iotkit = { git = "https://github.com/tanguy-c/iotkit", default-features = false, features = ["esp32c6", "wifi", "mqtt", "improv-serial", "ota-push"] }
```

Without it, `iotkit::mqtt::esp` and the `mqtt` console command are gone, and the `BROKER` health
check is not required. `deep-sleep` needs `mqtt` or `esphome-api`.

### The setup access point's tasks are gone

`provision::esp::{dhcp_task, dns_task, portal_task}` now run inside `window_task`, only while a
setup window is open. `iotkit::start` spawns it; an app that spawned them itself drops those
three (it keeps `portal_job_task`).

## To 0.3 (from 0.2)

### `Meta` has a `remember` field

Only for an app with its own entity type (`impl Entity`), whose `Meta` lists every field; the
entity builders need no change. Add the field:

```rust
Meta {
    // ...
    precision: None,
    remember: false, // add this line
}
```

## To 0.2 (from 0.1)

### `matter_task` takes the firmware

Only for an app that starts Matter itself (without `iotkit::start`): the version Matter reports
now comes from the firmware, as Matter updates compare it.

```rust
let product = Product { name: "My light", version: env!("CARGO_PKG_VERSION"), version_number: 1 }; // remove
spawner.spawn(matter_task(peripherals, store, "My light", iotkit::firmware!(&OTA_KEY), name, device).unwrap());
```

Over Thread, `iotkit::start` now does it all with the Thread `Board`: see
[Home Assistant](home-assistant.md#matter-over-thread).

### `Board` has no `flash` field

`iotkit::init!` now takes the flash: it needs it to roll back an update that crashes before
`iotkit::start`. The build fails with `struct Board has no field named flash` (or
`use of moved value: p.FLASH`) until you remove the field:

```rust
iotkit::init!(p);
let board = Board {
    flash: p.FLASH, // remove this line
    wifi: p.WIFI,
    // ...
};
```

To use the flash yourself, go through iotkit's: a second driver on it does not work (esp-storage
allows one).

- Your own settings: a [`Setting`](entities.md) with `app.load()` and `app.save()`.
- Your own partition (add it to `partitions.csv`): find it, then read and write it with
  `iotkit::config::esp::with`:

```rust
let range = iotkit::config::esp::find_partition("logs").expect("no `logs` partition");
let mut buf = [0u8; 64];
iotkit::config::esp::with(|flash| flash.read(range.start, &mut buf)).unwrap();
```

An app that drives the flash entirely by itself does not use `init!` or `start`: see
[ble-beacon](examples.md), which sets the chip up itself.

### `iotkit::start` needs `iotkit::init!`

`start` uses the flash `init!` sets up. Every example and generated project already calls `init!`
first; an app that set the chip up itself and then called `start` panics with `the flash is not
set up`: call `init!` instead.
