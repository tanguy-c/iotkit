//! Value and unit conversions between the model and Matter's attributes (host-tested).
//! Ported from esp-ha's `esp_ha_core::matter`.

use iotkit_model::{CoverState, FanCommand, FanDirection, FanState, HvacAction, HvacMode, LockCommand, LockState};

use crate::{MatterKind, Pollutant};

/// `v` rounded to the nearest integer (no `round` without std).
fn round(v: f32) -> i32 {
    if v >= 0.0 { (v + 0.5) as i32 } else { (v - 0.5) as i32 }
}

/// `log10` without std, good to 1e-4 over the range of illuminance.
fn log10(v: f32) -> f32 {
    // v = m · 2^e with m in [1, 2): log10(v) = (e + log2(m)) · log10(2).
    let bits = v.to_bits();
    let e = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    // log2(m) on [1, 2) by the series of ln((1+t)/(1-t)), t = (m-1)/(m+1).
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln_m = 2.0 * t * (1.0 + t2 / 3.0 + t2 * t2 / 5.0 + t2 * t2 * t2 / 7.0 + t2 * t2 * t2 * t2 / 9.0);
    (e as f32 * core::f32::consts::LN_2 + ln_m) * core::f32::consts::LOG10_E
}

/// A reading in `unit` in the unit Matter is based on for `kind` (°C, %, lx, hPa, ppm or
/// µg/m³); `None` if the unit is not one it takes.
pub fn to_matter_unit(kind: MatterKind, unit: &str, value: f32) -> Option<f32> {
    Some(match (kind, unit) {
        (MatterKind::TemperatureSensor, "°C") => value,
        (MatterKind::TemperatureSensor, "°F") => (value - 32.0) * 5.0 / 9.0,
        (MatterKind::TemperatureSensor, "K") => value - 273.15,
        (MatterKind::HumiditySensor | MatterKind::Battery, "%") => value,
        (MatterKind::LightSensor, "lx") => value,
        (MatterKind::PressureSensor, "hPa" | "mbar") => value,
        (MatterKind::PressureSensor, "Pa") => value / 100.0,
        (MatterKind::PressureSensor, "kPa") => value * 10.0,
        (MatterKind::PressureSensor, "bar") => value * 1000.0,
        (MatterKind::PressureSensor, "psi") => value * 68.947_57,
        (MatterKind::PressureSensor, "inHg") => value * 33.863_89,
        (MatterKind::PressureSensor, "mmHg") => value * 1.333_224,
        (MatterKind::AirQualitySensor(p), unit) => return concentration(p, unit, value).map(|(v, _)| v),
        _ => return None,
    })
}

/// A reading as Matter's `MeasuredValue` (or `BatPercentRemaining`); `None` when unknown or in
/// a unit Matter does not take.
pub fn measured_value(kind: MatterKind, unit: &str, value: f32) -> Option<i32> {
    let value = to_matter_unit(kind, unit, value)?;
    if !value.is_finite() {
        return None;
    }
    Some(match kind {
        // 0.01 °C
        MatterKind::TemperatureSensor => round(value * 100.0).clamp(-27315, 32767),
        // 0.01 %
        MatterKind::HumiditySensor => round(value * 100.0).clamp(0, 10000),
        // 10000 · log10(lux) + 1; 0 for too dark to measure.
        MatterKind::LightSensor if value < 1.0 => 0,
        MatterKind::LightSensor => round(10000.0 * log10(value) + 1.0).clamp(1, 0xfffe),
        // 0.1 kPa, which is 1 hPa.
        MatterKind::PressureSensor => round(value).clamp(-32767, 32767),
        // 0.5 %
        MatterKind::Battery => round(value * 2.0).clamp(0, 200),
        _ => return None,
    })
}

/// The Air Quality Sensor's pollutant of a sensor device class.
pub fn pollutant(device_class: &str) -> Option<Pollutant> {
    Some(match device_class {
        "carbon_dioxide" => Pollutant::Co2,
        "carbon_monoxide" => Pollutant::Co,
        "nitrogen_dioxide" => Pollutant::No2,
        "ozone" => Pollutant::O3,
        "pm1" => Pollutant::Pm1,
        "pm25" => Pollutant::Pm25,
        "pm10" => Pollutant::Pm10,
        "volatile_organic_compounds" | "volatile_organic_compounds_parts" => Pollutant::Tvoc,
        _ => return None,
    })
}

