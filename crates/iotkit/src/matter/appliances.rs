//! The clusters of the appliances: Window Covering, Valve, Door Lock, Fan Control, Thermostat
//! and Mode Select, plus the Smoke CO Alarm of a binary sensor. Ported from esp-ha.
//!
//! Reads come from the entity's state; commands and writes go through `Entity::command`, like
//! commands over MQTT. The conversions are in [`map`].

use crate::model::{
    ClimateCommand, ClimateState, Command, CoverCommand, CoverState, FanCommand, FanDirection, HvacMode, LockCommand,
    Spec,
};
use rs_matter_embassy::matter::{
    dm::{
        ArrayAttributeRead, Cluster, InvokeContext, ReadContext, WriteContext,
        clusters::decl::{
            door_lock, fan_control, mode_select, smoke_co_alarm, thermostat, valve_configuration_and_control as valve,
            window_covering,
        },
    },
    error::{Error, ErrorCode},
    tlv::{Nullable, TLVBuilderParent, Utf8StrBuilder},
};

use crate::matter::{
    MatterKind,
    endpoints::{COVER, DOOR_LOCK, Ep, FAN, MODE_SELECT, SMOKE_ALARM, THERMOSTAT, VALVE},
    map::{self, FanSpec},
};

/// `dataver` and `dataver_changed` on the endpoint's main data version.
macro_rules! dataver {
    () => {
        fn dataver(&self) -> u32 {
            self.0.datavers[0].get()
        }

        fn dataver_changed(&self) {
            self.0.datavers[0].changed();
        }
    };
}

/// A command the cluster does not accept (not in its command list).
fn not_accepted<T>() -> Result<T, Error> {
    Err(ErrorCode::CommandNotFound.into())
}

fn nullable<T>(value: Option<T>) -> Nullable<T> {
    match value {
        Some(v) => Nullable::some(v),
        None => Nullable::none(),
    }
}

/// Hands a command to the entity; a refused one is a constraint error.
fn command(ep: &Ep, command: Command) -> Result<(), Error> {
    if ep.entity.command(command) { Ok(()) } else { Err(ErrorCode::ConstraintError.into()) }
}

// ---------------------------------------------------------------- smoke / CO alarm

/// A smoke or CO binary sensor: `on` is a critical alarm.
pub(crate) struct SmokeCoH<'a>(pub(crate) &'a Ep);

impl SmokeCoH<'_> {
    fn alarm(&self) -> smoke_co_alarm::AlarmStateEnum {
        if self.0.on() { smoke_co_alarm::AlarmStateEnum::Critical } else { smoke_co_alarm::AlarmStateEnum::Normal }
    }
}

impl smoke_co_alarm::ClusterHandler for SmokeCoH<'_> {
    // Also serves the CO alarm: the endpoint's cluster tells which state is there.
    const CLUSTER: Cluster<'static> = SMOKE_ALARM;

    dataver!();

    fn expressed_state(&self, _ctx: impl ReadContext) -> Result<smoke_co_alarm::ExpressedStateEnum, Error> {
        use smoke_co_alarm::ExpressedStateEnum::*;
        Ok(match (self.0.on(), self.0.kind) {
            (false, _) => Normal,
            (true, MatterKind::CoAlarm) => COAlarm,
            (true, _) => SmokeAlarm,
        })
    }

    fn smoke_state(&self, _ctx: impl ReadContext) -> Result<smoke_co_alarm::AlarmStateEnum, Error> {
        Ok(self.alarm())
    }

    fn co_state(&self, _ctx: impl ReadContext) -> Result<smoke_co_alarm::AlarmStateEnum, Error> {
        Ok(self.alarm())
    }

    fn battery_alert(&self, _ctx: impl ReadContext) -> Result<smoke_co_alarm::AlarmStateEnum, Error> {
        Ok(smoke_co_alarm::AlarmStateEnum::Normal)
    }

    fn test_in_progress(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        Ok(false)
    }

    fn hardware_fault_alert(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        Ok(false)
    }

    fn end_of_service_alert(&self, _ctx: impl ReadContext) -> Result<smoke_co_alarm::EndOfServiceEnum, Error> {
        Ok(smoke_co_alarm::EndOfServiceEnum::Normal)
    }

    // A binary sensor has nothing to test.
    fn handle_self_test_request(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        Ok(())
    }
}

