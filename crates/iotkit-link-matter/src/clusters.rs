//! The cluster handlers of the entity endpoints, on rs-matter's generated cluster traits.

use alloc::{boxed::Box, vec::Vec};
use core::{future::Future, pin::Pin};

use embassy_futures::select::select_slice;
use iotkit_model::Command;
use rs_matter_embassy::matter::{
    crypto::Rng,
    dm::{
        AsyncHandler, Cluster, Dataver, HandlerContext, InvokeContext, InvokeReply, LifecycleOp, MatchContext,
        ReadContext, ReadReply, WriteContext,
        clusters::{
            app::{
                color_control::{
                    self as color, AttributeDefaults, ColorCapabilitiesBitmap, ColorControlHandler, ColorControlHooks,
                    RgbGamma, SetDeviceColor,
                },
                level_control::NoOnOff,
                on_off::NoLevelControl,
            },
            decl::{boolean_state, level_control, on_off, temperature_measurement as temperature},
        },
    },
    error::{Error, ErrorCode},
    tlv::Nullable,
};

use crate::{
    MatterKind,
    endpoints::{BOOLEAN, COLOR, Ep, LEVEL, ON_OFF, TEMPERATURE},
    temperature_hundredths,
};

pub(crate) struct TemperatureH<'a>(pub(crate) &'a Ep);

impl temperature::ClusterHandler for TemperatureH<'_> {
    const CLUSTER: Cluster<'static> = TEMPERATURE;

    fn dataver(&self) -> u32 {
        self.0.datavers[0].get()
    }

    fn dataver_changed(&self) {
        self.0.datavers[0].changed();
    }

    fn measured_value(&self, _ctx: impl ReadContext) -> Result<Nullable<i16>, Error> {
        let value = self.0.number().and_then(|v| temperature_hundredths(v, self.0.entity.meta().unit));
        Ok(value.map_or(Nullable::none(), Nullable::some))
    }

    fn min_measured_value(&self, _ctx: impl ReadContext) -> Result<Nullable<i16>, Error> {
        Ok(Nullable::none())
    }

    fn max_measured_value(&self, _ctx: impl ReadContext) -> Result<Nullable<i16>, Error> {
        Ok(Nullable::none())
    }
}

pub(crate) struct BooleanH<'a>(pub(crate) &'a Ep);

impl boolean_state::ClusterHandler for BooleanH<'_> {
    const CLUSTER: Cluster<'static> = BOOLEAN;

    fn dataver(&self) -> u32 {
        self.0.datavers[0].get()
    }

    fn dataver_changed(&self) {
        self.0.datavers[0].changed();
    }

    /// Contact sensor: `true` is "closed" in Matter, "on" (open) in the model.
    fn state_value(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        Ok(!self.0.on())
    }
}

pub(crate) struct OnOffH<'a>(pub(crate) &'a Ep);

impl on_off::ClusterHandler for OnOffH<'_> {
    const CLUSTER: Cluster<'static> = ON_OFF;

    fn dataver(&self) -> u32 {
        self.0.datavers[0].get()
    }

    fn dataver_changed(&self) {
        self.0.datavers[0].changed();
    }

    fn on_off(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        Ok(self.0.on())
    }

    fn handle_off(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        self.0.set_on(false);
        Ok(())
    }

    fn handle_on(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        self.0.set_on(true);
        Ok(())
    }

    fn handle_toggle(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        self.0.set_on(!self.0.target_on());
        Ok(())
    }

    fn handle_off_with_effect(
        &self,
        _ctx: impl InvokeContext,
        _r: on_off::OffWithEffectRequest<'_>,
    ) -> Result<(), Error> {
        Err(ErrorCode::CommandNotFound.into())
    }

    fn handle_on_with_recall_global_scene(&self, _ctx: impl InvokeContext) -> Result<(), Error> {
        Err(ErrorCode::CommandNotFound.into())
    }

    fn handle_on_with_timed_off(
        &self,
        _ctx: impl InvokeContext,
        _r: on_off::OnWithTimedOffRequest<'_>,
    ) -> Result<(), Error> {
        Err(ErrorCode::CommandNotFound.into())
    }
}

pub(crate) struct LevelH<'a>(pub(crate) &'a Ep);

