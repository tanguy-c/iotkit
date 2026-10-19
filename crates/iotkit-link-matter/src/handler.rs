//! The handler serving every entity endpoint, and the loop reporting entity changes.

use core::future::Future;

use embassy_futures::select::select;
use embassy_time::Timer;
use iotkit_model::{Binding, Spec, State};
use rs_matter_embassy::matter::{
    dm::{
        Dataver, Handler, HandlerContext, InvokeContext, InvokeReply, MatchContext, NonBlockingHandler, ReadContext,
        ReadReply, WriteContext,
        clusters::{
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
            fixed_label::{self, FixedLabelHandler},
        },
    },
    error::{Error, ErrorCode},
};

use crate::{
    MatterKind, Pollutant,
    appliances::{CoverH, FanH, LockH, ModeSelectH, SmokeCoH, ThermostatH, ValveH},
    clusters::{
        AirQualityH, BooleanH, Co2H, CoH, HumidityH, LevelH, LuxH, No2H, O3H, OccupancyH, OnOffH, Pm1H, Pm10H, Pm25H,
        PowerSourceH, PressureH, SwitchH, TemperatureH, TvocH, switch_features,
    },
    endpoints::{
        AIR_QUALITY, BOOLEAN, COVER, DOOR_LOCK, Ep, FAN, FIXED_LABEL, HUMIDITY, LEVEL, LUX, MODE_SELECT, OCCUPANCY,
        ON_OFF, POWER_SOURCE, PRESSURE, SMOKE_ALARM, SWITCH, TEMPERATURE, THERMOSTAT, VALVE, concentration_cluster,
    },
    map::{self, SwitchEvent},
};

/// Serves the entity endpoints by building the typed cluster handler on the fly.
pub(crate) struct Entities {
    pub(crate) eps: &'static [Ep],
}

/// Runs `$call` with `$h` bound to the adaptor for (`$ep`, `$cluster`).
macro_rules! dispatch {
    ($ep:expr, $eps:expr, $cluster:expr, |$h:ident| $call:expr) => {{
        let ep: &Ep = $ep;
        let cluster = $cluster;
        match ep.kind {
            MatterKind::TemperatureSensor if cluster == TEMPERATURE.id => {
                let $h = temperature::HandlerAdaptor(TemperatureH(ep));
                $call
            }
            MatterKind::HumiditySensor if cluster == HUMIDITY.id => {
                let $h = humidity::HandlerAdaptor(HumidityH(ep));
                $call
            }
            MatterKind::LightSensor if cluster == LUX.id => {
                let $h = lux::HandlerAdaptor(LuxH(ep));
                $call
            }
            MatterKind::PressureSensor if cluster == PRESSURE.id => {
                let $h = pressure::HandlerAdaptor(PressureH(ep));
                $call
            }
            MatterKind::ContactSensor | MatterKind::WaterLeakDetector if cluster == BOOLEAN.id => {
                let $h = boolean_state::HandlerAdaptor(BooleanH(ep));
                $call
            }
            MatterKind::OccupancySensor if cluster == OCCUPANCY.id => {
                let $h = occupancy::HandlerAdaptor(OccupancyH(ep));
                $call
            }
            MatterKind::SmokeAlarm | MatterKind::CoAlarm if cluster == SMOKE_ALARM.id => {
                let $h = smoke_co_alarm::HandlerAdaptor(SmokeCoH(ep));
                $call
            }
            MatterKind::AirQualitySensor(_) if cluster == AIR_QUALITY.id => {
                let $h = air_quality::HandlerAdaptor(AirQualityH(ep));
                $call
            }
            MatterKind::AirQualitySensor(p) if cluster == concentration_cluster(p).id => match p {
                Pollutant::Co2 => {
                    let $h = co2::HandlerAdaptor(Co2H(ep));
                    $call
                }
                Pollutant::Co => {
                    let $h = co::HandlerAdaptor(CoH(ep));
                    $call
                }
                Pollutant::No2 => {
                    let $h = no2::HandlerAdaptor(No2H(ep));
                    $call
                }
                Pollutant::O3 => {
                    let $h = o3::HandlerAdaptor(O3H(ep));
                    $call
                }
                Pollutant::Pm1 => {
                    let $h = pm1::HandlerAdaptor(Pm1H(ep));
                    $call
                }
                Pollutant::Pm25 => {
                    let $h = pm25::HandlerAdaptor(Pm25H(ep));
                    $call
                }
                Pollutant::Pm10 => {
                    let $h = pm10::HandlerAdaptor(Pm10H(ep));
                    $call
                }
                Pollutant::Tvoc => {
                    let $h = tvoc::HandlerAdaptor(TvocH(ep));
                    $call
                }
            },
            MatterKind::Battery if cluster == POWER_SOURCE.id => {
                let $h = power_source::HandlerAdaptor(PowerSourceH(ep, $eps));
                $call
            }
            MatterKind::GenericSwitch if cluster == SWITCH.id => {
                let $h = switch::HandlerAdaptor(SwitchH(ep));
                $call
            }
            MatterKind::ExtendedColorLight | MatterKind::OnOffPlug if cluster == ON_OFF.id => {
                let $h = on_off::HandlerAdaptor(OnOffH(ep));
                $call
            }
            MatterKind::ExtendedColorLight if cluster == LEVEL.id => {
                let $h = level_control::HandlerAdaptor(LevelH(ep));
                $call
            }
            MatterKind::WindowCovering if cluster == COVER.id => {
                let $h = window_covering::HandlerAdaptor(CoverH(ep));
                $call
            }
            MatterKind::WaterValve if cluster == VALVE.id => {
                let $h = valve::HandlerAdaptor(ValveH(ep));
                $call
            }
            MatterKind::DoorLock if cluster == DOOR_LOCK.id => {
                let $h = door_lock::HandlerAdaptor(LockH(ep));
                $call
            }
            MatterKind::Fan if cluster == FAN.id => {
                let $h = fan_control::HandlerAdaptor(FanH(ep));
                $call
            }
            MatterKind::Thermostat if cluster == THERMOSTAT.id => {
                let $h = thermostat::HandlerAdaptor(ThermostatH(ep));
                $call
            }
            MatterKind::ModeSelect if cluster == MODE_SELECT.id => {
                let $h = mode_select::HandlerAdaptor(ModeSelectH(ep));
                $call
            }
            _ if cluster == FIXED_LABEL.id => {
                let $h =
                    fixed_label::HandlerAdaptor(FixedLabelHandler::new(Dataver::new(ep.labels_dataver), ep.labels));
                $call
            }
            _ => Err(ErrorCode::ClusterNotFound.into()),
        }
    }};
}

