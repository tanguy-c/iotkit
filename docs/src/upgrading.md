# Upgrading

What to change in a project when a release breaks the API, newest first. The
[changelog](https://github.com/tanguy-c/iotkit/blob/main/CHANGELOG.md) lists everything else.

## To the next release (from 0.1)

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
[thread-light](examples.md), which sets the chip up itself.

### `iotkit::start` needs `iotkit::init!`

`start` uses the flash `init!` sets up. Every example and generated project already calls `init!`
first; an app that set the chip up itself and then called `start` panics with `the flash is not
set up`: call `init!` instead.
