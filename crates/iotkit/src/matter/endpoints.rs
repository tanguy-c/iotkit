//! The endpoints: one per entity with a Matter equivalent, built from the device at startup.

use alloc::{boxed::Box, vec::Vec};

use crate::model::{ClimateState, Command, CoverState, Device, Entity, FanState, LightState, LockState, Spec, State};
use rs_matter_embassy::matter::{
    crypto::Rng,
    dm::{
        Cluster, Dataver, DeviceType, Endpoint, Node, Quality,
        clusters::{
            app::color_control as color,
            decl::{
                air_quality, boolean_state, carbon_dioxide_concentration_measurement as co2,
                carbon_monoxide_concentration_measurement as co, door_lock, fan_control,
                illuminance_measurement as lux, level_control, mode_select,
                nitrogen_dioxide_concentration_measurement as no2, occupancy_sensing as occupancy, on_off,
                ozone_concentration_measurement as o3, pm_1_concentration_measurement as pm1,
                pm_10_concentration_measurement as pm10, pm_25_concentration_measurement as pm25, power_source,
                pressure_measurement as pressure, relative_humidity_measurement as humidity, smoke_co_alarm, switch,
                temperature_measurement as temperature, thermostat,
                total_volatile_organic_compounds_concentration_measurement as tvoc,
                valve_configuration_and_control as valve, window_covering,
            },
            desc::{self, ClusterHandler as _},
            fixed_label::{self, FixedLabelEntry},
            identify,
        },
    },
    tlv::Nullable,
    with,
};

use crate::matter::{
    MatterKind, Pollutant, brightness_from_level,
    map::{self, FanSpec, SwitchFeatures},
    matter_kind,
};

pub(crate) const DESC: Cluster<'static> = desc::DescHandler::CLUSTER;
pub(crate) const TEMPERATURE: Cluster<'static> =
    temperature::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(with!());
pub(crate) const BOOLEAN: Cluster<'static> = boolean_state::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(with!());

/// On/Off without the Lighting feature (no scenes, timed on, start-up state).
pub(crate) const ON_OFF: Cluster<'static> = on_off::FULL_CLUSTER
    .with_revision(6)
    .with_features(0)
    .with_attrs(with!(required))
    .with_cmds(with!(on_off::CommandId::Off | on_off::CommandId::On | on_off::CommandId::Toggle));

pub(crate) const LEVEL: Cluster<'static> = level_control::FULL_CLUSTER
    .with_revision(6)
    .with_features(level_control::Feature::ON_OFF.bits())
    .with_attrs(with!(
        required;
        level_control::AttributeId::CurrentLevel
            | level_control::AttributeId::MinLevel
            | level_control::AttributeId::MaxLevel
            | level_control::AttributeId::OnLevel
            | level_control::AttributeId::Options
    ))
    .with_cmds(with!(
        level_control::CommandId::MoveToLevel
            | level_control::CommandId::Move
            | level_control::CommandId::Step
            | level_control::CommandId::Stop
            | level_control::CommandId::MoveToLevelWithOnOff
            | level_control::CommandId::MoveWithOnOff
            | level_control::CommandId::StepWithOnOff
            | level_control::CommandId::StopWithOnOff
    ));

