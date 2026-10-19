//! The firmware as an entity: Home Assistant shows the installed and latest versions and installs
//! the update.

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};
use heapless::String;

use crate::{Binding, Command, Dirty, Entity, Kind, Meta, State, entities::builders};

/// What an update entity shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub installed: String<16>,
    /// Empty while unknown (never checked).
    pub latest: String<16>,
    pub summary: String<64>,
    pub release_url: String<96>,
    pub in_progress: bool,
    /// While installing, if known.
    pub percentage: Option<u8>,
}

/// The device's firmware update. The kit checks for releases and installs them; the app only
/// declares it (see `iotkit-app`'s `ha-update` feature).
pub struct Update {
    meta: Meta,
    state: Mutex<CriticalSectionRawMutex, RefCell<UpdateState>>,
    dirty: Dirty,
    install: Signal<CriticalSectionRawMutex, ()>,
}

builders!(Update);

/// `s` cut to `N` bytes, on a character boundary.
fn cut<const N: usize>(s: &str) -> String<N> {
    let mut end = s.len().min(N);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    String::try_from(&s[..end]).unwrap_or_default()
}

impl Update {
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            meta: Meta::new(Kind::Update, id, name),
            state: Mutex::new(RefCell::new(UpdateState {
                installed: String::new(),
                latest: String::new(),
                summary: String::new(),
                release_url: String::new(),
                in_progress: false,
                percentage: None,
            })),
            dirty: Dirty::new(),
            install: Signal::new(),
        }
    }

    fn change(&self, f: impl FnOnce(&mut UpdateState)) {
        let changed = self.state.lock(|s| {
            let mut s = s.borrow_mut();
            let before = s.clone();
            f(&mut s);
            *s != before
        });
        if changed {
            self.dirty.mark();
        }
    }

    pub fn set_installed(&self, version: &str) {
        self.change(|s| s.installed = cut(version));
    }

    /// The latest release: its version, what it changes and where its notes are (empty: none).
    pub fn set_latest(&self, version: &str, summary: &str, release_url: &str) {
        self.change(|s| {
            s.latest = cut(version);
            s.summary = cut(summary);
            s.release_url = cut(release_url);
        });
    }

    /// Installing (with a percentage if known), or not.
    pub fn set_progress(&self, in_progress: bool, percentage: Option<u8>) {
        self.change(|s| {
            s.in_progress = in_progress;
            s.percentage = percentage.map(|p| p.min(100));
        });
    }

    /// Waits until the controller asks to install the latest version.
    pub async fn wait_install(&self) {
        self.install.wait().await
    }
}

impl Entity for Update {
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn state(&self) -> State {
        if self.state.lock(|s| s.borrow().installed.is_empty()) { State::Unknown } else { State::Update }
    }
    fn update(&self) -> Option<UpdateState> {
        Some(self.state.lock(|s| s.borrow().clone()))
    }
    fn command(&self, command: Command) -> bool {
        if command == Command::Install {
            self.install.signal(());
            true
        } else {
            false
        }
    }
    fn take_dirty(&self, binding: Binding) -> bool {
        self.dirty.take(binding)
    }
    fn mark_dirty(&self, binding: Binding) {
        self.dirty.mark_for(binding)
    }
}
