//! A device's firmware in one call: [`start`] runs every iotkit service in the order they need,
//! and the app keeps only its own hardware and entities.
//!
//! ```ignore
//! static OTA_KEY: PublicKey = iotkit_app::public_key(include_str!("../keys/ota.pub"));
//! static LED: Light = Light::new("led", "LED");
//! static ENTITIES: [&dyn Entity; 1] = [&LED];
//! static CONFIG: Config = Config {
//!     firmware: iotkit_app::firmware!(&OTA_KEY),
//!     manufacturer: "Me", model: "Desk lamp", chip: "ESP32-C6", product_name: "Desk lamp",
//!     entities: &ENTITIES, watches: &[], checks: &[], console: None,
//! };
//!
//! let app = iotkit_app::start(spawner, Board { flash: p.FLASH, wifi: p.WIFI, .. }, &CONFIG).await;
//! ```
//!
//! What [`start`] does, in order: takes the crash record and counts the boot; guards and handles
//! a pending update (trial boots, rollback); starts the watchdog; joins Wi-Fi (or opens the setup
//! window: access point, Improv over Bluetooth and serial); answers `<host>.local`; starts MQTT
//! (Home Assistant discovery) when a broker is set, and Matter with the `matter-wifi` feature;
//! serves OTA pushes; watches the BOOT button; runs the USB console. Once every health check
//! passes (the kit's and [`Config::checks`]), the boot is stable and an update is confirmed.

#![no_std]

#[cfg(not(any(feature = "esp32c6", feature = "esp32c3", feature = "esp32c5", feature = "esp32s3")))]
compile_error!("iotkit-app: enable one chip feature: esp32c6, esp32c3, esp32c5 or esp32s3");
#[cfg(any(
    all(feature = "esp32c6", any(feature = "esp32c3", feature = "esp32c5", feature = "esp32s3")),
    all(feature = "esp32c3", any(feature = "esp32c5", feature = "esp32s3")),
    all(feature = "esp32c5", feature = "esp32s3"),
))]
compile_error!("iotkit-app: enable only one chip feature");
#[cfg(all(feature = "esp32c5", feature = "matter-wifi"))]
compile_error!("iotkit-app: `matter-wifi` is not supported on the ESP32-C5 yet (no TRNG in esp-hal 1.2)");
// Matter does not sleep (an always-on controller session).
#[cfg(all(feature = "deep-sleep", feature = "matter-wifi"))]
compile_error!("iotkit-app: `deep-sleep` is for MQTT devices; not with `matter-wifi`");
// Matter over Wi-Fi needs about 21 KB more internal RAM than the ESP32-S3 has left (PSRAM
// support would fix it; not done yet).
#[cfg(all(feature = "esp32s3", feature = "matter-wifi"))]
compile_error!("iotkit-app: `matter-wifi` does not fit in the ESP32-S3's internal RAM yet (use MQTT, or a C6)");
// Matter over Wi-Fi needs about 24 KB more static RAM than the ESP32-C3 has.
#[cfg(all(feature = "esp32c3", feature = "matter-wifi"))]
compile_error!("iotkit-app: `matter-wifi` does not fit in the ESP32-C3's RAM (use MQTT, or a C6/C5/S3)");

pub mod battery;
mod console;
#[cfg(feature = "deep-sleep")]
mod sleep;
#[cfg(feature = "ha-update")]
mod update;

#[cfg(feature = "deep-sleep")]
pub use sleep::{SleepConfig, SleepyApp, WakeReason, start_sleepy, wake_reason};

#[cfg(feature = "ha-update")]
extern crate alloc;

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant};
use esp_hal::{
    gpio::{AnyPin, Input, InputConfig, Pull},
    peripherals::{BT, FLASH, TIMG1, USB_DEVICE, WIFI},
    timer::timg::TimerGroup,
    usb::usb_serial_jtag::UsbSerialJtag,
};
use log::{error, info, warn};
use static_cell::StaticCell;

pub use console::ConsoleExtension;
pub use iotkit_config::{self as config, Setting, esp::SharedStore};
pub use iotkit_diag::{self as diag, Boot, BootStats, TaskWatch};
pub use iotkit_image::{self as image, PublicKey, public_key};
#[cfg(feature = "matter-wifi")]
pub use iotkit_link_matter as matter;
pub use iotkit_link_mqtt as mqtt;
pub use iotkit_model::{self as model, *};
pub use iotkit_net::{self as net, esp::Net};
pub use iotkit_ota::{self as ota, esp::Firmware, firmware};
pub use iotkit_provision as provision;
pub use iotkit_runtime::{self as runtime, Check, Health};

/// The boot was counted in flash.
pub const RECORDED: Check = Check(0);
/// Wi-Fi joined, with an address (passed at once while no network is stored).
pub const NETWORK: Check = Check(1);
/// Connected to the MQTT broker (passed at once when none is set).
pub const BROKER: Check = Check(2);
/// First check number free for the app's own ([`Config::checks`]).
pub const APP_CHECKS: u8 = 8;

