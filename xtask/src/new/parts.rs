//! The parts a builder can add: what Home Assistant (and Matter) sees, and the TODO left in
//! the generated code.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Measures,
    Detects,
    Controls,
    Inputs,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Measures => "Measures",
            Group::Detects => "Detects",
            Group::Controls => "Controls",
            Group::Inputs => "Inputs",
        }
    }
}

/// How the generated code drives the part.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stub {
    /// A reading (`Sensor::set`), every 30 s.
    Reading,
    /// Something detected (`BinarySensor::set`).
    Detect,
    /// Presses (`Event::trigger`).
    Presses,
    Switch,
    Light,
    Cover,
    Lock,
    Fan,
    Climate,
    Number,
    Select,
}

/// How a part talks to the chip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bus {
    GpioIn,
    GpioOut,
    Adc,
    I2c,
    OneWire,
    Uart,
    Ledc,
    Rmt,
}

impl Bus {
    /// The esp-hal 1.2 setup, on one line, for `pin` (the ADC needs `p.GPIOn`, not an `AnyPin`).
    pub fn hint(self, pin: &str) -> String {
        match self {
            Bus::GpioIn => format!("GPIO: `Input::new({pin}, InputConfig::default().with_pull(Pull::Up))`"),
            Bus::GpioOut => {
                format!(
                    "GPIO: `Output::new({pin}, Level::Low, OutputConfig::default())` once, then `set_high()`/`set_low()`"
                )
            }
            Bus::Adc => {
                format!("ADC: `config.enable_pin({pin}, Attenuation::_11dB)`, `Adc::new(p.ADC1, config).into_async()`")
            }
            Bus::I2c => {
                format!("I²C: `I2c::new(p.I2C0, Default::default()).unwrap().with_sda({pin}).with_scl(…).into_async()`")
            }
            Bus::OneWire => {
                format!(
                    "1-Wire: `Flex::new({pin})`, `apply_output_config` with `DriveMode::OpenDrain`, input and output enabled"
                )
            }
            Bus::Uart => format!(
                "UART: `Uart::new(p.UART1, cfg).unwrap().with_rx({pin}).into_async()`, `cfg`: `uart::Config::default().with_baudrate(9600)`"
            ),
            Bus::Ledc => format!(
                "LEDC: `Ledc::new(p.LEDC)`, `timer::<LowSpeed>(Timer0)`, `channel(Channel0, {pin})`, `set_duty(percent)`"
            ),
            Bus::Rmt => format!(
                "RMT: `Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async()` (one per chip), its `channel1` on `{pin}`"
            ),
        }
    }
}

pub struct Kind {
    /// In answers files: `kind = "temperature"`.
    pub id: &'static str,
    pub group: Group,
    /// In the menu.
    pub label: &'static str,
    /// The default entity name.
    pub name: &'static str,
    /// The entity type, its constructor (`new`, or a shortcut such as `temperature`), and the
    /// builder calls after `::ctor(id, name)`.
    pub ty: &'static str,
    pub ctor: &'static str,
    pub new_args: &'static str,
    pub builder: &'static str,
    /// Read with the ADC (an ADC1 pin).
    pub analog: bool,
    /// What to connect to the pin (README's wiring table).
    pub wiring: &'static str,
    /// The TODO in the generated code: what to do, in the part's terms.
    pub todo: &'static str,
    /// How it can be wired (alternatives): one setup hint each under the TODO.
    pub buses: &'static [Bus],
    /// Driver crates (no_std, embedded-hal 1.0), checked on crates.io.
    pub drivers: &'static str,
    pub stub: Stub,
    /// The Matter device type it shows as, if any.
    pub matter: Option<&'static str>,
}

