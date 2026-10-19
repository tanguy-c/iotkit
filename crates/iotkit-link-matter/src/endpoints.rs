//! The endpoints: one per entity with a Matter equivalent, built from the device at startup.

use alloc::{boxed::Box, vec::Vec};

use iotkit_model::{Command, Device, Entity, LightState, State};
use rs_matter_embassy::matter::{
    crypto::Rng,
    dm::{
        Cluster, Dataver, DeviceType, Endpoint, Node,
        clusters::{
            app::color_control as color,
            decl::{boolean_state, level_control, on_off, temperature_measurement as temperature},
            desc::{self, ClusterHandler as _},
        },
    },
    with,
};

use crate::{MatterKind, brightness_from_level, matter_kind};

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

const fn dt(dtype: u16, drev: u16) -> DeviceType {
    DeviceType { dtype, drev }
}

type Layout = (&'static [DeviceType], &'static [Cluster<'static>]);

fn layout(kind: MatterKind) -> Layout {
    const COLOR_LIGHT: Layout = (&[dt(0x010D, 4)], &[DESC, ON_OFF, LEVEL, COLOR]);
    const PLUG: Layout = (&[dt(0x010A, 3)], &[DESC, ON_OFF]);
    const TEMPERATURE_SENSOR: Layout = (&[dt(0x0302, 2)], &[DESC, TEMPERATURE]);
    const CONTACT_SENSOR: Layout = (&[dt(0x0015, 1)], &[DESC, BOOLEAN]);
    match kind {
        MatterKind::ExtendedColorLight => COLOR_LIGHT,
        MatterKind::OnOffPlug => PLUG,
        MatterKind::TemperatureSensor => TEMPERATURE_SENSOR,
        MatterKind::ContactSensor => CONTACT_SENSOR,
    }
}

/// An entity endpoint.
pub(crate) struct Ep {
    pub(crate) id: u16,
    pub(crate) entity: &'static dyn Entity,
    pub(crate) kind: MatterKind,
    /// Data versions of the main cluster and of Level Control (lights).
    pub(crate) datavers: [Dataver; 2],
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
}

/// The node (root endpoint first, then one endpoint per mapped entity) and the entity endpoints.
pub(crate) fn build(device: &Device, root: Endpoint<'static>, rand: &mut impl Rng) -> (Node<'static>, &'static [Ep]) {
    let mut endpoints = Vec::from([root]);
    let mut eps = Vec::new();
    for entity in device.entities {
        let Some(kind) = matter_kind(entity.meta()) else { continue };
        let id = eps.len() as u16 + 1;
        let (device_types, clusters) = layout(kind);
        endpoints.push(Endpoint::new(id, device_types, clusters));
        eps.push(Ep { id, entity: *entity, kind, datavers: [Dataver::new_rand(rand), Dataver::new_rand(rand)] });
        log::info!("matter: endpoint {id}: '{}' as {kind:?}", entity.meta().id);
    }
    (Node { endpoints: Box::leak(endpoints.into_boxed_slice()) }, Box::leak(eps.into_boxed_slice()))
}