// ---------------------------------------------------------------- cover

pub(crate) struct CoverH<'a>(pub(crate) &'a Ep);

impl CoverH<'_> {
    fn takes_position(&self) -> bool {
        matches!(self.0.entity.spec(), Spec::Cover { position: true })
    }

    fn types(&self) -> (u8, u8) {
        map::cover_types(self.0.entity.meta().device_class)
    }
}

impl window_covering::ClusterHandler for CoverH<'_> {
    const CLUSTER: Cluster<'static> = COVER;

    dataver!();

    fn r#type(&self, _ctx: impl ReadContext) -> Result<window_covering::Type, Error> {
        use window_covering::Type::*;
        Ok(match self.types().0 {
            0 => RollerShade,
            4 => Drapery,
            5 => Awning,
            8 => TiltBlindLiftAndTilt,
            _ => Shutter,
        })
    }

    fn end_product_type(&self, _ctx: impl ReadContext) -> Result<window_covering::EndProductType, Error> {
        use window_covering::EndProductType::*;
        Ok(match self.types().1 {
            0 => RollerShade,
            10 => InteriorBlind,
            16 => CentralCurtain,
            19 => AwningTerracePatio,
            _ => RollerShutter,
        })
    }

    fn config_status(&self, _ctx: impl ReadContext) -> Result<window_covering::ConfigStatus, Error> {
        Ok(window_covering::ConfigStatus::OPERATIONAL | window_covering::ConfigStatus::LIFT_POSITION_AWARE)
    }

    fn operational_status(&self, _ctx: impl ReadContext) -> Result<window_covering::OperationalStatus, Error> {
        let (state, _) = self.0.cover();
        Ok(window_covering::OperationalStatus::from_bits_retain(map::cover_operational_status(state)))
    }

    fn current_position_lift_percent_100_ths(&self, _ctx: impl ReadContext) -> Result<Nullable<u16>, Error> {
        let (state, position) = self.0.cover();
        Ok(nullable(map::cover_position_of(state, position).map(map::cover_percent100ths)))
    }

    // Fully open or closed while moving there, else where it is.
    fn target_position_lift_percent_100_ths(&self, ctx: impl ReadContext) -> Result<Nullable<u16>, Error> {
        Ok(match self.0.cover().0 {
            Some(CoverState::Opening) => Nullable::some(0),
            Some(CoverState::Closing) => Nullable::some(10000),
            _ => self.current_position_lift_percent_100_ths(ctx)?,
        })
    }

    fn mode(&self, _ctx: impl ReadContext) -> Result<window_covering::Mode, Error> {
        Ok(window_covering::Mode::empty())
    }

    fn set_mode(&self, _ctx: impl WriteContext, _value: window_covering::Mode) -> Result<(), Error> {
        Ok(())
    }

    fn handle_up_or_open(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        command(self.0, Command::Cover(CoverCommand::Open))
    }

    fn handle_down_or_close(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        command(self.0, Command::Cover(CoverCommand::Close))
    }

    fn handle_stop_motion(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        command(self.0, Command::Cover(CoverCommand::Stop))
    }

    fn handle_go_to_lift_percentage(
        &self,
        _ctx: impl InvokeContext,
        r: window_covering::GoToLiftPercentageRequest<'_>,
    ) -> Result<(), Error> {
        if !self.takes_position() {
            return not_accepted();
        }
        let percent100ths = r.lift_percent_100_ths_value()?;
        if percent100ths > 10000 {
            return Err(ErrorCode::ConstraintError.into());
        }
        command(self.0, Command::Cover(CoverCommand::Position(map::cover_position(percent100ths))))
    }

    fn handle_go_to_lift_value(
        &self,
        _ctx: impl InvokeContext,
        _r: window_covering::GoToLiftValueRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_go_to_tilt_value(
        &self,
        _ctx: impl InvokeContext,
        _r: window_covering::GoToTiltValueRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_go_to_tilt_percentage(
        &self,
        _ctx: impl InvokeContext,
        _r: window_covering::GoToTiltPercentageRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }
}

// ---------------------------------------------------------------- valve

pub(crate) struct ValveH<'a>(pub(crate) &'a Ep);

