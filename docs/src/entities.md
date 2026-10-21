# Entities and settings

## Entities

An entity is a `static`: what Home Assistant (and Matter) shows. Any task sets its state or waits
for its commands; `start` keeps Home Assistant in step.

```rust,ignore
static TEMPERATURE: Sensor = Sensor::temperature("temperature", "Temperature");
static RELAY: Switch = Switch::new("relay", "Relay");
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY)).entities(&[&TEMPERATURE, &RELAY]);

TEMPERATURE.set(21.5);
let on = RELAY.wait_command().await;   // from Home Assistant or Matter
RELAY.set(on);                         // report what the device did
```

| Kind | State | Commands |
| --- | --- | --- |
| `Sensor` | a number | |
| `BinarySensor` | on/off | |
| `TextSensor` | text | |
| `Event` | press, double press, ... | |
| `Switch` | on/off | on/off |
| `Light` | on, brightness, colour | the same |
| `Button` | | press |
| `Number` | a value in a range | a value |
| `Select` | one of a list | one of a list |
| `Cover`, `Valve` | open/closed, position | open, close, stop, position |
| `Lock` | locked/unlocked | lock, unlock, open |
| `Fan` | on, speed, preset, oscillation, direction | the same |
| `Climate` | mode, target, current temperature | mode, target |

With `ha-update`, the kit adds an `Update` entity of its own ([updates](updates.md)). Ids are
`[a-z0-9_]`, checked when building. `apps/showcase` has most of them.

## Device classes, units, shortcuts

```rust,ignore
static DOOR: BinarySensor = BinarySensor::new("door", "Door").device_class(DeviceClass::DOOR);
static LEVEL: Sensor = Sensor::new("level", "Tank level").unit(Unit::PERCENT).measurement();
static PH: Sensor = Sensor::new("ph", "pH").device_class(DeviceClass::custom("ph"));
```

`DeviceClass` and `Unit` are constants as Home Assistant spells them; `custom` takes any other.
The class decides the icon, statistics and the Matter device type. Common sensors have
shortcuts with their class, unit and decimals: `Sensor::temperature`, `humidity`, `pressure`,
`illuminance`, `carbon_dioxide`, `pm25`, `battery`, `voltage`, `power`. Other builders:
`.icon("mdi:...")`, `.precision(n)`, `.diagnostic()`, `.config()` (where Home Assistant files it).

## The device

```rust,ignore
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))   // name and version from Cargo.toml
    .manufacturer("Me")
    .model("Garden sensor v2")
    .entities(&[&TEMPERATURE, &RELAY])
    .watches(&[&MAIN_WATCH])        // tasks that must keep feeding a watchdog
    .checks(&[RUNNING]);            // the app's own health checks (see updates)
```

## Settings

Values the device keeps across restarts are typed and versioned:

```rust,ignore
#[derive(Serialize, Deserialize)]
struct Brightness(u8);
impl Setting for Brightness { const KEY: u16 = 0x1000; const VERSION: u8 = 1; }

let app = iotkit::start(spawner, board, &CONFIG).await;
let level = app.load::<Brightness>().await.unwrap_or(Brightness(128));
app.save(&Brightness(200)).await;   // written only when it changed
```

Keys from `0x1000` are the app's. Bump `VERSION` when the type changes, and convert old values in
`Setting::migrate`. `SaveLater` tells when to save, a while after the last change (a dragged
slider writes the flash once).