/// Concentration Measurement's `MeasurementUnit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConcentrationUnit {
    /// Parts per million.
    Ppm,
    /// Micrograms per cubic metre.
    Ugm3,
}

/// Litres per mole of air at 25 °C and 1 atm.
const MOLAR_VOLUME: f32 = 24.45;

fn molar_mass(p: Pollutant) -> Option<f32> {
    match p {
        Pollutant::Co2 => Some(44.01),
        Pollutant::Co => Some(28.01),
        Pollutant::No2 => Some(46.006),
        Pollutant::O3 => Some(48.0),
        _ => None,
    }
}

/// A reading as sent over Matter: gases in ppm (as Home Assistant expects), particles in
/// µg/m³; a VOC mixture in µg/m³ stays so. `None` for a unit that does not convert.
pub fn concentration(p: Pollutant, unit: &str, value: f32) -> Option<(f32, ConcentrationUnit)> {
    use ConcentrationUnit::*;
    Some(match (p, unit) {
        (Pollutant::Pm1 | Pollutant::Pm25 | Pollutant::Pm10, "µg/m³") => (value, Ugm3),
        (Pollutant::Pm1 | Pollutant::Pm25 | Pollutant::Pm10, _) => return None,
        (_, "ppm") => (value, Ppm),
        (_, "ppb") => (value / 1000.0, Ppm),
        (_, "µg/m³") => match molar_mass(p) {
            Some(mass) => (value * MOLAR_VOLUME / mass / 1000.0, Ppm),
            None => (value, Ugm3),
        },
        _ => return None,
    })
}

/// Air Quality's `AirQuality` (Good 1 .. Extremely poor 6, Unknown 0) for a reading in the
/// unit of [`concentration`]: the European AQI bands for PM, NO₂ and O₃, the US ones for CO,
/// and CO₂ by ppm. VOCs are unknown.
pub fn air_quality(p: Pollutant, value: f32) -> u8 {
    let (bands, value) = match p {
        Pollutant::Co2 => ([800.0, 1000.0, 1500.0, 2000.0, 5000.0], value),
        Pollutant::Tvoc => return 0,
        Pollutant::Pm1 | Pollutant::Pm25 => ([10.0, 20.0, 25.0, 50.0, 75.0], value),
        Pollutant::Pm10 => ([20.0, 40.0, 50.0, 100.0, 150.0], value),
        Pollutant::Co => ([4.5, 9.5, 12.5, 15.5, 30.5], value),
        // The EAQI bands are in µg/m³.
        Pollutant::No2 => ([40.0, 90.0, 120.0, 230.0, 340.0], value * 46.006 * 1000.0 / MOLAR_VOLUME),
        Pollutant::O3 => ([50.0, 100.0, 130.0, 240.0, 380.0], value * 48.0 * 1000.0 / MOLAR_VOLUME),
    };
    if !value.is_finite() {
        return 0;
    }
    bands.iter().position(|&max| value < max).map_or(6, |i| i as u8 + 1)
}

/// Power Source's `BatChargeLevel`: OK (0) from 20 %, Warning (1) from 10 %, Critical (2).
pub fn battery_charge_level(percent: f32) -> u8 {
    match percent {
        ..10.0 => 2,
        ..20.0 => 1,
        _ => 0,
    }
}

/// Boolean State's `StateValue` of a binary sensor that is `on`: a contact sensor's `true` is
/// closed (`on` is open in the model); a leak detector's `true` is a leak.
pub fn state_value(kind: MatterKind, on: bool) -> bool {
    if kind == MatterKind::ContactSensor { !on } else { on }
}

/// What a button event type means for a Matter Generic Switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// One press.
    Press,
    /// A long press.
    LongPress,
    /// `n` presses in a row (2..=8).
    MultiPress(u8),
}

