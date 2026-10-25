# Battery devices

How a device saves its battery depends on how it talks to the home. `cargo iotkit new` picks the
mode when you answer "battery":

| Link | Mode | Feature | A command reaches it | One charge |
| --- | --- | --- | --- | --- |
| MQTT | deep sleep between reports | `deep-sleep` | at the next wake-up (the broker keeps it) | months |
| ESPHome API | deep sleep between reports | `deep-sleep` + `esphome-api` | only while awake | months |
| Matter over Thread | sleepy end device, CPU in light sleep | `matter-icd` | within 5 s | not yet: the receiver stays on ([below](#over-thread)) |
| Matter over Wi-Fi | not yet ([below](#over-wi-fi-not-yet)) | | | |

The battery level works the same in every mode except Thread (below): `iotkit::battery::Battery`
reads an ADC1 pin behind a divider, with a LiPo, 2×AA alkaline or custom discharge curve:
`Battery::new(p.ADC1, p.GPIO2, 2.0).report(&BATTERY, Some(&VOLTAGE))`. Over Matter, a `battery`
sensor is the node's Power Source.

## Deep sleep (MQTT, ESPHome API)

The `deep-sleep` feature starts a device that wakes up, reports and sleeps again; `apps/sleepy`
is the example.

```rust,ignore
static SLEEP: SleepConfig = SleepConfig { interval: Duration::from_secs(300), max_awake: Duration::from_secs(30) };

let app = iotkit::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;
TEMPERATURE.set(reading);
app.sleep().await   // sends the states, disconnects, sleeps `interval`
```

- **A wake-up is short** (about 2 s from wake to sleep on the C6 over MQTT): the access point,
  channel and address of the last wake-up are kept in RTC memory (no scan, no DHCP; the lease is
  reused for 30 minutes), boots are not written to flash, and discovery is sent only when it
  changed.
- **Over MQTT** the device has no availability in Home Assistant (asleep is not offline); its
  readings expire after twice the interval plus a minute. Commands are retained, so a switch
  turned on while the device sleeps applies at its next wake-up; one-shot commands (buttons,
  *Install*) are cleared once received.
- **Over the ESPHome API** (`esphome-api`) the device tells Home Assistant it sleeps, which then
  keeps its last states instead of showing it unavailable. At each wake-up it announces itself
  over mDNS, Home Assistant connects, and the device sleeps once the states are sent. Commands
  reach it only while it is awake: nothing holds them while it sleeps.
- **Staying awake for maintenance** over MQTT is the app's choice (`apps/sleepy` has a *Stay
  awake* switch, retained by the broker). The device also stays awake while a setup window is
  open or an update installs, so over MQTT *Install* in Home Assistant works on a sleeping device:
  at its next wake-up. Over the ESPHome API, see [below](#updates-and-maintenance-over-the-esphome-api).
  After an update it stays awake until the new firmware is healthy, or rolled back after its
  [timeout](updates.md#what-the-device-does).
- **No link** (broker or Home Assistant) within `max_awake`: the device sleeps 2, 4 then 8 times
  the interval, and after two failures forgets the access point and address. With `esphome-api`
  and a stored broker, either link counts.
- **Pairing mode** keeps it awake for 10 minutes, time to add it to Home Assistant and set it
  up: at power-on until it has reached its link once, and when the button is held 3 s, which
  also opens the setup window. A short press ends it sooner. A [status LED](components.md#status-led) blinks meanwhile.
- **The button** (`Board::boot_button`) wakes the chip when it is on a low-power pin (GPIO0-7 on
  the C6, GPIO0-5 on the C3; not the devkits' BOOT): a short press wakes it for a report (its
  `wake_reason` is `Pin`), a 3 s hold, counted from the wake-up, starts pairing mode. Wire it to
  GND with an external pull-up: the internal one may not hold in deep sleep. Elsewhere, the
  device warns at power-on that it cannot wake it.
- **While the button is down** the device does not sleep: a hold started during a wake-up reaches
  pairing mode even on a pin that cannot wake the chip.
- **Wake reason:** `app.wake_reason` (`PowerOn`, `Timer`, `Pin`, `Other`); a pin armed with esp-hal's
  `Input::wakeup_enable` wakes it too (on the C6 only GPIO0-7, so not BOOT).
- **Read the battery before Wi-Fi starts:** the radio's load lowers the voltage.

Deep sleep builds on the C3, C5, C6 and S3; it was tested on the C6. The USB serial port
disappears while the chip sleeps: a monitor has to reopen it.

### Updates and maintenance over the ESPHome API

Home Assistant holds no command for a sleeping ESPHome device (neither does ESPHome itself: its
docs keep a device awake with a retained MQTT message). A switch toggled or *Install* clicked
while the device sleeps fails, so there is no *Stay awake* switch over the API.

**Updates install by themselves** (with `ha-update`), at a set time, two settings of the device
in Home Assistant say when:

- *Auto-update* (on by default): off, a release waits for a click, as below.
- *Update time* (02:00 by default, `IOTKIT_UPDATE_TIME` at build time): a half hour of the day,
  or *At wake-up*. For a set time, the wake-up that finds the release downloads and checks it at
  once (the update entity then reads "Installs at 02:00"), the device wakes up at that time and
  installs it, then runs it on trial as any update. Home Assistant gives the time and its time
  zone at each connection; the RTC keeps it through deep sleep. After a power loss, the time is
  known again at the next connection.

To change a setting, or for anything else, keep it awake with the button, then act in Home
Assistant:

1. **Wake it and start pairing mode:** hold the button 3 s. On a low-power pin the press wakes
   the chip; on BOOT (which cannot), press RESET, release it, then hold BOOT right away. The
   status LED goes solid at 3 s, then blinks: the device is awake.
2. **Act within 10 minutes:** once Home Assistant has reconnected, click *Install* on the update
   entity, change a setting, or use the console. An install keeps the device awake to its end,
   then through its trial.
3. **Done:** a short press ends pairing mode; else the device goes back to sleep after the 10
   minutes.

Pairing mode also opens the setup window (access point, Improv), which costs power while it lasts.

## Matter on battery (`matter-icd`)

Matter keeps sessions with its controllers, so a Matter device does not deep-sleep: it stays
reachable as an *Intermittently Connected Device* with a short idle time (SIT). It advertises how
long it may take to answer (`SII`, `SAI` in DNS-SD), and controllers wait for it. Add the feature
next to `matter-thread`; the app starts with `iotkit::start` as usual.

### Over Thread

The device is a *sleepy end device*: it polls its parent (the border router or a mains-powered
Thread device) for the messages it keeps, every 5 s, or every 300 ms for 4 s after data went
either way, so an exchange is quick once started. Between polls the radio should be off; it is not
yet (below). Between polls
the CPU light-sleeps: `init!` starts the RTOS with esp-rtos's automatic light sleep (it takes
`LPWR`). The Bluetooth stack, while a pairing window is open, and the radio, while it listens or
sends, keep the CPU awake. The BOOT button is watched by its interrupt, not polled.

- **Latency:** a command waits up to the poll period. `IOTKIT_ICD_POLL_MS` at build time sets it
  (500 to 15000 ms, 5000 by default): shorter answers faster, longer lasts longer.
- **Your code:** whatever wakes the CPU costs battery. Wait on interrupts or long timers; avoid
  loops on short timers.
- **No battery level yet:** without Wi-Fi, ADC1 feeds the random number generator, so no ADC
  reading over Thread (this applies to analog parts too).
- `apps/thread-light --features iotkit/matter-icd` builds it; `xtask/answers/thread-battery.toml`
  is a generated example.
- **Not a battery device yet:** openthread 0.4's ESP radio driver does nothing when OpenThread
  puts the radio to sleep (esp-radio has no call to stop receiving), so the receiver stays on
  between polls while the CPU sleeps. Polling, latency and the ICD behaviour are as described,
  but a charge lasts about as long as with the radio always on: a day or so, not months. Frames
  that come while "asleep" are delivered late. The kit takes it from there once the driver can
  stop the receiver.

### Over Wi-Fi: not yet

`matter-icd` with `matter-wifi` does not build yet. On a battery the Wi-Fi radio would use its
power save mode, sleeping between the access point's DTIM beacons. esp-radio 1.0.0-beta.1 stops
the clock of the Wi-Fi power controller each time the modem dozes on the C6, so the radio never
wakes for a beacon: the device drops off the network (tested: 55 to 100 % of pings lost, replies
up to 30 s late). esp-hal's `main` branch has the fix; the kit lifts the restriction with the
esp-radio release that ships it. Even then, the Wi-Fi and Bluetooth drivers keep the CPU awake (no
light sleep), so expect days to weeks per charge. For months on a battery today, use Wi-Fi
without Matter (deep sleep).

### Not yet

Long idle time (LIT) devices, which sleep for minutes and send Check-In messages to their
registered controllers, are not supported: rs-matter has the ICD Management cluster, the kit does
not use it yet.

`matter-icd` is a build-time choice tested on the C6 for building only; measure the current of
your board before relying on it.
