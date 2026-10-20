//! Typed Home Assistant device classes and units: a typo is a build error, not an entity that
//! silently loses its icon, statistics or Matter type. `custom` is the escape hatch.

macro_rules! names {
    ($(#[$doc:meta])* $ty:ident { $($(#[$cdoc:meta])* $name:ident = $value:literal,)* }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $ty(&'static str);

        impl $ty {
            $($(#[$cdoc])* pub const $name: Self = Self($value);)*

            /// One not listed here, as Home Assistant spells it.
            pub const fn custom(name: &'static str) -> Self {
                Self(name)
            }

            /// As Home Assistant spells it.
            pub const fn as_str(self) -> &'static str {
                self.0
            }
        }
    };
}

names! {
    /// A Home Assistant device class: what the entity measures or is. One type for every entity
    /// kind; Home Assistant ignores a class its platform does not know.
    DeviceClass {
        // Sensors.
        /// Battery charge, `%`.
        BATTERY = "battery",
        /// CO2, `ppm`.
        CARBON_DIOXIDE = "carbon_dioxide",
        /// CO, `ppm` (also a binary sensor: detected).
        CARBON_MONOXIDE = "carbon_monoxide",
        /// Electric current, `A`.
        CURRENT = "current",
        /// Distance, `m`, `cm`, `mm`.
        DISTANCE = "distance",
        /// Duration, `s`, `min`, `h`.
        DURATION = "duration",
        /// Energy, `Wh`, `kWh`.
        ENERGY = "energy",
        /// Frequency, `Hz`.
        FREQUENCY = "frequency",
        /// Relative humidity, `%`.
        HUMIDITY = "humidity",
        /// Light level, `lx`.
        ILLUMINANCE = "illuminance",
        /// NO2, `µg/m³`.
        NITROGEN_DIOXIDE = "nitrogen_dioxide",
        /// Ozone, `µg/m³`.
        OZONE = "ozone",
        /// PM1, `µg/m³`.
        PM1 = "pm1",
        /// PM2.5, `µg/m³`.
        PM25 = "pm25",
        /// PM10, `µg/m³`.
        PM10 = "pm10",
        /// Power, `W`, `kW` (also a binary sensor: power detected).
        POWER = "power",
        /// Pressure, `hPa` and others.
        PRESSURE = "pressure",
        /// Wi-Fi signal, `dBm`.
        SIGNAL_STRENGTH = "signal_strength",
        /// Temperature, `°C`, `°F`, `K`.
        TEMPERATURE = "temperature",
        /// VOC, `µg/m³`.
        VOLATILE_ORGANIC_COMPOUNDS = "volatile_organic_compounds",
        /// VOC, `ppm`, `ppb`.
        VOLATILE_ORGANIC_COMPOUNDS_PARTS = "volatile_organic_compounds_parts",
        /// Voltage, `V`, `mV`.
        VOLTAGE = "voltage",
        // Binary sensors.
        /// Network connected.
        CONNECTIVITY = "connectivity",
        /// Door open (also a cover).
        DOOR = "door",
        /// Gas detected (also a valve).
        GAS = "gas",
        /// Wet (also a sensor: soil moisture, `%`).
        MOISTURE = "moisture",
        /// Motion detected (also an event).
        MOTION = "motion",
        /// Room occupied.
        OCCUPANCY = "occupancy",
        /// Open or closed.
        OPENING = "opening",
        /// Someone home.
        PRESENCE = "presence",
        /// Something wrong.
        PROBLEM = "problem",
        /// Smoke detected.
        SMOKE = "smoke",
        /// Vibration detected.
        VIBRATION = "vibration",
        /// Window open (also a cover).
        WINDOW = "window",
        // Covers.
        /// Awning.
        AWNING = "awning",
        /// Blind.
        BLIND = "blind",
        /// Curtain.
        CURTAIN = "curtain",
        /// Garage door.
        GARAGE = "garage",
        /// Gate.
        GATE = "gate",
        /// Shade.
        SHADE = "shade",
        /// Shutter.
        SHUTTER = "shutter",
        // Others.
        /// A button that identifies the device.
        IDENTIFY = "identify",
        /// A button that restarts the device.
        RESTART = "restart",
        /// A switch that is a power outlet.
        OUTLET = "outlet",
        /// A water valve (also a sensor: volume).
        WATER = "water",
        /// An event from a push button.
        BUTTON = "button",
        /// An event from a doorbell.
        DOORBELL = "doorbell",
        /// An update entity for the firmware.
        FIRMWARE = "firmware",
    }
}

names! {
    /// The unit of a value, as Home Assistant spells it.
    Unit {
        /// Degrees Celsius.
        CELSIUS = "°C",
        /// Degrees Fahrenheit.
        FAHRENHEIT = "°F",
        /// Kelvin.
        KELVIN = "K",
        /// Percent.
        PERCENT = "%",
        /// Lux.
        LUX = "lx",
        /// Hectopascal.
        HECTOPASCAL = "hPa",
        /// Millibar.
        MILLIBAR = "mbar",
        /// Pascal.
        PASCAL = "Pa",
        /// Kilopascal.
        KILOPASCAL = "kPa",
        /// Bar.
        BAR = "bar",
        /// Pounds per square inch.
        PSI = "psi",
        /// Inches of mercury.
        INHG = "inHg",
        /// Millimetres of mercury.
        MMHG = "mmHg",
        /// Parts per million.
        PPM = "ppm",
        /// Parts per billion.
        PPB = "ppb",
        /// Micrograms per cubic metre.
        MICROGRAMS_PER_CUBIC_METER = "µg/m³",
        /// Grams per cubic metre.
        GRAMS_PER_CUBIC_METER = "g/m³",
        /// Volt.
        VOLT = "V",
        /// Millivolt.
        MILLIVOLT = "mV",
        /// Ampere.
        AMPERE = "A",
        /// Watt.
        WATT = "W",
        /// Kilowatt.
        KILOWATT = "kW",
        /// Watt-hour.
        WATT_HOUR = "Wh",
        /// Kilowatt-hour.
        KILOWATT_HOUR = "kWh",
        /// Hertz.
        HERTZ = "Hz",
        /// Second.
        SECOND = "s",
        /// Minute.
        MINUTE = "min",
        /// Hour.
        HOUR = "h",
        /// Decibel-milliwatts (signal strength).
        DBM = "dBm",
        /// Metre.
        METER = "m",
        /// Centimetre.
        CENTIMETER = "cm",
        /// Millimetre.
        MILLIMETER = "mm",
        /// Litre.
        LITER = "L",
        /// Cubic metre.
        CUBIC_METER = "m³",
        /// Revolutions per minute.
        RPM = "rpm",
    }
}