/// The gesture of an event type: `press`, `long_press`, `double_press`, `triple_press`,
/// `multi_press_<n>` and a few synonyms; `None` without a switch equivalent.
pub fn gesture(event_type: &str) -> Option<Gesture> {
    Some(match event_type {
        "press" | "short_press" | "single_press" | "single" | "click" => Gesture::Press,
        "long_press" | "long" | "hold" => Gesture::LongPress,
        "double_press" | "double" | "double_click" => Gesture::MultiPress(2),
        "triple_press" | "triple" => Gesture::MultiPress(3),
        "quadruple_press" => Gesture::MultiPress(4),
        _ => match event_type.strip_prefix("multi_press_").and_then(|n| n.parse::<u8>().ok()) {
            Some(n @ 2..=8) => Gesture::MultiPress(n),
            _ => return None,
        },
    })
}

/// The Switch cluster's features for a button's event types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchFeatures {
    /// Has a long press.
    pub long_press: bool,
    /// Below 2: no multi-press.
    pub multi_press_max: u8,
}

impl SwitchFeatures {
    /// The features the event types need.
    pub fn of(types: &[&str]) -> Self {
        let mut f = Self { long_press: false, multi_press_max: 1 };
        for g in types.iter().filter_map(|t| gesture(t)) {
            match g {
                Gesture::Press => {}
                Gesture::LongPress => f.long_press = true,
                Gesture::MultiPress(n) => f.multi_press_max = f.multi_press_max.max(n),
            }
        }
        f
    }

    /// Has a multi-press.
    pub fn multi_press(self) -> bool {
        self.multi_press_max >= 2
    }

    /// `FeatureMap`: momentary (MS) with release (MSR), long press (MSL), multi-press (MSM).
    pub fn feature_map(self) -> u32 {
        const MS: u32 = 0x02;
        const MSR: u32 = 0x04;
        const MSL: u32 = 0x08;
        const MSM: u32 = 0x10;
        MS | MSR | if self.long_press { MSL } else { 0 } | if self.multi_press() { MSM } else { 0 }
    }
}

/// A Switch cluster event (position 1 pressed, 0 released).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchEvent {
    /// Pressed.
    InitialPress,
    /// Still pressed: a long press.
    LongPress,
    /// Released after a short press.
    ShortRelease,
    /// Released after a long press.
    LongRelease,
    /// The `n`th press of a series.
    MultiPressOngoing(u8),
    /// A series of `n` presses ended.
    MultiPressComplete(u8),
}

/// The events a real button would send for `gesture`, in order (at most 3 + 3 · 7 + 1).
pub fn switch_events(gesture: Gesture, features: SwitchFeatures) -> heapless::Vec<SwitchEvent, 25> {
    use SwitchEvent::*;
    let mut events = heapless::Vec::new();
    let presses = match gesture {
        Gesture::LongPress => {
            let _ = events.extend_from_slice(&[InitialPress, LongPress, LongRelease]);
            return events;
        }
        Gesture::Press => 1,
        Gesture::MultiPress(n) => n.min(features.multi_press_max),
    };
    let _ = events.extend_from_slice(&[InitialPress, ShortRelease]);
    if features.multi_press() {
        for n in 2..=presses {
            let _ = events.extend_from_slice(&[InitialPress, MultiPressOngoing(n), ShortRelease]);
        }
        let _ = events.push(MultiPressComplete(presses));
    }
    events
}

/// Window Covering's lift position (0 open, 10000 closed) from a position (0 closed, 100 open).
pub fn cover_percent100ths(position: u8) -> u16 {
    (100 - u16::from(position.min(100))) * 100
}

/// The position (0 closed, 100 open) from Window Covering's lift position (0 open, 10000 closed).
pub fn cover_position(percent100ths: u16) -> u8 {
    (100 - (percent100ths.min(10000) + 50) / 100) as u8
}

/// The position, or one implied by the state (open 100, closed 0).
pub fn cover_position_of(state: Option<CoverState>, position: Option<u8>) -> Option<u8> {
    position.or(match state {
        Some(CoverState::Open) => Some(100),
        Some(CoverState::Closed) => Some(0),
        _ => None,
    })
}

