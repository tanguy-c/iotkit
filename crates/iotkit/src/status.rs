//! What the device is doing, for a LED: [`watch`] follows it. Two families ([`Family`]):
//! *pairing* (waiting to be set up, the button held) and *diagnostic* (connecting, online,
//! offline, updating). A LED shows one, or both ([`Show`]), pairing first. The kit's
//! [`status_led`](crate::components::status_led) and [`status_rgb`](crate::components::status_rgb)
//! show it; an app can show it its own way.
//!
//! ```ignore
//! #[embassy_executor::task]
//! async fn ui(mut led: Output<'static>) -> ! {
//!     let mut status = iotkit::status::watch(Show::Pairing);
//!     loop {
//!         match status.next().await {
//!             Some(Status::Pairing) => led.set_high(), // or blink: `next` is cancel-safe
//!             _ => led.set_low(),                       // `None`: nothing of the family now
//!         }
//!     }
//! }
//! ```

#[cfg(feature = "_esp")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};

/// What the device is doing, the most pressing first in each [`Family`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// Pairing: the button is held past a threshold; released now, it does this.
    Held(Hold),
    /// Pairing: waiting to be set up, a setup or pairing window is open, or a deep-sleeping
    /// device is in pairing mode.
    Pairing,
    /// Diagnostic: an update downloads or installs.
    Installing,
    /// Diagnostic: the radio failed, or a deep-sleeping device found no link in time.
    Offline,
    /// Diagnostic: an update runs on trial, until healthy.
    Trial,
    /// Diagnostic: the network and links are up.
    Online,
    /// Diagnostic: joining the network, reaching the broker or a controller.
    Connecting,
}

/// The two kinds of [`Status`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Setting the device up: pairing windows, the button's gestures.
    Pairing,
    /// How it runs: its links and updates.
    Diagnostic,
}

impl Status {
    /// Its family.
    pub const fn family(self) -> Family {
        match self {
            Status::Held(_) | Status::Pairing => Family::Pairing,
            _ => Family::Diagnostic,
        }
    }
}

/// What a LED shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// The pairing family only: dark otherwise (on battery, no blinking at each wake-up).
    Pairing,
    /// The diagnostic family only.
    Diagnostic,
    /// Both, pairing first.
    All,
}

/// The button threshold reached ([`provision::gesture`](crate::provision::gesture)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// Released now: the setup (or pairing) window opens.
    Pairing,
    /// Released now: every setting is erased.
    FactoryReset,
}

/// What [`Status`] is made of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// The button threshold reached: 0 none, 1 pairing, 2 factory reset.
    pub held: u8,
    /// An update is being written.
    pub installing: bool,
    /// A setup or pairing window is open, or a deep-sleeping device is in pairing mode.
    pub pairing: bool,
    /// The radio failed, or no link in time.
    pub offline: bool,
    /// An update is on trial.
    pub trial: bool,
    /// The network's health checks passed.
    pub online: bool,
}

/// The status of `i` that `show` shows (`None`: nothing of its family now).
pub const fn of(i: Inputs, show: Show) -> Option<Status> {
    let pairing = match i {
        Inputs { held: 2.., .. } => Some(Status::Held(Hold::FactoryReset)),
        Inputs { held: 1, .. } => Some(Status::Held(Hold::Pairing)),
        Inputs { pairing: true, .. } => Some(Status::Pairing),
        _ => None,
    };
    let diagnostic = match i {
        Inputs { installing: true, .. } => Status::Installing,
        Inputs { offline: true, .. } => Status::Offline,
        Inputs { trial: true, .. } => Status::Trial,
        Inputs { online: true, .. } => Status::Online,
        _ => Status::Connecting,
    };
    match (show, pairing) {
        (Show::Pairing, p) | (Show::All, p @ Some(_)) => p,
        (Show::Diagnostic | Show::All, _) => Some(diagnostic),
    }
}

/// Watchers at most.
#[cfg(feature = "_esp")]
const WATCHERS: usize = 4;

#[cfg(feature = "_esp")]
static CHANGED: Watch<CriticalSectionRawMutex, (), WATCHERS> = Watch::new();

/// One of the inputs may have changed: watchers look again.
#[cfg(feature = "_esp")]
pub(crate) fn notify() {
    CHANGED.sender().send(());
}

#[cfg(feature = "_esp")]
static OFFLINE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// A link whose state is followed live: once it has come up, the status is
/// [`Connecting`](Status::Connecting) again while it is down. Matter's is not followed yet (its
/// health check, passed once, stands for it).
#[cfg(esp_wifi)]
#[derive(Clone, Copy)]
pub(crate) enum Link {
    /// Wi-Fi joined, with an IPv4 address.
    Network = 0,
    /// Connected to the MQTT broker.
    #[cfg(feature = "mqtt")]
    Broker = 1,
    /// Home Assistant subscribed over the ESPHome API.
    #[cfg(feature = "esphome-api")]
    Esphome = 2,
}

/// Holders of each [`Link`] now (sessions: the ESPHome API serves two).
#[cfg(esp_wifi)]
static UP: [crate::atomic::AtomicU8; 3] =
    [crate::atomic::AtomicU8::new(0), crate::atomic::AtomicU8::new(0), crate::atomic::AtomicU8::new(0)];
/// The [`Link`]s that have come up once (bit per link).
#[cfg(esp_wifi)]
static SEEN: crate::atomic::AtomicU8 = crate::atomic::AtomicU8::new(0);

