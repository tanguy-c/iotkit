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
    /// A reading (`Sensor::set`), every 30 s; `example` is a plausible value.
    Reading {
        example: &'static str,
    },
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

pub struct Kind {
    /// In answers files: `kind = "temperature"`.
    pub id: &'static str,
    pub group: Group,
    /// In the menu.
    pub label: &'static str,
    /// The default entity name.
    pub name: &'static str,
    /// The entity type and its builder calls after `::new(id, name)`.
    pub ty: &'static str,
    pub new_args: &'static str,
    pub builder: &'static str,
    /// Read with the ADC (an ADC1 pin).
    pub analog: bool,
    /// What to connect to the pin (README's wiring table).
    pub wiring: &'static str,
    /// The TODO in the generated code.
    pub todo: &'static str,
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
        new_args: "",
        builder: r#".unit("°C").device_class("temperature").measurement().precision(1)"#,
        analog: false,
        wiring: "the sensor's data pin (DS18B20 on 1-Wire with a 4.7 kΩ pull-up), or SDA of an I²C sensor (BME280, SHT31)",
        todo: "read the temperature in °C: a DS18B20 (crate `ds18b20` + `one-wire-bus`) or an I²C sensor (crates `bme280`, `sht3x`; esp-hal's `I2c`)",
        stub: Stub::Reading { example: "21.5" },
        matter: Some("Temperature sensor"),
    },
    Kind {
        id: "humidity",
        group: Group::Measures,
        label: "Humidity",
        name: "Humidity",
        ty: "Sensor",
        new_args: "",
        builder: r#".unit("%").device_class("humidity").measurement().precision(1)"#,
        analog: false,
        wiring: "SDA of an I²C sensor (BME280, SHT31, AHT20)",
        todo: "read the relative humidity in %: an I²C sensor (crates `bme280`, `sht3x`, `aht20-driver`; esp-hal's `I2c`)",
        stub: Stub::Reading { example: "45.0" },
        matter: Some("Humidity sensor"),
    },
    Kind {
        id: "pressure",
        group: Group::Measures,
        label: "Air pressure",
        name: "Pressure",
        ty: "Sensor",
        new_args: "",
        builder: r#".unit("hPa").device_class("pressure").measurement().precision(1)"#,
        analog: false,
        wiring: "SDA of an I²C sensor (BME280, BMP280)",
        todo: "read the pressure in hPa: a BME280/BMP280 over I²C (crate `bme280`; esp-hal's `I2c`)",
        stub: Stub::Reading { example: "1013.2" },
        matter: Some("Pressure sensor"),
    },
    Kind {
        id: "light_level",
        group: Group::Measures,
        label: "Light level",
        name: "Illuminance",
        ty: "Sensor",
        new_args: "",
        builder: r#".unit("lx").device_class("illuminance").measurement()"#,
        analog: false,
        wiring: "SDA of an I²C light sensor (BH1750, VEML7700)",
        todo: "read the light level in lux: a BH1750 or VEML7700 over I²C (crates `bh1750`, `veml7700`; esp-hal's `I2c`)",
        stub: Stub::Reading { example: "320.0" },
        matter: Some("Light sensor"),
    },
    Kind {
        id: "co2",
        group: Group::Measures,
        label: "CO2",
        name: "CO2",
        ty: "Sensor",
        new_args: "",
        builder: r#".unit("ppm").device_class("carbon_dioxide").measurement()"#,
        analog: false,
        wiring: "SDA of an SCD4x (I²C), or TX of a MH-Z19 (UART)",
        todo: "read the CO2 level in ppm: an SCD40/41 over I²C (crate `scd4x`) or a MH-Z19 over UART",
        stub: Stub::Reading { example: "650.0" },
        matter: Some("Air quality sensor"),
    },
    Kind {
        id: "pm25",
        group: Group::Measures,
        label: "Fine dust (PM2.5)",
        name: "PM2.5",
        ty: "Sensor",
        new_args: "",
        builder: r#".unit("µg/m³").device_class("pm25").measurement()"#,
        analog: false,
        wiring: "TX of a PMS5003 / SDS011 (UART)",
        todo: "read PM2.5 in µg/m³: a PMS5003 or SDS011 over UART (esp-hal's `Uart`; crate `pmsx003`)",
        stub: Stub::Reading { example: "8.0" },
        matter: Some("Air quality sensor"),
    },
    Kind {
        id: "soil_moisture",
        group: Group::Measures,
        label: "Soil moisture (analog probe)",
        name: "Soil moisture",
        ty: "Sensor",
        new_args: "",
        builder: r#".unit("%").device_class("moisture").measurement()"#,
        analog: true,
        wiring: "the probe's analog output (AOUT)",
        todo: "read the probe with the ADC (esp-hal's `Adc` on ADC1) and map dry..wet to 0..100 %",
        stub: Stub::Reading { example: "40.0" },
        matter: None,
    },
    Kind {
        id: "number",
        group: Group::Measures,
        label: "Another reading (a number)",
        name: "Reading",
        ty: "Sensor",
        new_args: "",
        builder: ".measurement()",
        analog: false,
        wiring: "your sensor",
        todo: "read the value (add `.unit(\"...\")` and a `.device_class(\"...\")` to the entity)",
        stub: Stub::Reading { example: "1.0" },
        matter: None,
    },
    // Detects
    Kind {
        id: "motion",
        group: Group::Detects,
        label: "Motion (PIR sensor)",
        name: "Motion",
        ty: "BinarySensor",
        new_args: "",
        builder: r#".device_class("motion")"#,
        analog: false,
        wiring: "the PIR sensor's OUT pin (HC-SR501, AM312)",
        todo: "detect motion: `Input::new(_pin, InputConfig::default())`, high while the PIR sees motion",
        stub: Stub::Detect,
        matter: Some("Occupancy sensor"),
    },
    Kind {
        id: "contact",
        group: Group::Detects,
        label: "Door or window (reed switch)",
        name: "Door",
        ty: "BinarySensor",
        new_args: "",
        builder: r#".device_class("door")"#,
        analog: false,
        wiring: "one side of the reed switch (the other to GND)",
        todo: "read the reed switch: `Input::new(_pin, InputConfig::default().with_pull(Pull::Up))`, low while closed (`true` = open)",
        stub: Stub::Detect,
        matter: Some("Contact sensor"),
    },
    Kind {
        id: "leak",
        group: Group::Detects,
        label: "Water leak",
        name: "Leak",
        ty: "BinarySensor",
        new_args: "",
        builder: r#".device_class("moisture")"#,
        analog: false,
        wiring: "one probe of the leak sensor (the other to GND)",
        todo: "detect water: `Input::new(_pin, InputConfig::default().with_pull(Pull::Up))`, low when the probes are wet",
        stub: Stub::Detect,
        matter: Some("Water leak detector"),
    },
    Kind {
        id: "smoke",
        group: Group::Detects,
        label: "Smoke alarm",
        name: "Smoke",
        ty: "BinarySensor",
        new_args: "",
        builder: r#".device_class("smoke")"#,
        analog: false,
        wiring: "the alarm's interconnect / relay output",
        todo: "read the smoke alarm's output: `Input::new(_pin, InputConfig::default())`",
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
        new_args: "",
        builder: "",
        analog: false,
        wiring: "the relay module's IN pin",
        todo: "drive the relay: `Output::new(_pin, Level::Low, OutputConfig::default())` once, then `set_level(on.into())`",
        stub: Stub::Switch,
        matter: Some("On/off plug"),
    },
    Kind {
        id: "light",
        group: Group::Controls,
        label: "Light (dimmable / colour LED strip)",
        name: "Light",
        ty: "Light",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "DIN of a WS2812 strip, or the gate of a MOSFET for a single-colour strip",
        todo: "drive the light: a WS2812 strip with esp-hal's `Rmt` (crate `esp-hal-smartled`), or a dimmer with `Ledc` PWM",
        stub: Stub::Light,
        matter: Some("Extended colour light"),
    },
    Kind {
        id: "cover",
        group: Group::Controls,
        label: "Blind / curtain / garage door",
        name: "Blind",
        ty: "Cover",
        new_args: "",
        builder: r#".device_class("blind").position()"#,
        analog: false,
        wiring: "the motor driver's direction / enable input",
        todo: "move the motor to `position` (0 closed, 100 open), e.g. timed runs of a motor driver",
        stub: Stub::Cover,
        matter: Some("Window covering"),
    },
    Kind {
        id: "valve",
        group: Group::Controls,
        label: "Water valve",
        name: "Valve",
        ty: "Valve",
        new_args: "",
        builder: r#".device_class("water")"#,
        analog: false,
        wiring: "the valve's relay or driver input",
        todo: "open or close the valve (`position`: 0 closed, 100 open), e.g. a relay on a solenoid valve",
        stub: Stub::Cover,
        matter: Some("Water valve"),
    },
    Kind {
        id: "lock",
        group: Group::Controls,
        label: "Lock (door strike, smart lock)",
        name: "Lock",
        ty: "Lock",
        new_args: "",
        builder: "",
        analog: false,
        wiring: "the strike's relay or driver input",
        todo: "lock or unlock: e.g. drive the door strike's relay",
        stub: Stub::Lock,
        matter: Some("Door lock"),
    },
    Kind {
        id: "fan",
        group: Group::Controls,
        label: "Fan",
        name: "Fan",
        ty: "Fan",
        new_args: "",
        builder: ".speed()",
        analog: false,
        wiring: "the fan's PWM input or its MOSFET gate",
        todo: "run the fan at `wanted` (on, speed in %): PWM with esp-hal's `Ledc`",
        stub: Stub::Fan,
        matter: Some("Fan"),
    },
    Kind {
        id: "thermostat",
        group: Group::Controls,
        label: "Thermostat (heater control)",
        name: "Thermostat",
        ty: "Climate",
        new_args: ", &[HvacMode::Heat, HvacMode::Off]",
        builder: "",
        analog: false,
        wiring: "the heater's relay input",
        todo: "read the room temperature into `state.current`, then switch the heater towards `state.target` (set `state.action`)",
        stub: Stub::Climate,
        matter: Some("Thermostat"),
    },
    Kind {
        id: "setting",
        group: Group::Controls,
        label: "A setting (a number: threshold, duration…)",
        name: "Setting",
        ty: "Number",
        new_args: ", 0.0, 100.0, 1.0",
        builder: "",
        analog: false,
        wiring: "(none: a value the device uses)",
        todo: "use the new value (and keep it across restarts with `App::save`, if needed)",
        stub: Stub::Number,
        matter: None,
    },
    Kind {
        id: "mode",
        group: Group::Controls,
        label: "A mode (a choice: auto / on / off…)",
        name: "Mode",
        ty: "Select",
        new_args: r#", &["auto", "on", "off"]"#,
        builder: "",
        analog: false,
        wiring: "(none: a choice the device uses)",
        todo: "apply the chosen mode (`index` into the options)",
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
        new_args: r#", &["press", "double_press", "long_press"]"#,
        builder: r#".device_class("button")"#,
        analog: false,
        wiring: "one side of the button (the other to GND)",
        todo: "detect presses: `Input::new(_pin, InputConfig::default().with_pull(Pull::Up))`, `wait_for_falling_edge().await`, then time them; `trigger(0)` press, `trigger(1)` double, `trigger(2)` long",
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
        matches!(self.stub, Stub::Reading { .. } | Stub::Detect)
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
}