/// `OperationalStatus`: global and lift bits, 1 opening, 2 closing.
pub fn cover_operational_status(state: Option<CoverState>) -> u8 {
    let movement = match state {
        Some(CoverState::Opening) => 1,
        Some(CoverState::Closing) => 2,
        _ => 0,
    };
    movement | movement << 2
}

/// Window Covering `Type` and `EndProductType` of a device class.
pub fn cover_types(class: Option<&str>) -> (u8, u8) {
    match class {
        Some("awning") => (5, 19),
        Some("curtain") => (4, 16),
        Some("blind") => (8, 10),
        Some("shade") => (0, 0),
        _ => (6, 17),
    }
}

/// A valve's `CurrentState`: 0 closed, 1 open, 2 moving.
pub fn valve_state(state: Option<CoverState>, position: Option<u8>) -> Option<u8> {
    Some(match state {
        Some(CoverState::Closed) => 0,
        Some(CoverState::Open | CoverState::Stopped) => 1,
        Some(CoverState::Opening | CoverState::Closing) => 2,
        None => u8::from(position? > 0),
    })
}

/// A valve's `TargetState` while moving: 0 closed, 1 open.
pub fn valve_target(state: Option<CoverState>) -> Option<u8> {
    match state {
        Some(CoverState::Opening) => Some(1),
        Some(CoverState::Closing) => Some(0),
        _ => None,
    }
}

/// Door Lock's `LockState`: 1 locked, 2 unlocked, 3 unlatched, 0 not fully locked.
pub fn lock_state(state: LockState, unbolt: bool) -> u8 {
    match state {
        LockState::Locked => 1,
        LockState::Unlocked => 2,
        LockState::Open | LockState::Opening if unbolt => 3,
        LockState::Open | LockState::Opening => 2,
        _ => 0,
    }
}

/// UnlockDoor also unlatches a lock that can.
pub fn unlock_door_command(unbolt: bool) -> LockCommand {
    if unbolt { LockCommand::Open } else { LockCommand::Unlock }
}

/// Fan Control's `FanMode` values.
pub mod fan_mode {
    /// Off.
    pub const OFF: u8 = 0;
    /// Low.
    pub const LOW: u8 = 1;
    /// Medium.
    pub const MEDIUM: u8 = 2;
    /// High.
    pub const HIGH: u8 = 3;
    /// On.
    pub const ON: u8 = 4;
    /// Auto.
    pub const AUTO: u8 = 5;
    /// Smart (taken as Auto).
    pub const SMART: u8 = 6;
}

/// A fan's options, from its spec.
#[derive(Clone, Copy, Debug)]
pub struct FanSpec {
    /// Takes a speed (percentage).
    pub speed: bool,
    /// Preset modes.
    pub presets: &'static [&'static str],
    /// Can oscillate.
    pub oscillation: bool,
    /// Can change direction.
    pub direction: bool,
}

impl FanSpec {
    /// The index of an `auto` preset: Matter's Auto mode.
    pub fn auto(&self) -> Option<u8> {
        self.presets.iter().position(|p| *p == "auto").map(|i| i as u8)
    }

    /// `FeatureMap`: Auto, Rocking, Airflow direction.
    pub fn features(&self) -> u32 {
        const AUTO: u32 = 0x02;
        const ROCKING: u32 = 0x04;
        const AIRFLOW_DIRECTION: u32 = 0x20;
        (if self.auto().is_some() { AUTO } else { 0 })
            | if self.oscillation { ROCKING } else { 0 }
            | if self.direction { AIRFLOW_DIRECTION } else { 0 }
    }

    /// `FanModeSequence`: Off/Low/Med/High (0), with Auto (2).
    pub fn mode_sequence(&self) -> u8 {
        if self.auto().is_some() { 2 } else { 0 }
    }