pub const KINDS: &[Kind] = &[
    // Measures
    Kind {
        id: "temperature",
        group: Group::Measures,
        label: "Temperature",
        name: "Temperature",
        ty: "Sensor",
        ctor: "temperature",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "the sensor's data pin (DS18B20 on 1-Wire with a 4.7 kΩ pull-up), or SDA of an I²C sensor (BME280, SHT31)",
        todo: "read °C",
        buses: &[Bus::I2c, Bus::OneWire],
        drivers: "`sht4x`, `embedded-sht3x`, `bme280-rs`; DS18B20: `onewire` (blocking)",
        stub: Stub::Reading,
        matter: Some("Temperature sensor"),
    },
    Kind {
        id: "humidity",
        group: Group::Measures,
        label: "Humidity",
        name: "Humidity",
        ty: "Sensor",
        ctor: "humidity",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "SDA of an I²C sensor (BME280, SHT31, AHT20)",
        todo: "read % RH",
        buses: &[Bus::I2c],
        drivers: "`sht4x`, `embedded-sht3x`, `bme280-rs`, `aht20-async`",
        stub: Stub::Reading,
        matter: Some("Humidity sensor"),
    },
    Kind {
        id: "pressure",
        group: Group::Measures,
        label: "Air pressure",
        name: "Pressure",
        ty: "Sensor",
        ctor: "pressure",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "SDA of an I²C sensor (BME280, BMP280)",
        todo: "read hPa",
        buses: &[Bus::I2c],
        drivers: "`bme280-rs`; BMP280: `bme280` (blocking)",
        stub: Stub::Reading,
        matter: Some("Pressure sensor"),
    },
    Kind {
        id: "light_level",
        group: Group::Measures,
        label: "Light level",
        name: "Illuminance",
        ty: "Sensor",
        ctor: "illuminance",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "SDA of an I²C light sensor (BH1750, VEML7700)",
        todo: "read lux",
        buses: &[Bus::I2c],
        drivers: "`bh1750` (blocking), `veml7700`",
        stub: Stub::Reading,
        matter: Some("Light sensor"),
    },
    Kind {
        id: "co2",
        group: Group::Measures,
        label: "CO2",
        name: "CO2",
        ty: "Sensor",
        ctor: "carbon_dioxide",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "SDA of an SCD4x (I²C), or TX of a MH-Z19 (UART)",
        todo: "read ppm",
        buses: &[Bus::I2c, Bus::Uart],
        drivers: "SCD4x: `scd4x`; MH-Z19: `mh-z19` (frames only; add `.with_tx`)",
        stub: Stub::Reading,
        matter: Some("Air quality sensor"),
    },
    Kind {
        id: "pm25",
        group: Group::Measures,
        label: "Fine dust (PM2.5)",
        name: "PM2.5",
        ty: "Sensor",
        ctor: "pm25",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "TX of a PMS5003 / SDS011 (UART)",
        todo: "read PM2.5 in µg/m³",
        buses: &[Bus::Uart],
        drivers: "PMS5003: `pmsx003` (blocking `embedded-io`)",
        stub: Stub::Reading,
        matter: Some("Air quality sensor"),
    },
    Kind {
        id: "soil_moisture",
        group: Group::Measures,
        label: "Soil moisture (analog probe)",
        name: "Soil moisture",
        ty: "Sensor",
        ctor: "new",
        new_args: "",
        builder: ".unit(Unit::PERCENT).device_class(DeviceClass::MOISTURE).measurement()",
        analog: true,
        wiring: "the probe's analog output (AOUT)",
        todo: "read the probe, map dry..wet to 0..100 %",
        buses: &[Bus::Adc],
        drivers: "",
        stub: Stub::Reading,
        matter: None,
    },
    Kind {
        id: "number",
        group: Group::Measures,
        label: "Another reading (a number)",
        name: "Reading",
        ty: "Sensor",
        ctor: "new",
        new_args: "",
        builder: ".measurement()",
        analog: false,
        wiring: "your sensor",
        todo: "read the value (and add `.unit(\"...\")`, `.device_class(\"...\")` to the entity)",
        buses: &[],
        drivers: "",
        stub: Stub::Reading,
        matter: None,
    },
    // Detects
    Kind {
        id: "motion",
        group: Group::Detects,
        label: "Motion (PIR sensor)",
        name: "Motion",
        ty: "BinarySensor",
        ctor: "new",
        new_args: "",
        builder: ".device_class(DeviceClass::MOTION)",
        analog: false,
        wiring: "the PIR sensor's OUT pin (HC-SR501, AM312)",
        todo: "high while the PIR sees motion",
        buses: &[Bus::GpioIn],
        drivers: "",
        stub: Stub::Detect,
        matter: Some("Occupancy sensor"),
    },
    Kind {
        id: "contact",
        group: Group::Detects,
        label: "Door or window (reed switch)",
        name: "Door",
        ty: "BinarySensor",
        ctor: "new",
        new_args: "",
        builder: ".device_class(DeviceClass::DOOR)",
        analog: false,
        wiring: "one side of the reed switch (the other to GND)",
        todo: "low while closed (`true` = open)",
        buses: &[Bus::GpioIn],
        drivers: "",
        stub: Stub::Detect,
        matter: Some("Contact sensor"),
    },
    Kind {
        id: "leak",
        group: Group::Detects,
        label: "Water leak",
        name: "Leak",
        ty: "BinarySensor",
        ctor: "new",
        new_args: "",
        builder: ".device_class(DeviceClass::MOISTURE)",
        analog: false,
        wiring: "one probe of the leak sensor (the other to GND)",
        todo: "low when the probes are wet",
        buses: &[Bus::GpioIn],
        drivers: "",
        stub: Stub::Detect,
        matter: Some("Water leak detector"),
    },
    Kind {
        id: "smoke",
        group: Group::Detects,
        label: "Smoke alarm",
        name: "Smoke",
        ty: "BinarySensor",
        ctor: "new",
        new_args: "",
        builder: ".device_class(DeviceClass::SMOKE)",
        analog: false,
        wiring: "the alarm's interconnect / relay output",
        todo: "read the alarm's output",
        buses: &[Bus::GpioIn],
        drivers: "",
        stub: Stub::Detect,
        matter: Some("Smoke alarm"),
    },
    // Controls
    Kind {
        id: "relay",
        group: Group::Controls,
        label: "Relay / on-off (lamp, pump, socket)",
        name: "Relay",
        ty: "Switch",
        ctor: "new",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "the relay module's IN pin",
        todo: "switch the relay to `on`",
        buses: &[Bus::GpioOut],
        drivers: "",
        stub: Stub::Switch,
        matter: Some("On/off plug"),
    },
    Kind {
        id: "light",
        group: Group::Controls,
        label: "Light (dimmable / colour LED strip)",
        name: "Light",
        ty: "Light",
        ctor: "new",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "DIN of a WS2812 strip, or the gate of a MOSFET for a single-colour strip",
        todo: "drive the light from `state.on`, `state.brightness`, `state.rgb`",
        buses: &[Bus::Rmt, Bus::Ledc],
        drivers: "WS2812: `esp-hal-smartled` (as the status LED)",
        stub: Stub::Light,
        matter: Some("Extended colour light"),
    },
    Kind {
        id: "cover",
        group: Group::Controls,
        label: "Blind / curtain / garage door",
        name: "Blind",
        ty: "Cover",
        ctor: "new",
        new_args: "",
        builder: ".device_class(DeviceClass::BLIND).position()",
        analog: false,
        wiring: "the motor driver's direction / enable input",
        todo: "move the motor to `position` (0 closed, 100 open; halt it on `Stop`), e.g. timed runs",
        buses: &[Bus::GpioOut],
        drivers: "",
        stub: Stub::Cover,
        matter: Some("Window covering"),
    },
    Kind {
        id: "valve",
        group: Group::Controls,
        label: "Water valve",
        name: "Valve",
        ty: "Valve",
        ctor: "new",
        new_args: "",
        builder: ".device_class(DeviceClass::WATER)",
        analog: false,
        wiring: "the valve's relay or driver input",
        todo: "open or close the valve (`position`: 0 closed, 100 open)",
        buses: &[Bus::GpioOut],
        drivers: "",
        stub: Stub::Cover,
        matter: Some("Water valve"),
    },
    Kind {
        id: "lock",
        group: Group::Controls,
        label: "Lock (door strike, smart lock)",
        name: "Lock",
        ty: "Lock",
        ctor: "new",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "the strike's relay or driver input",
        todo: "drive the strike's relay",
        buses: &[Bus::GpioOut],
        drivers: "",
        stub: Stub::Lock,
        matter: Some("Door lock"),
    },
    Kind {
        id: "fan",
        group: Group::Controls,
        label: "Fan",
        name: "Fan",
        ty: "Fan",
        ctor: "new",
        new_args: "",
        builder: ".speed()",
        analog: false,
        wiring: "the fan's PWM input or its MOSFET gate",
        todo: "run the fan at `wanted` (on, speed in %; PWM at 25 kHz)",
        buses: &[Bus::Ledc],
        drivers: "",
        stub: Stub::Fan,
        matter: Some("Fan"),
    },
    Kind {
        id: "thermostat",
        group: Group::Controls,
        label: "Thermostat (heater control)",
        name: "Thermostat",
        ty: "Climate",
        ctor: "new",
        new_args: ", &[HvacMode::Heat, HvacMode::Off]",
        builder: "",
        analog: false,
        wiring: "the heater's relay input",
        todo: "read the room temperature into `state.current` (as for a temperature part), then switch the heater towards `state.target` (set `state.action`)",
        buses: &[Bus::GpioOut],
        drivers: "",
        stub: Stub::Climate,
        matter: Some("Thermostat"),
    },
    Kind {
        id: "setting",
        group: Group::Controls,
        label: "A setting (a number: threshold, duration…)",
        name: "Setting",
        ty: "Number",
        ctor: "new",
        new_args: ", 0.0, 100.0, 1.0",
        builder: "",
        analog: false,
        wiring: "(none: a value the device uses)",
        todo: "use the new value (and keep it across restarts with `App::save`, if needed)",
        buses: &[],
        drivers: "",
        stub: Stub::Number,
        matter: None,
    },
    Kind {
        id: "mode",
        group: Group::Controls,
        label: "A mode (a choice: auto / on / off…)",
        name: "Mode",
        ty: "Select",
        ctor: "new",
        new_args: r#", &["auto", "on", "off"]"#,
        builder: "",
        analog: false,
        wiring: "(none: a choice the device uses)",
        todo: "apply the chosen mode (`index` into the options)",
        buses: &[],
        drivers: "",
        stub: Stub::Select,
        matter: Some("Mode select"),
    },
    // Inputs
    Kind {
        id: "button",
        group: Group::Inputs,
        label: "Push button (press, double press, long press)",
        name: "Button",
        ty: "Event",
        ctor: "new",
        new_args: r#", &["press", "double_press", "long_press"]"#,
        builder: ".device_class(DeviceClass::BUTTON)",
        analog: false,
        wiring: "one side of the button (the other to GND)",
        todo: "time the presses (`wait_for_falling_edge().await`, not the timer): `trigger(0)` press, `trigger(1)` double, `trigger(2)` long",
        buses: &[Bus::GpioIn],
        drivers: "",
        stub: Stub::Presses,
        matter: Some("Generic switch"),
    },
];