/// Full colour: hue/saturation, xy and colour temperature, all converted to RGB.
/// Transitions and the colour loop are run by rs-matter's handler.
pub(crate) const COLOR: Cluster<'static> = color::FULL_CLUSTER
    .with_features(
        color::Feature::HUE_AND_SATURATION.bits()
            | color::Feature::ENHANCED_HUE.bits()
            | color::Feature::COLOR_LOOP.bits()
            | color::Feature::XY.bits()
            | color::Feature::COLOR_TEMPERATURE.bits(),
    )
    .with_attrs(with!(
        required;
        color::AttributeId::CurrentHue
            | color::AttributeId::CurrentSaturation
            | color::AttributeId::RemainingTime
            | color::AttributeId::CurrentX
            | color::AttributeId::CurrentY
            | color::AttributeId::ColorTemperatureMireds
            | color::AttributeId::ColorMode
            | color::AttributeId::Options
            | color::AttributeId::NumberOfPrimaries
            | color::AttributeId::EnhancedCurrentHue
            | color::AttributeId::EnhancedColorMode
            | color::AttributeId::ColorLoopActive
            | color::AttributeId::ColorLoopDirection
            | color::AttributeId::ColorLoopTime
            | color::AttributeId::ColorLoopStartEnhancedHue
            | color::AttributeId::ColorLoopStoredEnhancedHue
            | color::AttributeId::ColorCapabilities
            | color::AttributeId::ColorTempPhysicalMinMireds
            | color::AttributeId::ColorTempPhysicalMaxMireds
            | color::AttributeId::CoupleColorTempToLevelMinMireds
            | color::AttributeId::StartUpColorTemperatureMireds
    ))
    .with_cmds(with!(
        color::CommandId::MoveToHue
            | color::CommandId::MoveHue
            | color::CommandId::StepHue
            | color::CommandId::MoveToSaturation
            | color::CommandId::MoveSaturation
            | color::CommandId::StepSaturation
            | color::CommandId::MoveToHueAndSaturation
            | color::CommandId::MoveToColor
            | color::CommandId::MoveColor
            | color::CommandId::StepColor
            | color::CommandId::MoveToColorTemperature
            | color::CommandId::EnhancedMoveToHue
            | color::CommandId::EnhancedMoveHue
            | color::CommandId::EnhancedStepHue
            | color::CommandId::EnhancedMoveToHueAndSaturation
            | color::CommandId::ColorLoopSet
            | color::CommandId::StopMoveStep
            | color::CommandId::MoveColorTemperature
            | color::CommandId::StepColorTemperature
    ));

/// Carries the entity's name: Home Assistant names a test-vendor device's entities after their
/// `ha_entitylabel` label.
pub(crate) const FIXED_LABEL: Cluster<'static> = fixed_label::CLUSTER;
/// On every entity endpoint (but the battery's): a controller asks the device to show itself.
pub(crate) const IDENTIFY: Cluster<'static> = identify::CLUSTER;
pub(crate) const HUMIDITY: Cluster<'static> = humidity::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(with!());
pub(crate) const LUX: Cluster<'static> = lux::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(with!());
pub(crate) const PRESSURE: Cluster<'static> = pressure::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(with!());
/// Reported as a PIR sensor, the usual motion sensor.
pub(crate) const OCCUPANCY: Cluster<'static> = occupancy::FULL_CLUSTER
    .with_features(occupancy::Feature::PASSIVE_INFRARED.bits())
    .with_attrs(with!(required))
    .with_cmds(with!());
/// Every air quality level.
pub(crate) const AIR_QUALITY: Cluster<'static> = air_quality::FULL_CLUSTER
    .with_features(
        air_quality::Feature::FAIR.bits()
            | air_quality::Feature::MODERATE.bits()
            | air_quality::Feature::VERY_POOR.bits()
            | air_quality::Feature::EXTREMELY_POOR.bits(),
    )
    .with_attrs(with!(required))
    .with_cmds(with!());
/// A Concentration Measurement cluster with a numeric reading.
macro_rules! concentration {
    ($name:ident, $module:ident) => {
        pub(crate) const $name: Cluster<'static> = $module::FULL_CLUSTER
            .with_features($module::Feature::NUMERIC_MEASUREMENT.bits())
            .with_attrs(with!(
                required;
                $module::AttributeId::MeasuredValue
                    | $module::AttributeId::MinMeasuredValue
                    | $module::AttributeId::MaxMeasuredValue
                    | $module::AttributeId::MeasurementUnit
            ))
            .with_cmds(with!());
    };
}
concentration!(CO2, co2);
concentration!(CO, co);
concentration!(NO2, no2);
concentration!(O3, o3);
concentration!(PM1, pm1);
concentration!(PM25, pm25);
concentration!(PM10, pm10);
concentration!(TVOC, tvoc);
/// Smoke or CO alarm, from a binary sensor; the self-test does nothing.
pub(crate) const SMOKE_ALARM: Cluster<'static> = smoke_co_alarm::FULL_CLUSTER
    .with_features(smoke_co_alarm::Feature::SMOKE_ALARM.bits())
    .with_attrs(with!(required; smoke_co_alarm::AttributeId::SmokeState))
    .with_cmds(with!(smoke_co_alarm::CommandId::SelfTestRequest));