    /// `FanMode` of a state: the speed in thirds, or Auto with the `auto` preset.
    pub fn mode_of(&self, state: &FanState) -> u8 {
        match state {
            FanState { on: false, .. } => fan_mode::OFF,
            FanState { preset: Some(p), .. } if Some(*p) == self.auto() => fan_mode::AUTO,
            _ if !self.speed => fan_mode::HIGH,
            FanState { percentage: ..=33, .. } => fan_mode::LOW,
            FanState { percentage: ..=66, .. } => fan_mode::MEDIUM,
            _ => fan_mode::HIGH,
        }
    }

    /// `PercentCurrent` of a state (100 without a speed).
    pub fn percent(&self, state: &FanState) -> u8 {
        match state {
            FanState { on: false, .. } => 0,
            _ if !self.speed => 100,
            _ => state.percentage.min(100),
        }
    }

    /// The command for a `FanMode` write; `None` for a mode it does not take.
    pub fn command_for_mode(&self, mode: u8) -> Option<FanCommand> {
        let speed = |p| if self.speed { FanCommand::Percentage(p) } else { FanCommand::On(true) };
        Some(match mode {
            fan_mode::OFF => FanCommand::On(false),
            fan_mode::LOW => speed(33),
            fan_mode::MEDIUM => speed(66),
            fan_mode::HIGH => speed(100),
            fan_mode::ON => FanCommand::On(true),
            fan_mode::AUTO | fan_mode::SMART => FanCommand::Preset(self.auto()?),
            _ => return None,
        })
    }

    /// The command for a `PercentSetting` write (0 turns it off).
    pub fn command_for_percent(&self, percent: u8) -> FanCommand {
        match percent {
            0 => FanCommand::On(false),
            p if self.speed => FanCommand::Percentage(p.min(100)),
            _ => FanCommand::On(true),
        }
    }
}

/// `AirflowDirection`: 0 forward, 1 reverse.
pub fn airflow_direction(d: FanDirection) -> u8 {
    match d {
        FanDirection::Forward => 0,
        FanDirection::Reverse => 1,
    }
}

/// Thermostat's `SystemMode` values.
pub mod system_mode {
    /// Off.
    pub const OFF: u8 = 0;
    /// Auto.
    pub const AUTO: u8 = 1;
    /// Cool.
    pub const COOL: u8 = 3;
    /// Heat.
    pub const HEAT: u8 = 4;
    /// Emergency heat.
    pub const EMERGENCY_HEAT: u8 = 5;
    /// Precooling.
    pub const PRECOOLING: u8 = 6;
    /// Fan only.
    pub const FAN_ONLY: u8 = 7;
    /// Dry.
    pub const DRY: u8 = 8;
}

/// `SystemMode` of a mode.
pub fn system_mode_of(mode: HvacMode) -> u8 {
    match mode {
        HvacMode::Off => system_mode::OFF,
        HvacMode::Heat => system_mode::HEAT,
        HvacMode::Cool => system_mode::COOL,
        HvacMode::HeatCool | HvacMode::Auto => system_mode::AUTO,
        HvacMode::Dry => system_mode::DRY,
        HvacMode::FanOnly => system_mode::FAN_ONLY,
    }
}

/// The thermostat's mode for a `SystemMode`, if it has it.
pub fn hvac_mode_for(system_mode: u8, modes: &[HvacMode]) -> Option<HvacMode> {
    let mode = match system_mode {
        system_mode::OFF => HvacMode::Off,
        system_mode::AUTO if modes.contains(&HvacMode::HeatCool) => HvacMode::HeatCool,
        system_mode::AUTO => HvacMode::Auto,
        system_mode::COOL | system_mode::PRECOOLING => HvacMode::Cool,
        system_mode::HEAT | system_mode::EMERGENCY_HEAT => HvacMode::Heat,
        system_mode::FAN_ONLY => HvacMode::FanOnly,
        system_mode::DRY => HvacMode::Dry,
        _ => return None,
    };
    modes.contains(&mode).then_some(mode)
}

fn heats(modes: &[HvacMode]) -> bool {
    modes.iter().any(|m| matches!(m, HvacMode::Heat | HvacMode::HeatCool | HvacMode::Auto))
}