impl ValveH<'_> {
    fn takes_level(&self) -> bool {
        matches!(self.0.entity.spec(), Spec::Cover { position: true })
    }
}

fn valve_state(state: Option<u8>) -> Nullable<valve::ValveStateEnum> {
    nullable(state.map(|s| match s {
        0 => valve::ValveStateEnum::Closed,
        1 => valve::ValveStateEnum::Open,
        _ => valve::ValveStateEnum::Transitioning,
    }))
}

impl valve::ClusterHandler for ValveH<'_> {
    const CLUSTER: Cluster<'static> = VALVE;

    dataver!();

    // No automatic closing.
    fn open_duration(&self, _ctx: impl ReadContext) -> Result<Nullable<u32>, Error> {
        Ok(Nullable::none())
    }

    fn default_open_duration(&self, _ctx: impl ReadContext) -> Result<Nullable<u32>, Error> {
        Ok(Nullable::none())
    }

    fn remaining_duration(&self, _ctx: impl ReadContext) -> Result<Nullable<u32>, Error> {
        Ok(Nullable::none())
    }

    fn set_default_open_duration(&self, _ctx: impl WriteContext, value: Nullable<u32>) -> Result<(), Error> {
        if value.is_none() { Ok(()) } else { Err(ErrorCode::ConstraintError.into()) }
    }

    fn current_state(&self, _ctx: impl ReadContext) -> Result<Nullable<valve::ValveStateEnum>, Error> {
        let (state, position) = self.0.cover();
        Ok(valve_state(map::valve_state(state, position)))
    }

    fn target_state(&self, _ctx: impl ReadContext) -> Result<Nullable<valve::ValveStateEnum>, Error> {
        Ok(valve_state(map::valve_target(self.0.cover().0)))
    }

    fn current_level(&self, _ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(nullable(self.0.cover().1))
    }

    fn target_level(&self, _ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(Nullable::none())
    }

    fn handle_open(&self, _ctx: impl InvokeContext, r: valve::OpenRequest<'_>) -> Result<(), Error> {
        if r.open_duration()?.is_some_and(|d| d.is_some()) {
            log::warn!("matter: valve endpoint {}: open duration ignored", self.0.id);
        }
        let c = match r.target_level()? {
            Some(level) if self.takes_level() => {
                if level > 100 {
                    return Err(ErrorCode::ConstraintError.into());
                }
                CoverCommand::Position(level)
            }
            _ => CoverCommand::Open,
        };
        command(self.0, Command::Cover(c))
    }

    fn handle_close(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        command(self.0, Command::Cover(CoverCommand::Close))
    }
}

// ---------------------------------------------------------------- lock

pub(crate) struct LockH<'a>(pub(crate) &'a Ep);

impl LockH<'_> {
    fn unbolt(&self) -> bool {
        matches!(self.0.entity.spec(), Spec::Lock { open: true })
    }
}