const CO_ALARM: Cluster<'static> = smoke_co_alarm::FULL_CLUSTER
    .with_features(smoke_co_alarm::Feature::CO_ALARM.bits())
    .with_attrs(with!(required; smoke_co_alarm::AttributeId::COState))
    .with_cmds(with!(smoke_co_alarm::CommandId::SelfTestRequest));
/// Lift only. Without a position, the position follows the open/closed state, and
/// GoToLiftPercentage is not accepted.
pub(crate) const COVER: Cluster<'static> = window_covering::FULL_CLUSTER
    .with_features(window_covering::Feature::LIFT.bits() | window_covering::Feature::POSITION_AWARE_LIFT.bits())
    .with_attrs(with!(
        required;
        window_covering::AttributeId::CurrentPositionLiftPercent100ths
            | window_covering::AttributeId::TargetPositionLiftPercent100ths
    ))
    .with_cmds(with!(
        window_covering::CommandId::UpOrOpen
            | window_covering::CommandId::DownOrClose
            | window_covering::CommandId::StopMotion
    ));
const COVER_POSITION: Cluster<'static> = COVER.with_cmds(with!(
    window_covering::CommandId::UpOrOpen
        | window_covering::CommandId::DownOrClose
        | window_covering::CommandId::StopMotion
        | window_covering::CommandId::GoToLiftPercentage
));
/// Open/close, plus a level with a position (feature LVL).
pub(crate) const VALVE: Cluster<'static> = valve::FULL_CLUSTER
    .with_attrs(|attr, _, features| {
        use valve::AttributeId as A;
        !attr.quality.contains(Quality::OPTIONAL)
            || (features & valve::Feature::LEVEL.bits() != 0
                && matches!(attr.id.try_into(), Ok(A::CurrentLevel | A::TargetLevel)))
    })
    .with_cmds(with!(valve::CommandId::Open | valve::CommandId::Close));
/// No PIN or users: locked and unlocked without a code; unbolting with `Lock::open`.
pub(crate) const DOOR_LOCK: Cluster<'static> =
    door_lock::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(|cmd, _, features| {
        use door_lock::CommandId as C;
        match cmd.id.try_into() {
            Ok(C::LockDoor | C::UnlockDoor) => true,
            Ok(C::UnboltDoor) => features & door_lock::Feature::UNBOLT.bits() != 0,
            _ => false,
        }
    });
/// Modes and percent, rocking and airflow direction by feature (see [`FanSpec::features`]).
pub(crate) const FAN: Cluster<'static> = fan_control::FULL_CLUSTER
    .with_attrs(|attr, _, features| {
        use fan_control::{AttributeId as A, Feature as F};
        !attr.quality.contains(Quality::OPTIONAL)
            || match attr.id.try_into() {
                Ok(A::RockSupport | A::RockSetting) => features & F::ROCKING.bits() != 0,
                Ok(A::AirflowDirection) => features & F::AIRFLOW_DIRECTION.bits() != 0,
                _ => false,
            }
    })
    .with_cmds(with!());
/// Heating and/or cooling setpoints by feature (see [`map::thermostat_features`]).
pub(crate) const THERMOSTAT: Cluster<'static> = thermostat::FULL_CLUSTER
    .with_attrs(|attr, _, features| {
        use thermostat::{AttributeId as A, Feature as F};
        let has = |feature: F| features & feature.bits() != 0;
        !attr.quality.contains(Quality::OPTIONAL)
            || match attr.id.try_into() {
                Ok(A::ThermostatRunningState) => true,
                Ok(A::OccupiedHeatingSetpoint | A::AbsMinHeatSetpointLimit | A::AbsMaxHeatSetpointLimit) => {
                    has(F::HEATING)
                }
                Ok(A::OccupiedCoolingSetpoint | A::AbsMinCoolSetpointLimit | A::AbsMaxCoolSetpointLimit) => {
                    has(F::COOLING)
                }
                Ok(A::MinSetpointDeadBand) => has(F::AUTO_MODE),
                _ => false,
            }
    })
    .with_cmds(with!(thermostat::CommandId::SetpointRaiseLower));
pub(crate) const MODE_SELECT: Cluster<'static> =
    mode_select::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(with!(mode_select::CommandId::ChangeToMode));
