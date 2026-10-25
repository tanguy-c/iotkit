# Components and drivers

`iotkit::components` has ready-made parts. Each is an `async fn` that drives one entity from
embedded-hal hardware (esp-hal's `Output`, `Input` and LEDC channels implement the traits). The
app awaits it in its own task, so a part it does not use is not in the firmware.

```rust,ignore
#[embassy_executor::task]
async fn pump_task(pin: Output<'static>) {
    components::relay(&PUMP, pin, Active::High).await
}
// in main:
spawner.spawn(pump_task(Output::new(p.GPIO4, Level::Low, OutputConfig::default())).unwrap());
```

| Part | Entity | Hardware |
| --- | --- | --- |
| `relay` | `Switch` | an output pin (`Active::Low` for boards switched by a low input) |
| `binary_input` | `BinarySensor` | an input pin, debounced: contact, motion, leak |
| `button` | `Event` | an input pin: `press`, `double_press`, `triple_press`, `long_press` (the types the event has) |
| `dimmer` | `Light` | a PWM channel; the duty follows brightness² |
| `fan` | `Fan` | a PWM channel; the duty is the speed |
| `rgb_light` | `Light` | WS2812 LEDs (feature `smartled`, `esp-hal-smartled` on an RMT channel) |
| `poll` | `Sensor` | any read function, every interval, through filters |

`cargo iotkit new` uses them where it can, and leaves a `TODO` with hints for the other parts.

## Status LED

`iotkit::status` says what the device is doing, in two families:

- **Pairing:** `Pairing` (a setup or pairing window is open, or a battery device is in pairing
  mode) and `Held` while the button is held past 3 s or 10 s.
- **Diagnostic:** `Connecting`, `Online`, `Offline`, `Installing` and `Trial` (an update not kept
  yet).

A LED shows one family or both (`status::Show::Pairing`, `Diagnostic` or `All`, pairing first),
with a blink pattern per status (`components::pattern`). On a battery device, `Show::Pairing`
keeps it dark at each wake-up and lights it only to set the device up. Two parts show it:

```rust,ignore
#[embassy_executor::task]
async fn status_task(led: Output<'static>) {
    components::status_led(led, Active::High, Show::All).await // a LED and a resistor to GND
}
// or the board's WS2812 (feature `smartled`), in colours (`components::color`):
//     components::status_rgb(rmt_leds, 1, 32, Show::Pairing).await
```

To show it another way, follow it: `let mut status = iotkit::status::watch(Show::All);` then
`status.next().await` in a loop (at once, then at each change; `None` when nothing of the family
is to be shown; up to 4 watchers). Both parts leave the LED dark between patterns and before a
deep sleep or restart, so a battery device sleeps; a WS2812 still draws about 1 mA dark.
`cargo iotkit new` asks for one (`status = "rgb"` or `"led"` with `status_pin`).

## Filters

`poll` passes each reading through a filter chain; `None` drops it.

```rust,ignore
use components::filter::{Clamp, Delta, Filter, Linear, MovingAverage};

let percent = Linear::new((3000.0, 0.0), (1200.0, 100.0))   // two calibration points: raw, value
    .then(Clamp::new(0.0, 100.0))
    .then(MovingAverage::<4>::new())
    .then(Delta::new(1.0))                                   // report changes of 1 % or more
    .then(|v: f32| (v > 0.5).then_some(v));                  // any closure
```

Also `Offset` and `Multiply`.

## Any embedded-hal driver

Driver crates take the bus esp-hal gives (I²C, SPI, UART, 1-Wire). The read function of `poll`
is plain async code, so any `no_std` embedded-hal 1.0 driver fits:

```rust,ignore
#[embassy_executor::task]
async fn temperature_task(i2c: I2c<'static, Async>) {
    let mut sensor = SomeDriver::new(i2c);
    components::poll(&TEMPERATURE, Duration::from_secs(30), MovingAverage::<4>::new(), async || {
        sensor.read_celsius().await.ok()   // `None` on an error: the last value stays
    })
    .await
}
// in main:
let i2c = I2c::new(p.I2C0, Default::default()).unwrap().with_sda(p.GPIO6).with_scl(p.GPIO7).into_async();
spawner.spawn(temperature_task(i2c).unwrap());
```

One reading for several entities (temperature and humidity from one chip) is a loop:

```rust,ignore
let mut ticker = Ticker::every(Duration::from_secs(30));
loop {
    if let Ok(m) = sensor.measure().await {
        TEMPERATURE.set(m.celsius);
        HUMIDITY.set(m.humidity);
    }
    ticker.next().await;
}
```

`cargo iotkit new` names checked driver crates for its parts (`sht4x`, `bme280-rs`, `scd4x`,
`onewire` for the DS18B20, ...). Prefer async drivers; a short blocking bus transaction is fine,
a busy loop is not.

## Your own part

A part is an `async fn` over embedded-hal traits that never returns: wait for a command or an
edge, drive the hardware, report the state. Keep it free of registries or lists of every part, so
it stays out of firmware that does not use it. The repository's `add-component` checklist
(`.claude/skills/add-component`) covers tests and the wizard.
