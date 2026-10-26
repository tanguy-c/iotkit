//! [`start`] over Thread (`matter-thread`): Matter on the 802.15.4 radio, paired over Bluetooth
//! LE, updates over Matter (`matter-ota`). No Wi-Fi, MQTT, console or setup windows.

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use esp_hal::{
    gpio::{AnyPin, Input, InputConfig, Pull},
    peripherals::{ADC1, BT, IEEE802154, RNG, TIMG1},
    timer::timg::TimerGroup,
};
use log::{error, info, warn};
use static_cell::StaticCell;

use super::{App, Config, spawn};
use crate::health::{HEALTH, MATTER, NETWORK, RECORDED, supervisor_task};
use crate::provision::gesture::{Gesture, PROVISION_MS};
use crate::{Device, SharedStore};

/// The peripherals the kit takes; the app keeps the rest.
pub struct Board {
    /// The 802.15.4 radio, for Thread.
    pub radio: IEEE802154<'static>,
    /// Bluetooth LE, for pairing.
    pub bt: BT<'static>,
    /// The random number generator.
    pub rng: RNG<'static>,
    /// The TRNG's entropy source (without Wi-Fi, it needs the ADC).
    pub adc1: ADC1<'static>,
    /// Its watchdog backs the task watchdog.
    pub timg1: TIMG1<'static>,
    /// The BOOT button (active low): held 3 s, a Matter pairing window; held 10 s, a factory
    /// reset ([`provision::gesture`](crate::provision::gesture)).
    pub boot_button: Option<AnyPin<'static>>,
}

/// Starts the kit over Thread (see the [crate] docs). Call first in `main`, right after
/// [`init!`](crate::init). Panics without it, or if the `config` partition is missing.
pub async fn start(spawner: Spawner, board: Board, config: &'static Config) -> App {
    // The crash record: a crash before this (in `init!`, say) does not overwrite it.
    let boot = crate::diag::esp::boot();
    esp_println::logger::init_logger_from_env();
    let fw = config.firmware;
    info!("{} {} booted, reset cause: {:?}", fw.product, fw.version, boot.cause);
    if let Some(crash) = &boot.crash {
        error!("previous run crashed: {crash}");
    }
    for &check in config.checks {
        HEALTH.require(check);
    }
    HEALTH.require(NETWORK);
    HEALTH.require(MATTER);

    static STORE: StaticCell<SharedStore> = StaticCell::new();
    let store = match crate::config::esp::open().await {
        Ok(store) => &*STORE.init(Mutex::new(store)),
        Err(e) => panic!("config: {e}"),
    };
    let stats = match crate::diag::record_boot(&mut *store.lock().await, &boot).await {
        Ok((stats, last)) => {
            info!("boot #{}, {} crashes ({} in a row)", stats.boots, stats.crashes, stats.crashes_in_a_row);
            if let Some(last) = last {
                info!("last crash: {last}");
            }
            HEALTH.pass(RECORDED);
            Some(stats)
        }
        Err(e) => {
            warn!("could not record the boot: {e}");
            None
        }
    };
    // After the boot is recorded (a crashing update still counts its crashes): may switch back
    // to the previous firmware and restart.
    #[cfg(esp_ota)]
    crate::ota::esp::boot(store, fw.version).await;

    spawn(spawner, crate::diag::esp::watchdog_task(TimerGroup::new(board.timg1).wdt, config.watches));
    // Before the app starts: its first command is the state saved before the restart.
    #[cfg(feature = "remember")]
    {
        let saved = crate::remember::esp::restore(store, config.entities).await;
        spawn(spawner, crate::remember::esp::remember_task(store, config.entities, saved));
    }

    static NAME: StaticCell<heapless::String<16>> = StaticCell::new();
    let mac: [u8; 6] = esp_hal::efuse::base_mac_address().as_bytes().try_into().unwrap_or_default();
    let name = NAME.init(crate::net::hostname(mac)).as_str();
    static DEVICE: StaticCell<Device> = StaticCell::new();
    let device = &*DEVICE.init(Device {
        name,
        manufacturer: config.manufacturer,
        model: config.model,
        sw_version: fw.version_text,
        entities: config.entities,
    });
    let radios =
        crate::matter::thread::Peripherals { radio: board.radio, bt: board.bt, rng: board.rng, adc1: board.adc1 };
    spawn(spawner, crate::matter::thread::matter_task(radios, store, config.product_name, fw, name, device));
    spawn(spawner, supervisor_task(store, fw.version));
    if let Some(pin) = board.boot_button {
        spawn(spawner, button_task(Input::new(pin, InputConfig::default().with_pull(Pull::Up)), store));
    }

    App { store, stats }
}

/// BOOT button (active low): a pairing window, or a factory reset that forgets every setting and
/// Matter pairing. A short press closes an open pairing window once paired; otherwise it is the
/// app's ([`wait_press`](crate::provision::gesture::wait_press)).
#[embassy_executor::task]
async fn button_task(mut button: Input<'static>, store: &'static SharedStore) {
    loop {
        match crate::provision::gesture::next(&mut button, None).await {
            Gesture::Press if crate::matter::common::pairing() => crate::matter::common::close_pairing_window(),
            Gesture::Press => crate::provision::gesture::pressed(),
            Gesture::Provision => {
                info!("BOOT held {} s: pairing window", PROVISION_MS / 1000);
                crate::matter::thread::open_pairing_window();
            }
            Gesture::FactoryReset => {
                warn!("BOOT held 10 s: erasing all settings");
                if let Err(e) = crate::config::esp::factory_reset(store).await {
                    error!("factory reset failed: {e}");
                }
                crate::diag::esp::restart().await;
            }
        }
    }
}
