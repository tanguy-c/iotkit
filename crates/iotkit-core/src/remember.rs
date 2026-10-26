//! Entities that keep their state across restarts (`remember` feature).
//!
//! A [`Switch`](crate::model::Switch), [`Light`](crate::model::Light),
//! [`Number`](crate::model::Number) or [`Select`](crate::model::Select) built with `.remember()`
//! has its state kept in one [`Remembered`] setting, and handed back at start as a command: the
//! app applies it like any other, so it needs no code of its own (it reports the state with `set`
//! as usual, and takes commands from the start).
//!
//! ```
//! use iotkit_core::model::{Entity, Light, LightState};
//! use iotkit_core::remember::{Remembered, Saved, key};
//!
//! static LED: Light = Light::new("led", "LED").remember();
//! static ENTITIES: &[&dyn Entity] = &[&LED];
//!
//! // What was saved before the restart:
//! let mut saved = Remembered::default();
//! saved.0.push((key("led"), Saved::Light { on: false, brightness: 20, rgb: [255, 0, 0] })).unwrap();
//! assert_eq!(saved.restore(ENTITIES), 1);
//! # embassy_futures::block_on(async {
//! let state = LED.wait_command().await; // the app's first command
//! assert_eq!(state, LightState { on: false, brightness: 20, rgb: [255, 0, 0] });
//! LED.set(state);
//! # });
//! assert!(!saved.update(ENTITIES)); // nothing new to save
//! ```
//!
//! The chip side (`iotkit`'s `remember` module) restores before the app starts, and saves a few
//! seconds after the last change, only when something differs: a slider dragged is one write.

use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::config::{Setting, keys};
use crate::model::{Command, Entity, State};

/// Most entities that remember: their states fit in one setting.
pub const MAX: usize = 16;

/// A state kept in flash.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Saved {
    /// Switch.
    Bool(bool),
    /// Light.
    Light {
        /// On or off.
        on: bool,
        /// 0-255, kept while off.
        brightness: u8,
        /// Red, green, blue.
        rgb: [u8; 3],
    },
    /// Number.
    Number(f32),
    /// Select: the index of the option.
    Option(u8),
}

impl Saved {
    /// What to keep of `state`; `None` while unknown, and for the kinds that do not remember.
    pub fn of(state: State) -> Option<Self> {
        match state {
            State::Bool(on) => Some(Self::Bool(on)),
            State::Light(l) => Some(Self::Light { on: l.on, brightness: l.brightness, rgb: l.rgb }),
            State::Number(v) if v.is_finite() => Some(Self::Number(v)),
            State::Option(i) => Some(Self::Option(i)),
            _ => None,
        }
    }

    /// The command that brings it back (the entity checks it as any other: an option no longer
    /// in the list, after an update, is dropped).
    pub fn command(self) -> Command {
        match self {
            Self::Bool(on) => Command::Set(on),
            Self::Light { on, brightness, rgb } => {
                Command::Light { on: Some(on), brightness: Some(brightness), rgb: Some(rgb) }
            }
            Self::Number(v) => Command::Number(v),
            Self::Option(i) => Command::Option(i),
        }
    }
}