impl LevelH<'_> {
    fn level(&self) -> u8 {
        crate::level_from_brightness(self.0.light().brightness)
    }

    fn step(&self, up: bool, size: u8, with_on_off: bool) {
        // From the commanded level: steps in a row add up before the app applies them.
        let from = crate::level_from_brightness(self.0.target_light().brightness);
        let level = if up { from.saturating_add(size).min(254) } else { from.saturating_sub(size).max(1) };
        // "WithOnOff": going up switches on, reaching MinLevel going down switches off.
        self.0.set_level(level, with_on_off.then_some(up || level > 1));
    }
}

impl level_control::ClusterHandler for LevelH<'_> {
    const CLUSTER: Cluster<'static> = LEVEL;

    fn dataver(&self) -> u32 {
        self.0.datavers[1].get()
    }

    fn dataver_changed(&self) {
        self.0.datavers[1].changed();
    }

    fn current_level(&self, _ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(Nullable::some(self.level()))
    }

    fn min_level(&self, _ctx: impl ReadContext) -> Result<u8, Error> {
        Ok(1)
    }

    fn max_level(&self, _ctx: impl ReadContext) -> Result<u8, Error> {
        Ok(254)
    }

    fn options(&self, _ctx: impl ReadContext) -> Result<level_control::OptionsBitmap, Error> {
        Ok(level_control::OptionsBitmap::empty())
    }

    fn on_level(&self, _ctx: impl ReadContext) -> Result<Nullable<u8>, Error> {
        Ok(Nullable::none())
    }

    fn set_options(&self, _ctx: impl WriteContext, _value: level_control::OptionsBitmap) -> Result<(), Error> {
        Ok(())
    }

    fn set_on_level(&self, _ctx: impl WriteContext, _value: Nullable<u8>) -> Result<(), Error> {
        Ok(())
    }

    fn handle_move_to_level(
        &self,
        _ctx: impl InvokeContext,
        r: level_control::MoveToLevelRequest<'_>,
    ) -> Result<(), Error> {
        self.0.set_level(r.level()?, None);
        Ok(())
    }

    fn handle_move_to_level_with_on_off(
        &self,
        _ctx: impl InvokeContext,
        r: level_control::MoveToLevelWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        let level = r.level()?;
        self.0.set_level(level, Some(level > 0));
        Ok(())
    }

    // No transitions: "move" jumps to the end of the range.
    fn handle_move(&self, _ctx: impl InvokeContext, r: level_control::MoveRequest<'_>) -> Result<(), Error> {
        self.step(r.move_mode()? == level_control::MoveModeEnum::Up, 254, false);
        Ok(())
    }

    fn handle_move_with_on_off(
        &self,
        _ctx: impl InvokeContext,
        r: level_control::MoveWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        self.step(r.move_mode()? == level_control::MoveModeEnum::Up, 254, true);
        Ok(())
    }

    fn handle_step(&self, _ctx: impl InvokeContext, r: level_control::StepRequest<'_>) -> Result<(), Error> {
        self.step(r.step_mode()? == level_control::StepModeEnum::Up, r.step_size()?, false);
        Ok(())
    }

    fn handle_step_with_on_off(
        &self,
        _ctx: impl InvokeContext,
        r: level_control::StepWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        self.step(r.step_mode()? == level_control::StepModeEnum::Up, r.step_size()?, true);
        Ok(())
    }

    fn handle_stop(&self, _ctx: impl InvokeContext, _r: level_control::StopRequest<'_>) -> Result<(), Error> {
        Ok(())
    }

    fn handle_stop_with_on_off(
        &self,
        _ctx: impl InvokeContext,
        _r: level_control::StopWithOnOffRequest<'_>,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn handle_move_to_closest_frequency(
        &self,
        _ctx: impl InvokeContext,
        _r: level_control::MoveToClosestFrequencyRequest<'_>,
    ) -> Result<(), Error> {
        Err(ErrorCode::CommandNotFound.into())
    }
}

/// Drives a light from rs-matter's Color Control handler, which keeps the colour state and runs
/// transitions and the colour loop. Every colour mode is converted to RGB.
pub(crate) struct ColorHooks {
    ep: &'static Ep,
}

