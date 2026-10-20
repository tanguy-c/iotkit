//! BOOT button gestures: hold 3 s to open provisioning, 10 s to factory-reset.

/// Hold at least this long (ms) to open provisioning.
pub const PROVISION_MS: u32 = 3_000;
/// Hold at least this long (ms) to factory-reset.
pub const FACTORY_RESET_MS: u32 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// What a press of the BOOT button asks for.
pub enum Gesture {
    /// Shorter than [`PROVISION_MS`]: left to the app.
    Press,
    /// Open provisioning (held [`PROVISION_MS`]).
    Provision,
    /// Erase the settings and restart (held [`FACTORY_RESET_MS`]).
    FactoryReset,
}

/// The gesture of a press held `held_ms`, decided at release.
pub fn on_release(held_ms: u32) -> Gesture {
    match held_ms {
        FACTORY_RESET_MS.. => Gesture::FactoryReset,
        PROVISION_MS.. => Gesture::Provision,
        _ => Gesture::Press,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        assert_eq!(on_release(100), Gesture::Press);
        assert_eq!(on_release(2_999), Gesture::Press);
        assert_eq!(on_release(3_000), Gesture::Provision);
        assert_eq!(on_release(9_999), Gesture::Provision);
        assert_eq!(on_release(10_000), Gesture::FactoryReset);
        assert_eq!(on_release(60_000), Gesture::FactoryReset);
    }
}