/// The health check: the kit's checks, and the app's added by [`start`].
pub static HEALTH: Health = Health::new(&[RECORDED, NETWORK, BROKER]);

/// The peripherals the kit takes; the app keeps the rest.
pub struct Board {
    pub flash: FLASH<'static>,
    pub wifi: WIFI<'static>,
    pub bt: BT<'static>,
    /// Its watchdog backs the task watchdog.
    pub timg1: TIMG1<'static>,
    /// The USB serial port: console and Improv.
    pub usb: USB_DEVICE<'static>,
    /// The BOOT button (active low; GPIO9 on the ESP32-C6-DevKitC-1): hold 3 s to open the
    /// setup window, 10 s to factory-reset; a short press goes to the app
    /// ([`provision::esp::wait_press`]).
    pub boot_button: Option<AnyPin<'static>>,
}

/// What the device is.
pub struct Config {
    /// `iotkit_app::firmware!(&OTA_KEY)`: package name, version, update key.
    pub firmware: Firmware,
    /// Shown in Home Assistant.
    pub manufacturer: &'static str,
    pub model: &'static str,
    /// Told to Improv clients.
    pub chip: &'static str,
    /// The product name over Matter.
    pub product_name: &'static str,
    pub entities: &'static [&'static dyn Entity],
    /// The app's task watches: a task that stops feeding its watch restarts the device.
    pub watches: &'static [&'static TaskWatch],
    /// The app's own health checks (numbers from [`APP_CHECKS`]), passed with `HEALTH.pass`.
    pub checks: &'static [Check],
    /// Extra console commands.
    pub console: Option<ConsoleExtension>,
}

/// The running kit.
#[derive(Clone, Copy)]
pub struct App {
    pub store: &'static SharedStore,
    /// `None` if the radio did not start.
    pub net: Option<Net>,
    /// Boot and crash counters (`crashes_in_a_row` lets an app stop a boot loop).
    pub stats: Option<BootStats>,
}

impl App {
    /// A setting from flash; `None` if never saved or unreadable.
    pub async fn load<S: Setting>(&self) -> Option<S> {
        match self.store.lock().await.load::<S>().await {
            Ok(value) => value,
            Err(e) => {
                warn!("config: setting {:#06x}: {e}", S::KEY);
                None
            }
        }
    }

    /// Saves a setting (only written when it changed).
    pub async fn save<S: Setting>(&self, value: &S) {
        if let Err(e) = self.store.lock().await.save(value).await {
            warn!("config: could not save setting {:#06x}: {e}", S::KEY);
        }
    }
}

