//! The handler serving every entity endpoint, and the loop reporting entity changes.

use core::future::Future;

use embassy_futures::select::select;
use embassy_time::Timer;
use iotkit_model::Binding;
use rs_matter_embassy::matter::{
    dm::{
        Handler, HandlerContext, InvokeContext, InvokeReply, MatchContext, NonBlockingHandler, ReadContext, ReadReply,
        WriteContext,
        clusters::decl::{boolean_state, level_control, on_off, temperature_measurement as temperature},
    },
    error::{Error, ErrorCode},
};

use crate::{
    MatterKind,
    clusters::{BooleanH, LevelH, OnOffH, TemperatureH},
    endpoints::{BOOLEAN, Ep, LEVEL, ON_OFF, TEMPERATURE},
};

/// Serves the entity endpoints by building the typed cluster handler on the fly.
pub(crate) struct Entities {
    pub(crate) eps: &'static [Ep],
}

/// Runs `$call` with `$h` bound to the adaptor for (`$ep`, `$cluster`).
macro_rules! dispatch {
    ($ep:expr, $cluster:expr, |$h:ident| $call:expr) => {{
        let ep: &Ep = $ep;
        let cluster = $cluster;
        match ep.kind {
            MatterKind::TemperatureSensor if cluster == TEMPERATURE.id => {
                let $h = temperature::HandlerAdaptor(TemperatureH(ep));
                $call
            }
            MatterKind::ContactSensor if cluster == BOOLEAN.id => {
                let $h = boolean_state::HandlerAdaptor(BooleanH(ep));
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
            _ => Err(ErrorCode::ClusterNotFound.into()),
        }
    }};
}

impl Entities {
    fn ep(&self, id: u16) -> Result<&Ep, Error> {
        self.eps.iter().find(|ep| ep.id == id).ok_or_else(|| ErrorCode::EndpointNotFound.into())
    }

    /// Entity changes (from the app, MQTT or Matter) become attribute reports.
    async fn report(&self, ctx: impl HandlerContext) -> Result<(), Error> {
        loop {
            for ep in self.eps {
                if ep.entity.take_dirty(Binding::MATTER) {
                    for dataver in &ep.datavers {
                        dataver.changed();
                    }
                    ctx.notify_endpoint_changed(ep.id);
                }
            }
            // The timer also catches a change that came between the scan and the wait.
            select(iotkit_model::changed(Binding::MATTER), Timer::after_secs(1)).await;
        }
    }
}

impl Handler for Entities {
    fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> Result<(), Error> {
        let attr = ctx.attr();
        let (endpoint, cluster) = (attr.endpoint_id, attr.cluster_id);
        dispatch!(self.ep(endpoint)?, cluster, |h| h.read(ctx, reply))
    }

    fn write(&self, ctx: impl WriteContext) -> Result<(), Error> {
        let attr = ctx.attr();
        let (endpoint, cluster) = (attr.endpoint_id, attr.cluster_id);
        dispatch!(self.ep(endpoint)?, cluster, |h| h.write(ctx))
    }

    fn invoke(&self, ctx: impl InvokeContext, reply: impl InvokeReply) -> Result<(), Error> {
        let cmd = ctx.cmd();
        let (endpoint, cluster) = (cmd.endpoint_id, cmd.cluster_id);
        dispatch!(self.ep(endpoint)?, cluster, |h| h.invoke(ctx, reply))
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