impl door_lock::ClusterHandler for LockH<'_> {
    const CLUSTER: Cluster<'static> = DOOR_LOCK;

    dataver!();

    fn lock_state(&self, _ctx: impl ReadContext) -> Result<Nullable<door_lock::DlLockState>, Error> {
        use door_lock::DlLockState::*;
        Ok(nullable(self.0.lock().map(|state| match map::lock_state(state, self.unbolt()) {
            1 => Locked,
            2 => Unlocked,
            3 => Unlatched,
            _ => NotFullyLocked,
        })))
    }

    fn lock_type(&self, _ctx: impl ReadContext) -> Result<door_lock::DlLockType, Error> {
        Ok(door_lock::DlLockType::DeadBolt)
    }

    fn actuator_enabled(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        Ok(true)
    }

    fn operating_mode(&self, _ctx: impl ReadContext) -> Result<door_lock::OperatingModeEnum, Error> {
        Ok(door_lock::OperatingModeEnum::Normal)
    }

    // A cleared bit means supported: Normal and Vacation (both required).
    fn supported_operating_modes(&self, _ctx: impl ReadContext) -> Result<door_lock::DlSupportedOperatingModes, Error> {
        Ok(door_lock::DlSupportedOperatingModes::from_bits_retain(0xFFFC))
    }

    fn set_operating_mode(&self, _ctx: impl WriteContext, value: door_lock::OperatingModeEnum) -> Result<(), Error> {
        match value {
            door_lock::OperatingModeEnum::Normal => Ok(()),
            _ => Err(ErrorCode::ConstraintError.into()),
        }
    }

    fn handle_lock_door(&self, _ctx: impl InvokeContext, _r: door_lock::LockDoorRequest<'_>) -> Result<(), Error> {
        command(self.0, Command::Lock(LockCommand::Lock))
    }

    fn handle_unlock_door(&self, _ctx: impl InvokeContext, _r: door_lock::UnlockDoorRequest<'_>) -> Result<(), Error> {
        command(self.0, Command::Lock(map::unlock_door_command(self.unbolt())))
    }

    fn handle_unbolt_door(&self, _ctx: impl InvokeContext, _r: door_lock::UnboltDoorRequest<'_>) -> Result<(), Error> {
        if !self.unbolt() {
            return not_accepted();
        }
        command(self.0, Command::Lock(LockCommand::Unlock))
    }

    fn handle_unlock_with_timeout(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::UnlockWithTimeoutRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_week_day_schedule(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::SetWeekDayScheduleRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_get_week_day_schedule<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::GetWeekDayScheduleRequest<'_>,
        _response: door_lock::GetWeekDayScheduleResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_clear_week_day_schedule(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::ClearWeekDayScheduleRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_year_day_schedule(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::SetYearDayScheduleRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_get_year_day_schedule<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::GetYearDayScheduleRequest<'_>,
        _response: door_lock::GetYearDayScheduleResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_clear_year_day_schedule(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::ClearYearDayScheduleRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_holiday_schedule(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::SetHolidayScheduleRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_get_holiday_schedule<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::GetHolidayScheduleRequest<'_>,
        _response: door_lock::GetHolidayScheduleResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_clear_holiday_schedule(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::ClearHolidayScheduleRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_user(&self, _ctx: impl InvokeContext, _r: door_lock::SetUserRequest<'_>) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_get_user<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::GetUserRequest<'_>,
        _response: door_lock::GetUserResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_clear_user(&self, _ctx: impl InvokeContext, _r: door_lock::ClearUserRequest<'_>) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_credential<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::SetCredentialRequest<'_>,
        _response: door_lock::SetCredentialResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_get_credential_status<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::GetCredentialStatusRequest<'_>,
        _response: door_lock::GetCredentialStatusResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_clear_credential(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::ClearCredentialRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_aliro_reader_config(
        &self,
        _ctx: impl InvokeContext,
        _r: door_lock::SetAliroReaderConfigRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_clear_aliro_reader_config(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        not_accepted()
    }
}

// ---------------------------------------------------------------- fan

pub(crate) struct FanH<'a>(pub(crate) &'a Ep);

impl FanH<'_> {
    fn spec(&self) -> FanSpec {
        match self.0.entity.spec() {
            Spec::Fan { speed, presets, oscillation, direction } => FanSpec { speed, presets, oscillation, direction },
            _ => FanSpec { speed: false, presets: &[], oscillation: false, direction: false },
        }
    }

    fn command(&self, c: FanCommand) -> Result<(), Error> {
        command(self.0, Command::Fan(c))
    }
}

impl fan_control::ClusterHandler for FanH<'_> {
    const CLUSTER: Cluster<'static> = FAN;

    dataver!();

    fn fan_mode(&self, _ctx: impl ReadContext) -> Result<fan_control::FanModeEnum, Error> {
        use fan_control::FanModeEnum::*;
        Ok(match self.spec().mode_of(&self.0.fan()) {
            map::fan_mode::OFF => Off,
            map::fan_mode::LOW => Low,
            map::fan_mode::MEDIUM => Medium,
            map::fan_mode::AUTO => Auto,
            _ => High,
        })
    }

    fn fan_mode_sequence(&self, _ctx: impl ReadContext) -> Result<fan_control::FanModeSequenceEnum, Error> {
        Ok(match self.spec().mode_sequence() {
            2 => fan_control::FanModeSequenceEnum::OffLowMedHighAuto,
            _ => fan_control::FanModeSequenceEnum::OffLowMedHigh,
        })
    }

    fn percent_setting(&self, _ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(Nullable::some(self.spec().percent(&self.0.fan())))
    }

    fn percent_current(&self, _ctx: impl ReadContext) -> Result<u8, Error> {
        Ok(self.spec().percent(&self.0.fan()))
    }

    fn rock_support(&self, _ctx: impl ReadContext) -> Result<fan_control::RockBitmap, Error> {
        Ok(fan_control::RockBitmap::ROCK_LEFT_RIGHT)
    }

    fn rock_setting(&self, _ctx: impl ReadContext) -> Result<fan_control::RockBitmap, Error> {
        Ok(if self.0.fan().oscillating {
            fan_control::RockBitmap::ROCK_LEFT_RIGHT
        } else {
            fan_control::RockBitmap::empty()
        })
    }

    fn airflow_direction(&self, _ctx: impl ReadContext) -> Result<fan_control::AirflowDirectionEnum, Error> {
        Ok(match map::airflow_direction(self.0.fan().direction) {
            1 => fan_control::AirflowDirectionEnum::Reverse,
            _ => fan_control::AirflowDirectionEnum::Forward,
        })
    }

    fn set_fan_mode(&self, _ctx: impl WriteContext, value: fan_control::FanModeEnum) -> Result<(), Error> {
        let c = self.spec().command_for_mode(value as u8).ok_or(ErrorCode::ConstraintError)?;
        self.command(c)
    }

    fn set_percent_setting(&self, _ctx: impl WriteContext, value: Nullable<u8>) -> Result<(), Error> {
        match value.into_option() {
            Some(percent) if percent <= 100 => self.command(self.spec().command_for_percent(percent)),
            _ => Err(ErrorCode::ConstraintError.into()),
        }
    }

    fn set_rock_setting(&self, _ctx: impl WriteContext, value: fan_control::RockBitmap) -> Result<(), Error> {
        self.command(FanCommand::Oscillate(!value.is_empty()))
    }

    fn set_airflow_direction(
        &self,
        _ctx: impl WriteContext,
        value: fan_control::AirflowDirectionEnum,
    ) -> Result<(), Error> {
        self.command(FanCommand::Direction(match value {
            fan_control::AirflowDirectionEnum::Reverse => FanDirection::Reverse,
            fan_control::AirflowDirectionEnum::Forward => FanDirection::Forward,
        }))
    }

    fn handle_step(&self, _ctx: impl InvokeContext, _r: fan_control::StepRequest<'_>) -> Result<(), Error> {
        not_accepted()
    }
}

// ---------------------------------------------------------------- thermostat

pub(crate) struct ThermostatH<'a>(pub(crate) &'a Ep);

/// A thermostat's spec: modes, target range and step.
struct ClimateSpec {
    modes: &'static [HvacMode],
    min: f32,
    max: f32,
    step: f32,
}

impl ThermostatH<'_> {
    fn spec(&self) -> ClimateSpec {
        match self.0.entity.spec() {
            Spec::Climate { modes, min, max, step } => ClimateSpec { modes, min, max, step },
            _ => ClimateSpec { modes: &[HvacMode::Off], min: 7.0, max: 35.0, step: 0.5 },
        }
    }

    /// The state, or the initial one (first mode, 21 °C within the range).
    fn state(&self) -> ClimateState {
        let spec = self.spec();
        self.0.climate().unwrap_or(ClimateState {
            mode: spec.modes[0],
            target: 21f32.clamp(spec.min, spec.max),
            current: None,
            action: None,
        })
    }

    fn set_target(&self, centi: i16) -> Result<(), Error> {
        let spec = self.spec();
        let target = map::climate_target(centi, spec.min, spec.max, spec.step).ok_or(ErrorCode::ConstraintError)?;
        command(self.0, Command::Climate(ClimateCommand::Target(target)))
    }
}

impl thermostat::ClusterHandler for ThermostatH<'_> {
    const CLUSTER: Cluster<'static> = THERMOSTAT;

    dataver!();

    fn local_temperature(&self, _ctx: impl ReadContext) -> Result<Nullable<i16>, Error> {
        Ok(nullable(self.state().current.filter(|t| t.is_finite()).map(map::centi_celsius)))
    }

    fn control_sequence_of_operation(
        &self,
        _ctx: impl ReadContext,
    ) -> Result<thermostat::ControlSequenceOfOperationEnum, Error> {
        use thermostat::ControlSequenceOfOperationEnum::*;
        Ok(match map::control_sequence(self.spec().modes) {
            0 => CoolingOnly,
            4 => CoolingAndHeating,
            _ => HeatingOnly,
        })
    }

    // Fixed by the thermostat's modes.
    fn set_control_sequence_of_operation(
        &self,
        _ctx: impl WriteContext,
        value: thermostat::ControlSequenceOfOperationEnum,
    ) -> Result<(), Error> {
        if value as u8 == map::control_sequence(self.spec().modes) {
            Ok(())
        } else {
            Err(ErrorCode::ConstraintError.into())
        }
    }

    fn system_mode(&self, _ctx: impl ReadContext) -> Result<thermostat::SystemModeEnum, Error> {
        use thermostat::SystemModeEnum::*;
        Ok(match map::system_mode_of(self.state().mode) {
            map::system_mode::AUTO => Auto,
            map::system_mode::COOL => Cool,
            map::system_mode::HEAT => Heat,
            map::system_mode::FAN_ONLY => FanOnly,
            map::system_mode::DRY => Dry,
            _ => Off,
        })
    }

    fn set_system_mode(&self, _ctx: impl WriteContext, value: thermostat::SystemModeEnum) -> Result<(), Error> {
        let mode = map::hvac_mode_for(value as u8, self.spec().modes).ok_or(ErrorCode::ConstraintError)?;
        command(self.0, Command::Climate(ClimateCommand::Mode(mode)))
    }

    // One target: both setpoints show it.
    fn occupied_heating_setpoint(&self, _ctx: impl ReadContext) -> Result<i16, Error> {
        Ok(map::centi_celsius(self.state().target))
    }

    fn occupied_cooling_setpoint(&self, _ctx: impl ReadContext) -> Result<i16, Error> {
        Ok(map::centi_celsius(self.state().target))
    }

    fn set_occupied_heating_setpoint(&self, _ctx: impl WriteContext, value: i16) -> Result<(), Error> {
        self.set_target(value)
    }

    fn set_occupied_cooling_setpoint(&self, _ctx: impl WriteContext, value: i16) -> Result<(), Error> {
        self.set_target(value)
    }

    fn abs_min_heat_setpoint_limit(&self, _ctx: impl ReadContext) -> Result<i16, Error> {
        Ok(map::centi_celsius(self.spec().min))
    }

    fn abs_max_heat_setpoint_limit(&self, _ctx: impl ReadContext) -> Result<i16, Error> {
        Ok(map::centi_celsius(self.spec().max))
    }

    fn abs_min_cool_setpoint_limit(&self, _ctx: impl ReadContext) -> Result<i16, Error> {
        Ok(map::centi_celsius(self.spec().min))
    }

    fn abs_max_cool_setpoint_limit(&self, _ctx: impl ReadContext) -> Result<i16, Error> {
        Ok(map::centi_celsius(self.spec().max))
    }

    // Both setpoints are the same target.
    fn min_setpoint_dead_band(&self, _ctx: impl ReadContext) -> Result<i8, Error> {
        Ok(0)
    }

    fn thermostat_running_state(&self, _ctx: impl ReadContext) -> Result<thermostat::RelayStateBitmap, Error> {
        Ok(thermostat::RelayStateBitmap::from_bits_retain(map::running_state(self.state().action)))
    }

    fn handle_setpoint_raise_lower(
        &self,
        _ctx: impl InvokeContext,
        r: thermostat::SetpointRaiseLowerRequest<'_>,
    ) -> Result<(), Error> {
        let spec = self.spec();
        let target = map::raise_target(self.state().target, r.amount()?, spec.min, spec.max, spec.step);
        command(self.0, Command::Climate(ClimateCommand::Target(target)))
    }

    fn handle_set_weekly_schedule(
        &self,
        _ctx: impl InvokeContext,
        _r: thermostat::SetWeeklyScheduleRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_get_weekly_schedule<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: thermostat::GetWeeklyScheduleRequest<'_>,
        _response: thermostat::GetWeeklyScheduleResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_clear_weekly_schedule(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_active_schedule_request(
        &self,
        _ctx: impl InvokeContext,
        _r: thermostat::SetActiveScheduleRequestRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_set_active_preset_request(
        &self,
        _ctx: impl InvokeContext,
        _r: thermostat::SetActivePresetRequestRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_add_thermostat_suggestion<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: thermostat::AddThermostatSuggestionRequest<'_>,
        _response: thermostat::AddThermostatSuggestionResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }

    fn handle_remove_thermostat_suggestion(
        &self,
        _ctx: impl InvokeContext,
        _r: thermostat::RemoveThermostatSuggestionRequest<'_>,
    ) -> Result<(), Error> {
        not_accepted()
    }

    fn handle_atomic_request<P: TLVBuilderParent>(
        &self,
        _ctx: impl InvokeContext,
        _r: thermostat::AtomicRequestRequest<'_>,
        _response: thermostat::AtomicResponseBuilder<P>,
    ) -> Result<P, Error> {
        not_accepted()
    }
}

// ---------------------------------------------------------------- mode select

/// A `Select`: mode `i` is its option `i`, labelled with the option.
pub(crate) struct ModeSelectH<'a>(pub(crate) &'a Ep);

impl ModeSelectH<'_> {
    fn options(&self) -> &'static [&'static str] {
        match self.0.entity.spec() {
            Spec::Select { options } => options,
            _ => &[],
        }
    }

    fn write_option<P: TLVBuilderParent>(
        builder: mode_select::ModeOptionStructBuilder<P>,
        mode: usize,
        label: &str,
    ) -> Result<P, Error> {
        builder.label(label)?.mode(mode as u8)?.semantic_tags()?.end()?.end()
    }
}

impl mode_select::ClusterHandler for ModeSelectH<'_> {
    const CLUSTER: Cluster<'static> = MODE_SELECT;

    dataver!();

    fn description<P: TLVBuilderParent>(&self, _ctx: impl ReadContext, builder: Utf8StrBuilder<P>) -> Result<P, Error> {
        builder.set(self.0.labels.first().map_or("", |l| l.value))
    }

    fn standard_namespace(&self, _ctx: impl ReadContext) -> Result<Nullable<u16>, Error> {
        Ok(Nullable::none())
    }

    fn supported_modes<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: ArrayAttributeRead<
            mode_select::ModeOptionStructArrayBuilder<P>,
            mode_select::ModeOptionStructBuilder<P>,
        >,
    ) -> Result<P, Error> {
        let options = self.options();
        match builder {
            ArrayAttributeRead::ReadAll(mut array) => {
                for (mode, label) in options.iter().enumerate() {
                    array = Self::write_option(array.push()?, mode, label)?;
                }
                array.end()
            }
            ArrayAttributeRead::ReadOne(index, item) => {
                let label = options.get(index as usize).ok_or(ErrorCode::ConstraintError)?;
                Self::write_option(item, index as usize, label)
            }
            ArrayAttributeRead::ReadNone(array) => array.end(),
        }
    }

    // The first option until one is set.
    fn current_mode(&self, _ctx: impl ReadContext) -> Result<u8, Error> {
        Ok(self.0.option().unwrap_or(0))
    }

    fn handle_change_to_mode(
        &self,
        _ctx: impl InvokeContext,
        r: mode_select::ChangeToModeRequest<'_>,
    ) -> Result<(), Error> {
        command(self.0, Command::Option(r.new_mode()?))
    }
}