/// Starts the kit (see the [crate] docs). Call first in `main`, right after `esp_hal::init`,
/// the heap and `esp_rtos::start`. Panics if the `config` partition is missing.
pub async fn start(spawner: Spawner, board: Board, config: &'static Config) -> App {
    // First: the crash record must be taken before anything can overwrite it.
    let boot = iotkit_diag::esp::boot();
    esp_println::logger::init_logger_from_env();
    let fw = config.firmware;
    info!("{} {} booted, reset cause: {:?}", fw.product, fw.version, boot.cause);
    if let Some(crash) = &boot.crash {
        error!("previous run crashed: {crash}");
    }
    for &check in config.checks {
        HEALTH.require(check);
    }

    iotkit_config::esp::init(board.flash);
    // Before anything that can fail: an update that crashes this early is still rolled back.
    iotkit_ota::esp::early_guard();
    // Rollback test of the early guard: a build with `IOTKIT_PANIC_EARLY=1` crashes before
    // `iotkit_ota::esp::boot` can count its trial boots.
    if option_env!("IOTKIT_PANIC_EARLY").is_some() {
        panic!("IOTKIT_PANIC_EARLY is set");
    }
    static STORE: StaticCell<SharedStore> = StaticCell::new();
    let store = match iotkit_config::esp::open().await {
        Ok(store) => &*STORE.init(Mutex::new(store)),
        Err(e) => panic!("config: {e}"),
    };
    let stats = match iotkit_diag::record_boot(&mut *store.lock().await, &boot).await {
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
    iotkit_ota::esp::boot(store).await;

    spawn(spawner, iotkit_diag::esp::watchdog_task(TimerGroup::new(board.timg1).wdt, config.watches));

    let credentials = iotkit_net::esp::credentials(store).await;
    if credentials.is_none() {
        // No network until provisioned: the health check cannot wait for it.
        warn!("no Wi-Fi network stored: join the setup access point, or use Improv (Bluetooth, USB serial)");
        HEALTH.pass(NETWORK);
    }
    let provisioner = match iotkit_net::esp::start(spawner, board.wifi, credentials.clone()) {
        Ok(net) => Some(start_services(spawner, net, store, board.bt, config, credentials.is_none()).await),
        Err(e) => {
            error!("Wi-Fi: {e:?}");
            HEALTH.pass(NETWORK);
            HEALTH.pass(BROKER);
            None
        }
    };
    spawn(spawner, supervisor_task(store));
    if let Some(pin) = board.boot_button {
        let button = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
        spawn(spawner, iotkit_provision::esp::button_task(button, store));
    }
    let (usb_rx, _usb_tx) = UsbSerialJtag::new(board.usb).into_async().split();
    spawn(spawner, console::console_task(usb_rx, store, provisioner, config));

    App { store, net: provisioner.map(|p| p.net), stats }
}

/// Everything on the network: links, OTA, provisioning.
async fn start_services(
    spawner: Spawner,
    net: Net,
    store: &'static SharedStore,
    bt: BT<'static>,
    config: &'static Config,
    unprovisioned: bool,
) -> iotkit_provision::esp::Provisioner {
    spawn(spawner, network_check_task(net));
    // With `ha-update`, the kit's firmware entity after the app's.
    #[cfg(feature = "ha-update")]
    let entities = update::with_update(config.entities);
    #[cfg(not(feature = "ha-update"))]
    let entities = config.entities;
    static DEVICE: StaticCell<Device> = StaticCell::new();
    let device = &*DEVICE.init(Device {
        name: net.hostname,
        manufacturer: config.manufacturer,
        model: config.model,
        sw_version: config.firmware.version_text,
        entities,
    });
    #[cfg(feature = "matter-wifi")]
    {
        let v = config.firmware.version;
        let product = iotkit_link_matter::wifi::Product {
            name: config.product_name,
            version: config.firmware.version_text,
            version_number: (v.major as u32) << 16 | (v.minor as u32) << 8 | v.patch as u32,
        };
        spawn(spawner, iotkit_link_matter::wifi::matter_task(net, store, product, device));
    }
    match iotkit_link_mqtt::esp::broker(store).await {
        Some(broker) => {
            spawn(spawner, iotkit_link_mqtt::esp::mqtt_task(net, device, broker));
            spawn(spawner, broker_check_task());
        }
        None => {
            warn!("no MQTT broker: set one with `mqtt HOST[:PORT]` on the console");
            HEALTH.pass(BROKER);
        }
    }
    spawn(spawner, iotkit_ota::esp::serve_task(net, store, config.firmware));
    #[cfg(feature = "ha-update")]
    spawn(spawner, update::update_task(net, store, config.firmware));
    let info = iotkit_provision::esp::DeviceInfo {
        firmware: config.firmware.product,
        version: config.firmware.version_text,
        chip: config.chip,
    };
    let provisioner = iotkit_provision::esp::Provisioner { net, store, info };
    spawn(spawner, iotkit_provision::esp::window_task(bt, provisioner));
    spawn(spawner, iotkit_provision::esp::dhcp_task(provisioner));
    spawn(spawner, iotkit_provision::esp::dns_task(provisioner));
    spawn(spawner, iotkit_provision::esp::portal_task(provisioner));
    spawn(spawner, iotkit_provision::esp::portal_job_task(provisioner));
    if unprovisioned {
        iotkit_provision::esp::open_window();
    }
    provisioner
}

/// Spawns a kit task; each runs once, so a failure is a bug.
fn spawn<S>(spawner: Spawner, token: Result<embassy_executor::SpawnToken<S>, embassy_executor::SpawnError>) {
    match token {
        Ok(token) => spawner.spawn(token),
        Err(e) => panic!("iotkit-app: task spawned twice: {e:?}"),
    }
}

#[embassy_executor::task]
async fn network_check_task(net: Net) {
    net.wait_online().await;
    HEALTH.pass(NETWORK);
}

#[embassy_executor::task]
async fn broker_check_task() {
    iotkit_link_mqtt::esp::wait_connected().await;
    HEALTH.pass(BROKER);
}

/// Once healthy: the boot is stable, and an update just installed is kept.
#[embassy_executor::task]
async fn supervisor_task(store: &'static SharedStore) {
    HEALTH.healthy().await;
    iotkit_ota::esp::confirm(store).await;
    match BootStats::mark_stable(&mut *store.lock().await).await {
        Ok(()) => info!("healthy"),
        Err(e) => warn!("could not mark the boot stable: {e}"),
    }
}

/// Saves a setting a while after its last change, so a dragged slider writes the flash once.
///
/// ```ignore
/// let mut later = SaveLater::new(Duration::from_secs(3));
/// // on each change:
/// later.changed();
/// // now and then (e.g. on a ticker):
/// if later.due() { app.save(&state).await }
/// ```
pub struct SaveLater {
    delay: Duration,
    at: Option<Instant>,
}

impl SaveLater {
    pub const fn new(delay: Duration) -> Self {
        Self { delay, at: None }
    }

    /// The value changed: save it after the delay (counted from the last change).
    pub fn changed(&mut self) {
        self.at = Some(Instant::now() + self.delay);
    }

    /// Whether to save now; `true` once per series of changes.
    pub fn due(&mut self) -> bool {
        let due = self.at.is_some_and(|at| Instant::now() >= at);
        if due {
            self.at = None;
        }
        due
    }
}