fn cools(modes: &[HvacMode]) -> bool {
    modes.iter().any(|m| matches!(m, HvacMode::Cool | HvacMode::HeatCool | HvacMode::Auto))
}

/// `FeatureMap`: Heating, Cooling, AutoMode.
pub fn thermostat_features(modes: &[HvacMode]) -> u32 {
    const HEATING: u32 = 0x01;
    const COOLING: u32 = 0x02;
    const AUTO_MODE: u32 = 0x20;
    let (heat, cool) = (heats(modes), cools(modes));
    let auto = modes.iter().any(|m| matches!(m, HvacMode::HeatCool | HvacMode::Auto));
    (if heat || !cool { HEATING } else { 0 }) | if cool { COOLING } else { 0 } | if auto { AUTO_MODE } else { 0 }
}

/// `ControlSequenceOfOperation`: 0 cooling only, 2 heating only, 4 both.
pub fn control_sequence(modes: &[HvacMode]) -> u8 {
    match (heats(modes), cools(modes)) {
        (true, true) => 4,
        (false, true) => 0,
        _ => 2,
    }
}

/// `ThermostatRunningState`: heat, cool, fan.
pub fn running_state(action: Option<HvacAction>) -> u16 {
    match action {
        Some(HvacAction::Heating) => 0x01,
        Some(HvacAction::Cooling) => 0x02,
        Some(HvacAction::Fan) => 0x04,
        _ => 0,
    }
}

/// A temperature in 0.01 °C.
pub fn centi_celsius(celsius: f32) -> i16 {
    round(celsius * 100.0).clamp(-27315, 32767) as i16
}

/// A setpoint (0.01 °C) as a target on the thermostat's steps; `None` out of its range (half a
/// step of slack: the limits are reported rounded to 0.01 °C).
pub fn climate_target(centi: i16, min: f32, max: f32, step: f32) -> Option<f32> {
    let value = round(f32::from(centi) / 100.0 / step) as f32 * step;
    let slack = step / 2.0;
    (value >= min - slack && value <= max + slack).then(|| value.clamp(min, max))
}

