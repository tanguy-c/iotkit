# Battery devices

The `deep-sleep` feature (MQTT only, not with `matter-wifi`) starts a device that wakes up,
reports and sleeps again. `cargo iotkit new` writes one when you answer "battery";
`apps/sleepy` is the example.

```rust,ignore
static SLEEP: SleepConfig = SleepConfig { interval: Duration::from_secs(300), max_awake: Duration::from_secs(30) };

let app = iotkit::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;
TEMPERATURE.set(reading);
app.sleep().await   // sends the states, disconnects, sleeps `interval`
```

- **A wake-up is short** (about 2 s from wake to sleep on the C6): the access point, channel and
  address of the last wake-up are kept in RTC memory (no scan, no DHCP; the lease is reused for 30
  minutes), boots are not written to flash, and discovery is sent only when it changed.
- **In Home Assistant** the device has no availability (asleep is not offline); its readings
  expire after twice the interval. Commands are retained, so a switch turned on while the device
  sleeps applies at its next wake-up; one-shot commands (buttons, *Install*) are cleared once
  received.
- **Staying awake for maintenance** is the app's choice (`apps/sleepy` has a retained *Stay
  awake* switch). The device also stays awake while a setup window is open or an update installs,
  so *Install* in Home Assistant works on a sleeping device: at its next wake-up.
- **No broker:** if it is not reached within `max_awake`, the device sleeps 2, 4 then 8 times the
  interval, and after two failures forgets the access point and address.
- **Wake reason:** `app.wake_reason` (`PowerOn`, `Timer`, `Pin`); a pin armed with esp-hal's
  `Input::wakeup_enable` wakes it too (on the C6 only GPIO0-7, so not BOOT).
- **Battery level:** `iotkit::battery::Battery` reads an ADC1 pin behind a divider, with a LiPo,
  2×AA alkaline or custom discharge curve, before Wi-Fi starts (the radio's load lowers the
  voltage): `Battery::new(p.ADC1, p.GPIO2, 2.0).report(&BATTERY, Some(&VOLTAGE))`.

Deep sleep builds on the C3, C5, C6 and S3; it was tested on the C6. The USB serial port
disappears while the chip sleeps: a monitor has to reopen it.