/// A [`Link`] up while it lives: dropped with the session or connection, on any path.
#[cfg(esp_wifi)]
pub(crate) struct LinkUp(Link);

#[cfg(esp_wifi)]
impl LinkUp {
    pub(crate) fn new(link: Link) -> Self {
        use crate::atomic::Ordering::Relaxed;
        UP[link as usize].fetch_add(1, Relaxed);
        SEEN.fetch_or(1 << link as u8, Relaxed);
        notify();
        Self(link)
    }
}

#[cfg(esp_wifi)]
impl Drop for LinkUp {
    fn drop(&mut self) {
        UP[self.0 as usize].fetch_sub(1, crate::atomic::Ordering::Relaxed);
        notify();
    }
}

/// A link that came up once is down now.
#[cfg(esp_wifi)]
fn link_down() -> bool {
    use crate::atomic::Ordering::Relaxed;
    let seen = SEEN.load(Relaxed);
    (0..UP.len()).any(|i| seen & 1 << i != 0 && UP[i].load(Relaxed) == 0)
}

/// The radio failed, or a deep-sleeping device found no link in time (or found it again).
#[cfg(feature = "_esp")]
pub(crate) fn set_offline(offline: bool) {
    OFFLINE.store(offline, core::sync::atomic::Ordering::Relaxed);
    notify();
}

/// The status now, as `show` shows it.
#[cfg(feature = "_esp")]
pub fn current(show: Show) -> Option<Status> {
    use crate::health::{BROKER, ESPHOME, HEALTH, MATTER, NETWORK};
    let links = [NETWORK, BROKER, MATTER, ESPHOME].iter().fold(0, |m, c| m | 1 << c.0);
    #[allow(unused_mut)]
    let mut i = Inputs {
        held: crate::provision::gesture::HELD.load(crate::atomic::Ordering::Relaxed),
        offline: OFFLINE.load(core::sync::atomic::Ordering::Relaxed),
        online: HEALTH.missing() & links == 0,
        ..Inputs::default()
    };
    // The checks pass once, for the update's trial: a link down now is not online.
    #[cfg(esp_wifi)]
    {
        i.online &= !link_down();
    }
    #[cfg(esp_ota)]
    {
        i.installing = crate::ota::esp::writing();
        i.trial = crate::ota::esp::on_trial();
    }
    #[cfg(esp_wifi)]
    {
        i.pairing |= crate::provision::esp::window_open();
    }
    #[cfg(all(esp_wifi, feature = "deep-sleep"))]
    {
        i.pairing |= crate::app::sleep::pairing();
    }
    #[cfg(feature = "_matter")]
    {
        i.pairing |= crate::matter::common::pairing();
    }
    of(i, show)
}

/// Follows [`current`]: from [`watch`].
#[cfg(feature = "_esp")]
pub struct Watcher {
    changed: embassy_sync::watch::DynReceiver<'static, ()>,
    show: Show,
    last: Option<Option<Status>>,
}

/// Follows the status, as `show` shows it. At most 4 watchers (the kit's LED helpers each take
/// one): panics past that.
#[cfg(feature = "_esp")]
pub fn watch(show: Show) -> Watcher {
    let changed = CHANGED.dyn_receiver().expect("iotkit::status: at most 4 watchers");
    Watcher { changed, show, last: None }
}

#[cfg(feature = "_esp")]
impl Watcher {
    /// The status: at once the first time, then when it changes (`None`: nothing of the family
    /// shown). Cancel-safe.
    pub async fn next(&mut self) -> Option<Status> {
        loop {
            let now = current(self.show);
            if self.last != Some(now) {
                self.last = Some(now);
                return now;
            }
            self.changed.changed().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        let all = Inputs { held: 1, installing: true, pairing: true, offline: true, trial: true, online: true };
        assert_eq!(of(all, Show::All), Some(Status::Held(Hold::Pairing)), "pairing first");
        assert_eq!(of(Inputs { held: 2, ..all }, Show::Pairing), Some(Status::Held(Hold::FactoryReset)));
        assert_eq!(of(Inputs { held: 0, ..all }, Show::Pairing), Some(Status::Pairing));
        assert_eq!(of(all, Show::Diagnostic), Some(Status::Installing), "the pairing family hidden");
        let diagnostic = Inputs { held: 0, pairing: false, ..all };
        assert_eq!(of(diagnostic, Show::Pairing), None, "dark: nothing to pair");
        assert_eq!(of(diagnostic, Show::All), Some(Status::Installing));
        let diagnostic = Inputs { installing: false, ..diagnostic };
        assert_eq!(of(diagnostic, Show::All), Some(Status::Offline));
        assert_eq!(of(Inputs { offline: false, ..diagnostic }, Show::All), Some(Status::Trial));
        assert_eq!(of(Inputs { online: true, ..Inputs::default() }, Show::Diagnostic), Some(Status::Online));
        assert_eq!(of(Inputs::default(), Show::All), Some(Status::Connecting));
        for s in [Status::Held(Hold::FactoryReset), Status::Pairing] {
            assert_eq!(s.family(), Family::Pairing);
        }
        for s in [Status::Installing, Status::Trial, Status::Offline, Status::Online, Status::Connecting] {
            assert_eq!(s.family(), Family::Diagnostic);
        }
    }
}