pub fn kind(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

impl Kind {
    /// Read once per wake-up on a battery device (rather than driven by commands).
    pub fn is_reading(&self) -> bool {
        matches!(self.stub, Stub::Reading | Stub::Detect)
    }
}

/// `"Living room temperature"` → `"living_room_temperature"` (entity id, file names).
pub fn slug(name: &str, sep: char) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c.to_ascii_lowercase(),
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with(sep) {
            out.push(sep);
        }
    }
    while out.ends_with(sep) {
        out.pop();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, if sep == '_' { "e_" } else { "d-" });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Living room Température!", '_'), "living_room_temperature");
        assert_eq!(slug("  Garden sensor ", '-'), "garden-sensor");
        assert_eq!(slug("2nd floor", '_'), "e_2nd_floor");
        assert_eq!(slug("CO2", '_'), "co2");
    }

    #[test]
    fn catalog() {
        let mut ids: Vec<_> = KINDS.iter().map(|k| k.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), KINDS.len(), "unique ids");
    }

    /// Every wired part gets a hint per bus, on the pin it is given.
    #[test]
    fn hints() {
        for k in KINDS {
            let wired = !k.wiring.starts_with("(none") && k.id != "number";
            assert_eq!(!k.buses.is_empty(), wired, "{}", k.id);
            for bus in k.buses {
                assert!(bus.hint("p.GPIO23").contains("p.GPIO23"), "{} {bus:?}", k.id);
            }
            assert_eq!(k.analog, k.buses == [Bus::Adc], "{}", k.id);
        }
    }
}
