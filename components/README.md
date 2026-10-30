# Components

The parts `cargo iotkit new` and `cargo iotkit add` offer: one TOML file per part, its name the
part's id (`kind = "relay"` in an answers file, `cargo iotkit add relay`). The CLI embeds this
folder when it is built; `cargo iotkit components` lists them.

Each part becomes a module of the generated project, `src/parts/<entity id>.rs`: its entity, a
`spawn` that `main` calls with its pins, and its task. With a `[code]` table, the part works as
generated (a ready-made part from `iotkit::components`); otherwise its task is the entity type's
loop with a `TODO(<entity id>)` and the `[todo]` hints.

```toml
label  = "Relay / on-off (lamp, pump, socket)"  # the menu, searched as you type
group  = "controls"            # its section: measures, detects, controls or inputs
order  = 10                    # place in its section (lowest first)
name   = "Relay"               # the entity's default name
wiring = "the relay module's IN pin"   # README's wiring table: what to connect
matter = "On/off plug"         # the Matter device type it shows as; left out: none
sources = ["https://..."]      # for a sensor chip: two at least (see below)

[entity]                       # static RELAY: Switch = Switch::new("relay", "Relay");
type    = "Switch"             # an iotkit entity type (default `Sensor`): its task's loop comes with it
ctor    = "new"                # the default; or a shortcut such as `temperature`
args    = ""                   # after the id and the name: `, 0.0, 100.0, 1.0`
builder = ""                   # calls after it: `.device_class(DeviceClass::DOOR)`

[[pins]]                       # one per pin; none for a value (a setting, a mode)
role = "pin"                   # the default: its variable in the code; `pin = 4` sets the first
use  = "output"                # input, output or analog (an ADC1 pin): checked against the chip

[todo]                         # without ready-made code (or its pin), on battery
text    = "switch the relay to `on`"
buses   = ["gpio-out"]         # a setup hint each: gpio-in, gpio-out, adc, i2c, one-wire, uart, ledc, rmt
drivers = ""                   # driver crates (no_std, embedded-hal 1.0)

# Ready-made code (on a plugged-in device, every pin chosen). Liquid: {{ entity }} (its static),
# {{ id }}, {{ chip }} and {{ pins.<role> }} (the GPIO number). `spawn` gets each pin by its role
# (`pin: GPIO4<'static>`) and each of `takes` lowercased (`adc1: ADC1<'static>`).
[code]
uses     = ["esp_hal::gpio::Output", "iotkit::components"]  # one path each; the entity's type and
                               # `DeviceClass`/`Unit` come by themselves
takes    = []                  # peripherals only one part can have (`ADC1`): the next gets the TODO
features = []                  # iotkit features it needs (`smartled`)
setup    = ""                  # lines in `spawn` before the task starts
param    = "pin: Output<'static>"                               # the task's parameters
arg      = "Output::new(pin, Level::Low, OutputConfig::default())"  # what `spawn` passes it
body     = "components::relay(&{{ entity }}, pin, Active::High).await"
# On a battery device, a reading (`Sensor`, `BinarySensor`) is read once per wake-up: `read` is the
# body of the `read` fn `main` calls (same parameters as `spawn`), `read_uses` its `use` paths.
# Without it, a TODO there.
read      = "{{ entity }}.set(Input::new(pin, InputConfig::default()).is_high());"
read_uses = ["esp_hal::gpio::Input", "esp_hal::gpio::InputConfig"]
```

A sensor chip on I²C has no `[[pins]]` or `[code]`: it is on the project's bus (asked once), and
`[i2c]` reads it, the same plugged in (every 30 s) and on battery (once per wake-up). One
measuring several values has an entity each, named after the part (`Air temperature`):

```toml
[[entities]]                   # one per value; `[entity]` for a chip with one
key    = "temperature"         # its id after the part's: `air_temperature`
name   = "temperature"         # its name after the part's: "Air temperature"
ctor   = "temperature"
matter = "Temperature sensor"
value  = "r.temperature"       # its value in the reading `r` (default `r`)

[i2c]
addresses = [0x44, 0x45]       # the default first; `address = 0x45` in an answers file
read      = "components::i2c::sht4x({{ i2c }}, ADDR).await"   # an Option of the reading
```

A file for a given chip (an SHT3x, a BME280) checks its bus, address and pins against two
sources, as `boards/` does: ESPHome's component (`esphome/components/<name>/`: `DEPENDENCIES`,
`i2c_device_schema(0x44)`, device class and unit), Zephyr's binding
(`dts/bindings/sensor/<vendor>,<chip>.yaml`) or the datasheet. Facts only: ESPHome's C++ is GPLv3.

A new I²C chip's driver goes in `iotkit::components::i2c` (stateless, host-tested on a mock bus).
A new ready-made part in `iotkit::components` comes with its file here (the `add-component`
skill). `cargo test --manifest-path xtask/Cargo.toml --target host-tuple` loads every file and
renders its code on every chip; `xtask/answers/c6.toml` should use it, so CI builds it.