impl Entities {
    fn ep(&self, id: u16) -> Result<&Ep, Error> {
        self.eps.iter().find(|ep| ep.id == id).ok_or_else(|| ErrorCode::EndpointNotFound.into())
    }

    /// Entity changes (from the app, MQTT or Matter) become attribute reports; a button's events
    /// become Switch events.
    async fn report(&self, ctx: impl HandlerContext) -> Result<(), Error> {
        loop {
            for ep in self.eps {
                if !ep.entity.take_dirty(Binding::MATTER) {
                    continue;
                }
                if ep.kind == MatterKind::GenericSwitch {
                    emit_switch(&ctx, ep);
                    continue;
                }
                for dataver in &ep.datavers {
                    dataver.changed();
                }
                ctx.notify_endpoint_changed(ep.id);
            }
            // The timer also catches a change that came between the scan and the wait.
            select(iotkit_model::changed(Binding::MATTER), Timer::after_secs(1)).await;
        }
    }
}

/// Sends a button's last event as the Switch events of the same gesture.
fn emit_switch(ctx: &impl HandlerContext, ep: &Ep) {
    let (State::Event(i), Spec::Event { types }) = (ep.entity.state(), ep.entity.spec()) else { return };
    let Some(gesture) = types.get(usize::from(i)).and_then(|t| map::gesture(t)) else { return };
    for event in map::switch_events(gesture, switch_features(ep)) {
        let sent = match event {
            SwitchEvent::InitialPress => switch::InitialPress::emit_for(ctx, ep.id, |e| e.new_position(1)?.end()),
            SwitchEvent::LongPress => switch::LongPress::emit_for(ctx, ep.id, |e| e.new_position(1)?.end()),
            SwitchEvent::ShortRelease => switch::ShortRelease::emit_for(ctx, ep.id, |e| e.previous_position(1)?.end()),
            SwitchEvent::LongRelease => switch::LongRelease::emit_for(ctx, ep.id, |e| e.previous_position(1)?.end()),
            SwitchEvent::MultiPressOngoing(n) => switch::MultiPressOngoing::emit_for(ctx, ep.id, |e| {
                e.new_position(1)?.current_number_of_presses_counted(n)?.end()
            }),
            SwitchEvent::MultiPressComplete(n) => switch::MultiPressComplete::emit_for(ctx, ep.id, |e| {
                e.previous_position(1)?.total_number_of_presses_counted(n)?.end()
            }),
        };
        if let Err(e) = sent {
            log::warn!("matter: cannot send a switch event of endpoint {}: {e:?}", ep.id);
            return;
        }
    }
}

impl Handler for Entities {
    fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> Result<(), Error> {
        let attr = ctx.attr();
        let (endpoint, cluster) = (attr.endpoint_id, attr.cluster_id);
        dispatch!(self.ep(endpoint)?, self.eps, cluster, |h| h.read(ctx, reply))
    }

    fn write(&self, ctx: impl WriteContext) -> Result<(), Error> {
        let attr = ctx.attr();
        let (endpoint, cluster) = (attr.endpoint_id, attr.cluster_id);
        dispatch!(self.ep(endpoint)?, self.eps, cluster, |h| h.write(ctx))
    }

    fn invoke(&self, ctx: impl InvokeContext, reply: impl InvokeReply) -> Result<(), Error> {
        let cmd = ctx.cmd();
        let (endpoint, cluster) = (cmd.endpoint_id, cmd.cluster_id);
        dispatch!(self.ep(endpoint)?, self.eps, cluster, |h| h.invoke(ctx, reply))
    }

    fn bump_dataver(&self, ctx: impl MatchContext) {
        for ep in self.eps.iter().filter(|ep| ctx.endpt().is_none_or(|e| e == ep.id)) {
            for dataver in &ep.datavers {
                dataver.changed();
            }
        }
    }

    fn run(&self, ctx: impl HandlerContext) -> impl Future<Output = Result<(), Error>> {
        self.report(ctx)
    }
}

impl NonBlockingHandler for Entities {}