/// A battery: charge level and percentage.
pub(crate) const POWER_SOURCE: Cluster<'static> = power_source::FULL_CLUSTER
    .with_features(power_source::Feature::BATTERY.bits())
    .with_attrs(with!(
        required;
        power_source::AttributeId::BatPercentRemaining
            | power_source::AttributeId::BatChargeLevel
            | power_source::AttributeId::BatReplacementNeeded
            | power_source::AttributeId::BatReplaceability
    ))
    .with_cmds(with!());
/// Momentary switch; the features are set per endpoint (see [`SwitchFeatures`]).
pub(crate) const SWITCH: Cluster<'static> = switch::FULL_CLUSTER.with_attrs(with!(required)).with_cmds(with!());
const SWITCH_MULTI: Cluster<'static> =
    switch::FULL_CLUSTER.with_attrs(with!(required; switch::AttributeId::MultiPressMax)).with_cmds(with!());

/// The Concentration Measurement cluster of a pollutant.
pub(crate) const fn concentration_cluster(p: Pollutant) -> Cluster<'static> {
    match p {
        Pollutant::Co2 => CO2,
        Pollutant::Co => CO,
        Pollutant::No2 => NO2,
        Pollutant::O3 => O3,
        Pollutant::Pm1 => PM1,
        Pollutant::Pm25 => PM25,
        Pollutant::Pm10 => PM10,
        Pollutant::Tvoc => TVOC,
    }
}

const fn dt(dtype: u16, drev: u16) -> DeviceType {
    DeviceType { dtype, drev }
}

/// A `&'static` device type list (a `const fn` call is not promoted).
macro_rules! device_type {
    ($dtype:expr, $drev:expr) => {{
        const TYPES: &[DeviceType] = &[dt($dtype, $drev)];
        TYPES
    }};
}