impl ColorControlHooks for ColorHooks {
    const CLUSTER: Cluster<'static> = COLOR;
    const COLOR_CAPABILITIES: ColorCapabilitiesBitmap = ColorCapabilitiesBitmap::from_bits_truncate(
        ColorCapabilitiesBitmap::HUE_SATURATION.bits()
            | ColorCapabilitiesBitmap::ENHANCED_HUE.bits()
            | ColorCapabilitiesBitmap::COLOR_LOOP.bits()
            | ColorCapabilitiesBitmap::XY.bits()
            | ColorCapabilitiesBitmap::COLOR_TEMPERATURE.bits(),
    );
    // 6500 K .. 2000 K.
    const COLOR_TEMP_PHYSICAL_MIN_MIREDS: u16 = 153;
    const COLOR_TEMP_PHYSICAL_MAX_MIREDS: u16 = 500;

    fn set_device_color(&self, target: SetDeviceColor) -> Result<(), ()> {
        // Linear: the LED mixes light additively. A colour command must not switch the light on.
        let (r, g, b) = target.to_rgb(RgbGamma::Linear);
        self.ep.entity.command(Command::Light {
            on: Some(self.ep.target_on()),
            brightness: None,
            rgb: Some([r, g, b]),
        });
        Ok(())
    }

    fn start_up_color_temperature_mireds(&self) -> Result<Nullable<u16>, Error> {
        Ok(Nullable::none())
    }

    fn set_start_up_color_temperature_mireds(&self, _value: Nullable<u16>) -> Result<(), Error> {
        Ok(())
    }
}

type ColorHandler = color::HandlerAsyncAdaptor<ColorControlHandler<'static, ColorHooks, NoOnOff, NoLevelControl>>;

/// One handler's `run` future.
type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Error>> + 'a>>;

/// The Color Control cluster of every light endpoint.
pub(crate) struct Colors(Vec<(u16, ColorHandler)>);

impl Colors {
    pub(crate) fn new(eps: &'static [Ep], rand: &mut impl Rng) -> Self {
        Self(
            eps.iter()
                .filter(|ep| ep.kind == MatterKind::ExtendedColorLight)
                .map(|ep| {
                    let hooks = ColorHooks { ep };
                    let h = ColorControlHandler::new_standalone(
                        Dataver::new_rand(rand),
                        ep.id,
                        hooks,
                        AttributeDefaults::new(),
                    );
                    (ep.id, h.adapt())
                })
                .collect(),
        )
    }

    fn get(&self, endpoint: u16) -> Result<&ColorHandler, Error> {
        self.0.iter().find(|(id, _)| *id == endpoint).map(|(_, h)| h).ok_or_else(|| ErrorCode::EndpointNotFound.into())
    }
}

impl AsyncHandler for Colors {
    fn read_awaits(&self, ctx: impl ReadContext) -> bool {
        self.get(ctx.attr().endpoint_id).is_ok_and(|h| h.read_awaits(ctx))
    }

    fn write_awaits(&self, ctx: impl WriteContext) -> bool {
        self.get(ctx.attr().endpoint_id).is_ok_and(|h| h.write_awaits(ctx))
    }

    fn invoke_awaits(&self, ctx: impl InvokeContext) -> bool {
        self.get(ctx.cmd().endpoint_id).is_ok_and(|h| h.invoke_awaits(ctx))
    }

    async fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> Result<(), Error> {
        self.get(ctx.attr().endpoint_id)?.read(ctx, reply).await
    }

    async fn write(&self, ctx: impl WriteContext) -> Result<(), Error> {
        self.get(ctx.attr().endpoint_id)?.write(ctx).await
    }

    async fn invoke(&self, ctx: impl InvokeContext, reply: impl InvokeReply) -> Result<(), Error> {
        self.get(ctx.cmd().endpoint_id)?.invoke(ctx, reply).await
    }

    fn bump_dataver(&self, ctx: impl MatchContext) {
        for (id, h) in &self.0 {
            if ctx.endpt().is_none_or(|e| e == *id) {
                h.bump_dataver(&ctx);
            }
        }
    }

    fn lifecycle(&self, ctx: impl HandlerContext, op: LifecycleOp) -> Result<(), Error> {
        for (_, h) in &self.0 {
            h.lifecycle(&ctx, op)?;
        }
        Ok(())
    }

    async fn run(&self, ctx: impl HandlerContext) -> Result<(), Error> {
        let ctx = &ctx;
        let mut tasks: Vec<RunFuture<'_>> = self.0.iter().map(|(_, h)| Box::pin(h.run(ctx)) as RunFuture<'_>).collect();
        if tasks.is_empty() {
            return core::future::pending().await;
        }
        select_slice(Pin::new(&mut tasks[..])).await.0
    }
}