/// SetpointRaiseLower: `amount` in 0.1 °C.
pub fn raise_target(target: f32, amount: i8, min: f32, max: f32, step: f32) -> f32 {
    let value = round((target + f32::from(amount) / 10.0) / step) as f32 * step;
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurements() {
        assert_eq!(measured_value(MatterKind::TemperatureSensor, "°C", 21.46), Some(2146));
        assert_eq!(measured_value(MatterKind::HumiditySensor, "%", 45.5), Some(4550));
        assert_eq!(measured_value(MatterKind::PressureSensor, "hPa", 1013.2), Some(1013));
        assert_eq!(measured_value(MatterKind::PressureSensor, "kPa", 101.3), Some(1013));
        assert_eq!(measured_value(MatterKind::LightSensor, "lx", 0.5), Some(0));
        assert_eq!(measured_value(MatterKind::LightSensor, "lx", 1.0), Some(1));
        // 10000 · log10(1000) + 1
        let lux = measured_value(MatterKind::LightSensor, "lx", 1000.0).unwrap();
        assert!((30000..=30002).contains(&lux), "{lux}");
        assert_eq!(measured_value(MatterKind::Battery, "%", 87.0), Some(174));
        assert_eq!(measured_value(MatterKind::HumiditySensor, "°C", 1.0), None, "wrong unit");
        assert_eq!(measured_value(MatterKind::HumiditySensor, "%", f32::NAN), None);
    }

    #[test]
    fn air() {
        assert_eq!(pollutant("carbon_dioxide"), Some(Pollutant::Co2));
        assert_eq!(pollutant("temperature"), None);
        assert_eq!(concentration(Pollutant::Co2, "ppm", 650.0), Some((650.0, ConcentrationUnit::Ppm)));
        assert_eq!(concentration(Pollutant::Pm25, "µg/m³", 12.0), Some((12.0, ConcentrationUnit::Ugm3)));
        assert_eq!(concentration(Pollutant::Pm25, "ppm", 12.0), None);
        assert_eq!(air_quality(Pollutant::Co2, 650.0), 1);
        assert_eq!(air_quality(Pollutant::Co2, 1200.0), 3);
        assert_eq!(air_quality(Pollutant::Co2, 9000.0), 6);
        assert_eq!(air_quality(Pollutant::Pm25, 30.0), 4);
        assert_eq!(air_quality(Pollutant::Tvoc, 1.0), 0);
        assert_eq!(air_quality(Pollutant::Co2, f32::NAN), 0);
    }

    #[test]
    fn switches() {
        let f = SwitchFeatures::of(&["press", "double_press", "long_press", "spin"]);
        assert_eq!((f.long_press, f.multi_press_max), (true, 2));
        assert_eq!(f.feature_map(), 0x02 | 0x04 | 0x08 | 0x10);
        use SwitchEvent::*;
        assert_eq!(&switch_events(Gesture::Press, f)[..], &[InitialPress, ShortRelease, MultiPressComplete(1)]);
        assert_eq!(
            &switch_events(Gesture::MultiPress(2), f)[..],
            &[InitialPress, ShortRelease, InitialPress, MultiPressOngoing(2), ShortRelease, MultiPressComplete(2)]
        );
        let simple = SwitchFeatures::of(&["press"]);
        assert_eq!(&switch_events(Gesture::Press, simple)[..], &[InitialPress, ShortRelease]);
        assert_eq!(gesture("multi_press_5"), Some(Gesture::MultiPress(5)));
        assert_eq!(gesture("multi_press_9"), None);
    }

    #[test]
    fn covers_and_valves() {
        assert_eq!((cover_percent100ths(100), cover_percent100ths(0), cover_percent100ths(30)), (0, 10000, 7000));
        assert_eq!(cover_position(7000), 30);
        assert_eq!(cover_position_of(Some(CoverState::Open), None), Some(100));
        assert_eq!(cover_operational_status(Some(CoverState::Closing)), 0b1010);
        assert_eq!(valve_state(None, Some(40)), Some(1));
        assert_eq!(valve_state(Some(CoverState::Opening), None), Some(2));
        assert_eq!(lock_state(LockState::Open, true), 3);
        assert_eq!(lock_state(LockState::Open, false), 2);
    }

    #[test]
    fn fans() {
        let spec = FanSpec { speed: true, presets: &["sleep", "auto"], oscillation: true, direction: false };
        assert_eq!(spec.features(), 0x02 | 0x04);
        let on = FanState { on: true, percentage: 50, ..FanState::OFF };
        assert_eq!((spec.mode_of(&on), spec.percent(&on)), (fan_mode::MEDIUM, 50));
        assert_eq!(spec.mode_of(&FanState { preset: Some(1), ..on }), fan_mode::AUTO);
        assert_eq!(spec.command_for_mode(fan_mode::AUTO), Some(FanCommand::Preset(1)));
        assert_eq!(spec.command_for_percent(0), FanCommand::On(false));
        let plain = FanSpec { speed: false, presets: &[], oscillation: false, direction: false };
        assert_eq!(plain.command_for_mode(fan_mode::AUTO), None);
        assert_eq!(plain.percent(&on), 100);
    }

    #[test]
    fn thermostats() {
        let modes = [HvacMode::Heat, HvacMode::Cool, HvacMode::HeatCool, HvacMode::Off];
        assert_eq!(thermostat_features(&modes), 0x01 | 0x02 | 0x20);
        assert_eq!(control_sequence(&modes), 4);
        assert_eq!(control_sequence(&[HvacMode::Heat]), 2);
        assert_eq!(hvac_mode_for(system_mode::AUTO, &modes), Some(HvacMode::HeatCool));
        assert_eq!(hvac_mode_for(system_mode::DRY, &modes), None);
        assert_eq!(centi_celsius(21.5), 2150);
        assert_eq!(climate_target(2140, 7.0, 35.0, 0.5), Some(21.5));
        assert_eq!(climate_target(4000, 7.0, 35.0, 0.5), None);
        assert_eq!(raise_target(21.0, 15, 7.0, 35.0, 0.5), 22.5);
    }
}