type Layout = (&'static [DeviceType], &'static [Cluster<'static>]);

/// Device type and clusters of each kind of endpoint.
fn layout(kind: MatterKind, entity: &dyn Entity) -> Layout {
    const COLOR_LIGHT: Layout = (&[dt(0x010D, 4)], &[DESC, IDENTIFY, ON_OFF, LEVEL, COLOR, FIXED_LABEL]);
    const PLUG: Layout = (&[dt(0x010A, 3)], &[DESC, IDENTIFY, ON_OFF, FIXED_LABEL]);
    const TEMPERATURE_SENSOR: Layout = (&[dt(0x0302, 2)], &[DESC, IDENTIFY, TEMPERATURE, FIXED_LABEL]);
    const HUMIDITY_SENSOR: Layout = (&[dt(0x0307, 2)], &[DESC, IDENTIFY, HUMIDITY, FIXED_LABEL]);
    const LIGHT_SENSOR: Layout = (&[dt(0x0106, 3)], &[DESC, IDENTIFY, LUX, FIXED_LABEL]);
    const PRESSURE_SENSOR: Layout = (&[dt(0x0305, 2)], &[DESC, IDENTIFY, PRESSURE, FIXED_LABEL]);
    const CONTACT_SENSOR: Layout = (&[dt(0x0015, 1)], &[DESC, IDENTIFY, BOOLEAN, FIXED_LABEL]);
    const OCCUPANCY_SENSOR: Layout = (&[dt(0x0107, 4)], &[DESC, IDENTIFY, OCCUPANCY, FIXED_LABEL]);
    const LEAK_DETECTOR: Layout = (&[dt(0x0043, 1)], &[DESC, IDENTIFY, BOOLEAN, FIXED_LABEL]);
    const SMOKE: Layout = (&[dt(0x0076, 1)], &[DESC, IDENTIFY, SMOKE_ALARM, FIXED_LABEL]);
    const CO_ALARM_EP: Layout = (&[dt(0x0076, 1)], &[DESC, IDENTIFY, CO_ALARM, FIXED_LABEL]);
    const BATTERY: Layout = (&[dt(0x0011, 1)], &[DESC, POWER_SOURCE, FIXED_LABEL]);
    const MODE: Layout = (&[dt(0x0027, 1)], &[DESC, IDENTIFY, MODE_SELECT, FIXED_LABEL]);
    // The others depend on the entity's options: clusters per endpoint.
    let (device_type, cluster): (&'static [DeviceType], Cluster<'static>) = match (kind, entity.spec()) {
        (MatterKind::ExtendedColorLight, _) => return COLOR_LIGHT,
        (MatterKind::OnOffPlug, _) => return PLUG,
        (MatterKind::TemperatureSensor, _) => return TEMPERATURE_SENSOR,
        (MatterKind::HumiditySensor, _) => return HUMIDITY_SENSOR,
        (MatterKind::LightSensor, _) => return LIGHT_SENSOR,
        (MatterKind::PressureSensor, _) => return PRESSURE_SENSOR,
        (MatterKind::ContactSensor, _) => return CONTACT_SENSOR,
        (MatterKind::OccupancySensor, _) => return OCCUPANCY_SENSOR,
        (MatterKind::WaterLeakDetector, _) => return LEAK_DETECTOR,
        (MatterKind::SmokeAlarm, _) => return SMOKE,
        (MatterKind::CoAlarm, _) => return CO_ALARM_EP,
        (MatterKind::Battery, _) => return BATTERY,
        (MatterKind::ModeSelect, _) => return MODE,
        (MatterKind::AirQualitySensor(p), _) => {
            let clusters = Vec::from([DESC, IDENTIFY, AIR_QUALITY, concentration_cluster(p), FIXED_LABEL]);
            return (device_type!(0x002C, 1), Box::leak(clusters.into_boxed_slice()));
        }
        (MatterKind::GenericSwitch, Spec::Event { types }) => {
            let features = SwitchFeatures::of(types);
            let base = if features.multi_press() { SWITCH_MULTI } else { SWITCH };
            (device_type!(0x000F, 3), base.with_features(features.feature_map()))
        }
        (MatterKind::WindowCovering, Spec::Cover { position }) => {
            (device_type!(0x0202, 4), if position { COVER_POSITION } else { COVER })
        }
        (MatterKind::WaterValve, Spec::Cover { position }) => {
            let level = if position { valve::Feature::LEVEL.bits() } else { 0 };
            (device_type!(0x0042, 1), VALVE.with_features(level))
        }
        (MatterKind::DoorLock, Spec::Lock { open }) => {
            let unbolt = if open { door_lock::Feature::UNBOLT.bits() } else { 0 };
            (device_type!(0x000A, 3), DOOR_LOCK.with_features(unbolt))
        }
        (MatterKind::Fan, Spec::Fan { speed, presets, oscillation, direction }) => {
            let spec = FanSpec { speed, presets, oscillation, direction };
            (device_type!(0x002B, 3), FAN.with_features(spec.features()))
        }
        (MatterKind::Thermostat, Spec::Climate { modes, .. }) => {
            (device_type!(0x0301, 4), THERMOSTAT.with_features(map::thermostat_features(modes)))
        }
        (kind, _) => unreachable!("matter: {kind:?} does not match its entity"),
    };
    let clusters = Vec::from([DESC, IDENTIFY, cluster, FIXED_LABEL]);
    (device_type, Box::leak(clusters.into_boxed_slice()))
}

/// An entity endpoint.
pub(crate) struct Ep {
    pub(crate) id: u16,
    pub(crate) entity: &'static dyn Entity,
    pub(crate) kind: MatterKind,
    /// Data versions of the main cluster and of a second one (Level Control of a light,
    /// Concentration Measurement of an air quality sensor).
    pub(crate) datavers: [Dataver; 2],
    /// The Fixed Label list (the entity's name) and its data version, which never changes.
    pub(crate) labels: &'static [FixedLabelEntry<'static>],
    pub(crate) labels_dataver: u32,
}

impl Ep {
    pub(crate) fn on(&self) -> bool {
        match self.entity.state() {
            State::Bool(on) => on,
            State::Light(l) => l.on,
            _ => false,
        }
    }

    pub(crate) fn light(&self) -> LightState {
        match self.entity.state() {
            State::Light(l) => l,
            _ => LightState::default(),
        }
    }

    /// On/off and light state for building a command: the entity's [`Entity::target`], so
    /// commands that follow each other before the app applies them add up.
    pub(crate) fn target_on(&self) -> bool {
        match self.entity.target() {
            State::Bool(on) => on,
            State::Light(l) => l.on,
            _ => false,
        }
    }

    pub(crate) fn target_light(&self) -> LightState {
        match self.entity.target() {
            State::Light(l) => l,
            _ => LightState::default(),
        }
    }

    pub(crate) fn number(&self) -> Option<f32> {
        match self.entity.state() {
            State::Number(v) => Some(v),
            _ => None,
        }
    }

    pub(crate) fn set_on(&self, on: bool) {
        self.entity.command(Command::Set(on));
    }

    /// Sets the level (clamped to MinLevel 1, so a later On never lights at 0). `on: None`
    /// keeps the on/off state.
    pub(crate) fn set_level(&self, level: u8, on: Option<bool>) {
        let on = on.unwrap_or(self.target_light().on);
        self.entity.command(Command::Light { on: Some(on), brightness: Some(brightness_from_level(level)), rgb: None });
    }

    fn unit(&self) -> Option<crate::model::Unit> {
        self.entity.meta().unit
    }

    /// A reading as Matter's `MeasuredValue`.
    pub(crate) fn measured<T: TryFrom<i32>>(&self) -> Nullable<T> {
        let value = self.number().zip(self.unit()).and_then(|(v, unit)| map::measured_value(self.kind, unit, v));
        match value.and_then(|v| T::try_from(v).ok()) {
            Some(v) => Nullable::some(v),
            None => Nullable::none(),
        }
    }

    /// The reading in the Matter unit (ppm, µg/m³, %); `None` when unknown.
    pub(crate) fn matter_value(&self) -> Option<f32> {
        let value = self.number().zip(self.unit()).and_then(|(v, unit)| map::to_matter_unit(self.kind, unit, v));
        value.filter(|v| v.is_finite())
    }

    /// `MeasurementUnit` of an air quality reading.
    pub(crate) fn concentration_unit(&self) -> map::ConcentrationUnit {
        match (self.kind, self.unit()) {
            (MatterKind::AirQualitySensor(p), Some(unit)) => {
                map::concentration(p, unit, 0.0).map_or(map::ConcentrationUnit::Ppm, |(_, u)| u)
            }
            _ => map::ConcentrationUnit::Ppm,
        }
    }

    pub(crate) fn cover(&self) -> (Option<CoverState>, Option<u8>) {
        match self.entity.state() {
            State::Cover { state, position } => (state, position),
            _ => (None, None),
        }
    }

    pub(crate) fn lock(&self) -> Option<LockState> {
        match self.entity.state() {
            State::Lock(l) => Some(l),
            _ => None,
        }
    }

    pub(crate) fn fan(&self) -> FanState {
        match self.entity.state() {
            State::Fan(f) => f,
            _ => FanState::OFF,
        }
    }

    pub(crate) fn climate(&self) -> Option<ClimateState> {
        match self.entity.state() {
            State::Climate(c) => Some(c),
            _ => None,
        }
    }

    pub(crate) fn option(&self) -> Option<u8> {
        match self.entity.state() {
            State::Option(i) => Some(i),
            _ => None,
        }
    }
}

/// The node (root endpoint first, then one endpoint per mapped entity) and the entity endpoints.
pub(crate) fn build(device: &Device, root: Endpoint<'static>, rand: &mut impl Rng) -> (Node<'static>, &'static [Ep]) {
    let mut endpoints = Vec::from([root]);
    let mut eps = Vec::new();
    for entity in device.entities {
        let Some(kind) = matter_kind(*entity) else { continue };
        let id = eps.len() as u16 + 1;
        let (device_types, clusters) = layout(kind, *entity);
        endpoints.push(Endpoint::new(id, device_types, clusters));
        let labels =
            Box::leak(Box::new([FixedLabelEntry { label: "ha_entitylabel", value: label_value(entity.meta().name) }]));
        eps.push(Ep {
            id,
            entity: *entity,
            kind,
            datavers: [Dataver::new_rand(rand), Dataver::new_rand(rand)],
            labels,
            labels_dataver: Dataver::new_rand(rand).get(),
        });
        log::info!("matter: endpoint {id}: '{}' as {kind:?}", entity.meta().id);
    }
    (Node { endpoints: Box::leak(endpoints.into_boxed_slice()) }, Box::leak(eps.into_boxed_slice()))
}

/// Label values are at most 16 bytes (cut on a character boundary).
fn label_value(name: &'static str) -> &'static str {
    if name.len() <= 16 {
        return name;
    }
    let end = (0..=16).rev().find(|&i| name.is_char_boundary(i)).unwrap_or(0);
    log::warn!("matter: name '{name}' cut to 16 bytes");
    &name[..end]
}