/// An entity's key in [`Remembered`]: the FNV-1a hash of its id, so renaming or reordering the
/// other entities does not mix states up.
pub const fn key(id: &str) -> u32 {
    let bytes = id.as_bytes();
    let mut hash = 0x811c_9dc5_u32;
    let mut i = 0;
    while i < bytes.len() {
        hash = (hash ^ bytes[i] as u32).wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

/// The states of the entities that remember theirs, by [`key`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Remembered(pub Vec<(u32, Saved), MAX>);

impl Setting for Remembered {
    const KEY: u16 = keys::REMEMBERED;
    const VERSION: u8 = 1;
}

impl Remembered {
    /// Hands each entity that remembers its saved state, as a command. Returns how many.
    pub fn restore(&self, entities: &[&dyn Entity]) -> usize {
        remembering(entities)
            .filter_map(|e| self.get(key(e.meta().id)).map(|s| e.command(s.command())))
            .filter(|&taken| taken)
            .count()
    }

    /// Takes the current states of the entities that remember; `true` if anything changed. An
    /// unknown state (not set yet) keeps what was saved; entities that no longer remember (or
    /// no longer exist) are forgotten. Past [`MAX`] entities, the others are not kept.
    pub fn update(&mut self, entities: &[&dyn Entity]) -> bool {
        let before = self.0.len();
        self.0.retain(|(k, _)| remembering(entities).any(|e| key(e.meta().id) == *k));
        let mut changed = self.0.len() != before;
        for e in remembering(entities) {
            let Some(state) = Saved::of(e.state()) else { continue };
            let k = key(e.meta().id);
            match self.0.iter_mut().find(|(key, _)| *key == k) {
                Some((_, saved)) if *saved == state => {}
                Some((_, saved)) => {
                    *saved = state;
                    changed = true;
                }
                None => match self.0.push((k, state)) {
                    Ok(()) => changed = true,
                    Err(_) => log::warn!("remember: more than {MAX} entities, '{}' is not kept", e.meta().id),
                },
            }
        }
        changed
    }

    fn get(&self, k: u32) -> Option<Saved> {
        self.0.iter().find(|(key, _)| *key == k).map(|&(_, s)| s)
    }
}

/// The entities built with `.remember()`.
pub fn remembering<'a>(entities: &'a [&'a dyn Entity]) -> impl Iterator<Item = &'a dyn Entity> + 'a {
    entities.iter().copied().filter(|e| e.meta().remember)
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;

    use super::*;
    use crate::config::MAX_VALUE_LEN;
    use crate::model::{Light, LightState, Number, Select, Switch};

    #[test]
    fn restores_as_commands() {
        static RELAY: Switch = Switch::new("relay", "Relay").remember();
        static LAMP: Light = Light::new("lamp", "Lamp").remember();
        static SETPOINT: Number = Number::new("setpoint", "Setpoint", 0.0, 30.0, 0.5).remember();
        static MODE: Select = Select::new("mode", "Mode", &["eco", "comfort"]).remember();
        static OTHER: Switch = Switch::new("other", "Other");
        static ENTITIES: [&dyn Entity; 5] = [&RELAY, &LAMP, &SETPOINT, &MODE, &OTHER];

        let mut saved = Remembered::default();
        for (id, s) in [
            ("relay", Saved::Bool(true)),
            ("lamp", Saved::Light { on: false, brightness: 20, rgb: [0, 128, 255] }),
            ("setpoint", Saved::Number(21.5)),
            ("mode", Saved::Option(1)),
            ("other", Saved::Bool(true)),
        ] {
            saved.0.push((key(id), s)).unwrap();
        }
        assert_eq!(saved.restore(&ENTITIES), 4, "not the entity that does not remember");
        block_on(async {
            assert!(RELAY.wait_command().await);
            // Off, with its brightness and colour kept for the next "on".
            assert_eq!(LAMP.wait_command().await, LightState { on: false, brightness: 20, rgb: [0, 128, 255] });
            assert_eq!(SETPOINT.wait_command().await, 21.5);
            assert_eq!(MODE.wait_command().await, 1);
        });
        assert_eq!(OTHER.target(), State::Unknown);
    }

    #[test]
    fn a_stale_option_is_dropped() {
        static MODE: Select = Select::new("mode", "Mode", &["eco"]).remember();
        let mut saved = Remembered::default();
        saved.0.push((key("mode"), Saved::Option(3))).unwrap();
        assert_eq!(saved.restore(&[&MODE]), 0);
    }

    #[test]
    fn updates_only_what_changed() {
        static RELAY: Switch = Switch::new("relay", "Relay").remember();
        static LAMP: Light = Light::new("lamp", "Lamp").remember();
        static OTHER: Switch = Switch::new("other", "Other");
        static ENTITIES: [&dyn Entity; 3] = [&RELAY, &LAMP, &OTHER];

        let mut saved = Remembered::default();
        assert!(!saved.update(&ENTITIES), "nothing set yet");
        RELAY.set(true);
        OTHER.set(true);
        assert!(saved.update(&ENTITIES));
        assert_eq!(saved.0.as_slice(), &[(key("relay"), Saved::Bool(true))]);
        assert!(!saved.update(&ENTITIES));
        RELAY.set(false);
        LAMP.set(LightState { on: true, brightness: 255, rgb: [255, 255, 255] });
        assert!(saved.update(&ENTITIES));
        assert_eq!(saved.get(key("relay")), Some(Saved::Bool(false)));
        assert_eq!(saved.0.len(), 2);
    }

    #[test]
    fn forgets_entities_gone() {
        static RELAY: Switch = Switch::new("relay", "Relay").remember();
        let mut saved = Remembered::default();
        saved.0.push((key("renamed"), Saved::Bool(true))).unwrap();
        saved.0.push((key("relay"), Saved::Bool(true))).unwrap();
        assert!(saved.update(&[&RELAY]), "the stale entry goes");
        assert_eq!(saved.0.as_slice(), &[(key("relay"), Saved::Bool(true))]);
    }

    #[test]
    fn a_full_record_fits_in_a_setting() {
        let mut saved = Remembered::default();
        for i in 0..MAX as u32 {
            saved.0.push((u32::MAX - i, Saved::Light { on: true, brightness: 255, rgb: [255; 3] })).unwrap();
        }
        let mut buf = [0; MAX_VALUE_LEN];
        let encoded = postcard::to_slice(&saved, &mut buf[1..]).unwrap().len();
        assert!(encoded < MAX_VALUE_LEN, "{encoded} bytes");
        assert_eq!(postcard::from_bytes::<Remembered>(&buf[1..=encoded]).unwrap(), saved);
    }

    #[test]
    fn keys_are_stable() {
        // Saved on devices: changing the hash would forget every state.
        assert_eq!(key(""), 0x811c_9dc5);
        assert_eq!(key("a"), 0xe40c_292c);
        assert_ne!(key("relay"), key("relay_2"));
    }
}
