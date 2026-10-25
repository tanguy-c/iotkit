//! The kit's health check: what must work before a boot counts as stable and an update on trial
//! is kept. Each transport requires its checks when it starts:
//!
//! | Network         | [`NETWORK`]                       | [`BROKER`]           | [`MATTER`]               | [`ESPHOME`]         |
//! |-----------------|-----------------------------------|----------------------|--------------------------|---------------------|
//! | Wi-Fi           | joined, with an address           | MQTT broker, if set  | with `matter-wifi`       | with `esphome-api`  |
//! | Thread (Matter) | attached (child, router, leader)  | -                    | always                   | -                   |
//!
//! [`MATTER`] passes once a controller talks to the device over an operational (CASE) session,
//! and at once while it has no fabric. [`ESPHOME`] passes once Home Assistant has connected and
//! got the states, and at once until it has connected once (a device not added yet). An app adds its own ([`APP_CHECKS`] on).

#[cfg(esp_ota)]
use embassy_futures::select::{Either3, select3};
#[cfg(esp_ota)]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
#[cfg(esp_ota)]
use embassy_time::Timer;
use log::{info, warn};

use crate::image::Version;
use crate::{BootStats, Check, Health, SharedStore};

/// The boot was counted in flash.
pub const RECORDED: Check = Check(0);
/// The network is up: Wi-Fi joined with an address, or Thread attached (passed at once while
/// none is stored).
pub const NETWORK: Check = Check(1);
/// Connected to the MQTT broker (passed at once when none is set).
pub const BROKER: Check = Check(2);
/// A Matter controller reached the device (passed at once while it has no fabric).
pub const MATTER: Check = Check(3);
/// Home Assistant connected over the ESPHome API (passed at once until it has once).
pub const ESPHOME: Check = Check(4);
/// First check number free for the app's own (`Config::checks`).
pub const APP_CHECKS: u8 = 8;

/// The health check: [`RECORDED`], the checks of the network in use, and the app's.
pub static HEALTH: Health = Health::observed(&[RECORDED], crate::status::notify);

/// The radio that carries the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Radio {
    /// Wi-Fi (`net::esp::start`).
    WiFi,
    /// Thread, started by the Matter stack.
    Thread,
}

/// The radio failed during an update's trial: [`supervisor_task`] gives it up at once.
#[cfg(esp_ota)]
static RADIO_FAILED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The radio did not start (or the network stack on it stopped): its checks cannot pass, so they
/// are skipped. Not for an update on trial: the previous firmware had a radio (it received the
/// update), and this one, kept, could not be updated again but over USB.
pub fn radio_failed(radio: Radio) {
    #[cfg(esp_ota)]
    if crate::ota::esp::on_trial() {
        let name = match radio {
            Radio::WiFi => "Wi-Fi",
            Radio::Thread => "Thread",
        };
        log::error!("ota: the update cannot start {name}, back to the previous firmware");
        RADIO_FAILED.signal(());
        return;
    }
    #[cfg(not(esp_ota))]
    let _ = radio;
    crate::status::set_offline(true);
    for check in [NETWORK, BROKER, MATTER, ESPHOME] {
        HEALTH.pass(check);
    }
}

/// Once healthy: the boot is stable, and an update just installed is kept (rolled back if not
/// healthy in time, or at once if its radio fails). `version`: this firmware's.
#[cfg_attr(not(esp_ota), allow(unused_variables))]
#[embassy_executor::task]
pub async fn supervisor_task(store: &'static SharedStore, version: Version) {
    #[cfg(esp_ota)]
    if crate::ota::esp::on_trial() {
        let timeout = crate::ota::UPDATE_TIMEOUT_S;
        match select3(HEALTH.healthy(), Timer::after_secs(timeout.into()), RADIO_FAILED.wait()).await {
            Either3::First(()) => {}
            Either3::Second(()) => crate::ota::esp::give_up(store, HEALTH.missing(), version, timeout).await,
            Either3::Third(()) => crate::ota::esp::give_up(store, HEALTH.missing(), version, 0).await,
        }
    }
    HEALTH.healthy().await;
    #[cfg(esp_ota)]
    crate::ota::esp::confirm(store).await;
    match BootStats::mark_stable(&mut *store.lock().await).await {
        Ok(()) => info!("healthy"),
        Err(e) => warn!("could not mark the boot stable: {e}"),
    }
}
