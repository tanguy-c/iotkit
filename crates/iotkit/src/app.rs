//! The kit in one call: [`start`], the [`Board`] and [`Config`], the console, deep sleep and
//! updates from Home Assistant. Re-exported at the crate root.

pub mod battery;
mod console;
#[cfg(feature = "deep-sleep")]
mod sleep;
#[cfg(feature = "ha-update")]
mod update;

#[cfg(feature = "deep-sleep")]
pub use sleep::{SleepConfig, SleepyApp, WakeReason, start_sleepy, wake_reason};

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant};
#[cfg(feature = "improv-ble")]
use esp_hal::peripherals::BT;
use esp_hal::{
    gpio::{AnyPin, Input, InputConfig, Pull},
    peripherals::{FLASH, TIMG1, WIFI},
    timer::timg::TimerGroup,
};
#[cfg(all(any(feature = "console", feature = "improv-serial"), feature = "esp32"))]
use esp_hal::{peripherals::UART0, uart::Uart};
#[cfg(all(any(feature = "console", feature = "improv-serial"), not(feature = "esp32")))]
use esp_hal::{peripherals::USB_DEVICE, usb::usb_serial_jtag::UsbSerialJtag};
use log::{error, info, warn};

use crate::{BootStats, Check, Device, Entity, Firmware, Health, Net, Setting, SharedStore, TaskWatch};
use static_cell::StaticCell;

pub use console::ConsoleExtension;

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
    /// Holds the settings, the update slots and the crash record.
    pub flash: FLASH<'static>,
    /// The Wi-Fi radio.
    pub wifi: WIFI<'static>,
    /// Bluetooth LE, for Improv (`improv-ble`).
    #[cfg(feature = "improv-ble")]
    pub bt: BT<'static>,
    /// Its watchdog backs the task watchdog.
    pub timg1: TIMG1<'static>,
    /// The USB serial port: console and Improv (`console`, `improv-serial`).
    #[cfg(all(any(feature = "console", feature = "improv-serial"), not(feature = "esp32")))]
    pub usb: USB_DEVICE<'static>,
    /// The ESP32's console port (UART0, behind the board's USB bridge), for the console and
    /// Improv (`console`, `improv-serial`).
    #[cfg(all(any(feature = "console", feature = "improv-serial"), feature = "esp32"))]
    pub uart: UART0<'static>,
    /// UART0's RX pin: GPIO3.
    #[cfg(all(any(feature = "console", feature = "improv-serial"), feature = "esp32"))]
    pub uart_rx: AnyPin<'static>,
    /// The BOOT button (active low; GPIO9 on the ESP32-C6-DevKitC-1): the setup window and
    /// factory reset ([`provision::gesture`]); a short press goes to the app
    /// ([`provision::esp::wait_press`]).
    pub boot_button: Option<AnyPin<'static>>,
}

/// What the device is: `Config::new(iotkit_app::firmware!(&OTA_KEY))`, then the builders.
///
/// ```ignore
/// static CONFIG: Config = Config::new(iotkit_app::firmware!(&OTA_KEY))
///     .manufacturer("Me")
///     .entities(&[&LED, &TEMPERATURE]);
/// ```
#[non_exhaustive]
pub struct Config {
    /// `iotkit_app::firmware!(&OTA_KEY)`: package name, version, update key.
    pub firmware: Firmware,
    /// Shown in Home Assistant (default `"iotkit"`).
    pub manufacturer: &'static str,
    /// Shown in Home Assistant (default: the chip, [`CHIP`]).
    pub model: &'static str,
    /// The product name over Matter (default: the package name).
    pub product_name: &'static str,
    /// The device model: every entity, in order (Matter endpoints follow it).
    pub entities: &'static [&'static dyn Entity],
    /// The app's task watches: a task that stops feeding its watch restarts the device.
    pub watches: &'static [&'static TaskWatch],
    /// The app's own health checks (numbers from [`APP_CHECKS`]), passed with `HEALTH.pass`.
    pub checks: &'static [Check],
    /// Extra console commands.
    pub console: Option<ConsoleExtension>,
    /// [`CHIP`]; read through the config, which is smaller code than the constant.
    chip: &'static str,
}

impl Config {
    /// The defaults: no entities, watches, checks or console commands.
    pub const fn new(firmware: Firmware) -> Self {
        Self {
            firmware,
            manufacturer: "iotkit",
            model: CHIP,
            product_name: firmware.product,
            entities: &[],
            watches: &[],
            checks: &[],
            console: None,
            chip: CHIP,
        }
    }
    /// See [`Config::manufacturer`](Self#structfield.manufacturer).
    pub const fn manufacturer(mut self, manufacturer: &'static str) -> Self {
        self.manufacturer = manufacturer;
        self
    }
    /// See [`Config::model`](Self#structfield.model).
    pub const fn model(mut self, model: &'static str) -> Self {
        self.model = model;
        self
    }
    /// See [`Config::product_name`](Self#structfield.product_name).
    pub const fn product_name(mut self, name: &'static str) -> Self {
        self.product_name = name;
        self
    }
    /// See [`Config::entities`](Self#structfield.entities).
    pub const fn entities(mut self, entities: &'static [&'static dyn Entity]) -> Self {
        self.entities = entities;
        self
    }
    /// See [`Config::watches`](Self#structfield.watches).
    pub const fn watches(mut self, watches: &'static [&'static TaskWatch]) -> Self {
        self.watches = watches;
        self
    }
    /// See [`Config::checks`](Self#structfield.checks).
    pub const fn checks(mut self, checks: &'static [Check]) -> Self {
        self.checks = checks;
        self
    }
    /// See [`Config::console`](Self#structfield.console).
    pub const fn console(mut self, console: ConsoleExtension) -> Self {
        self.console = Some(console);
        self
    }
}

/// The chip, as told to Improv clients (`"ESP32-C6"`, ...).
pub const CHIP: &str = if cfg!(feature = "esp32c6") {
    "ESP32-C6"
} else if cfg!(feature = "esp32c3") {
    "ESP32-C3"
} else if cfg!(feature = "esp32c5") {
    "ESP32-C5"
} else if cfg!(feature = "esp32") {
    "ESP32"
} else {
    "ESP32-S3"
};

/// Sets up the chip for the kit, in `main` before [`start`]: the app descriptor, the clocks at
/// full speed, the heap (64 KB reclaimed from the bootloader plus `heap`, 96 KB by default; the
/// Wi-Fi and Bluetooth drivers allocate) and the RTOS timer. Declares `$p`, the peripherals
/// (`TIMG0` and `FROM_CPU_INTR0` are taken).
///
/// ```ignore
/// iotkit_app::init!(p);
/// iotkit_app::init!(mut p, heap: 128 * 1024);   // `mut`: to reborrow peripherals
/// ```
#[cfg(not(feature = "esp32"))]
#[macro_export]
macro_rules! init {
    (mut $p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [mut] $p, $($heap)?);
    };
    ($p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [] $p, $($heap)?);
    };
    (@go [$($mut:tt)?] $p:ident,) => {
        $crate::init!(@go [$($mut)?] $p, 96 * 1024);
    };
    (@go [$($mut:tt)?] $p:ident, $heap:expr) => {
        $crate::__private::esp_bootloader_esp_idf::esp_app_desc!();
        let $($mut)? $p = $crate::__private::esp_hal::init(
            $crate::__private::esp_hal::Config::default()
                .with_cpu_clock($crate::__private::esp_hal::clock::CpuClock::max()),
        );
        $crate::__private::esp_alloc::heap_allocator!(#[$crate::__private::esp_hal::ram(reclaimed)] size: 64 * 1024);
        $crate::__private::esp_alloc::heap_allocator!(size: $heap);
        let timg0 = $crate::__private::esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        $crate::__private::esp_rtos::start(timg0.timer0, $p.FROM_CPU_INTR0);
    };
}

/// [`init!`] for the original ESP32: the heap is the 96 KB the bootloader leaves (`heap` sets
/// it, at most 96 KB).
#[cfg(feature = "esp32")]
#[macro_export]
macro_rules! init {
    (mut $p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [mut] $p, $($heap)?);
    };
    ($p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [] $p, $($heap)?);
    };
    (@go [$($mut:tt)?] $p:ident,) => {
        $crate::init!(@go [$($mut)?] $p, 96 * 1024);
    };
    (@go [$($mut:tt)?] $p:ident, $heap:expr) => {
        $crate::__private::esp_bootloader_esp_idf::esp_app_desc!();
        let $($mut)? $p = $crate::__private::esp_hal::init(
            $crate::__private::esp_hal::Config::default()
                .with_cpu_clock($crate::__private::esp_hal::clock::CpuClock::max()),
        );
        // The ESP32's data RAM is small: the heap is the RAM the bootloader leaves (`dram2`).
        $crate::__private::esp_alloc::heap_allocator!(#[$crate::__private::esp_hal::ram(reclaimed)] size: $heap);
        let timg0 = $crate::__private::esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        $crate::__private::esp_rtos::start(timg0.timer0, $p.FROM_CPU_INTR0);
    };
}

/// The running kit.
#[derive(Clone, Copy)]
pub struct App {
    /// The settings in flash, shared with the kit (load and save the app's own).
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

    crate::config::esp::init(board.flash);
    // Before anything that can fail: an update that crashes this early is still rolled back.
    crate::ota::esp::early_guard();
    // Rollback test of the early guard: a build with `IOTKIT_PANIC_EARLY=1` crashes before
    // `crate::ota::esp::boot` can count its trial boots.
    if option_env!("IOTKIT_PANIC_EARLY").is_some() {
        panic!("IOTKIT_PANIC_EARLY is set");
    }
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
    crate::ota::esp::boot(store).await;

    spawn(spawner, crate::diag::esp::watchdog_task(TimerGroup::new(board.timg1).wdt, config.watches));

    let credentials = crate::net::esp::credentials(store).await;
    if credentials.is_none() {
        // No network until provisioned: the health check cannot wait for it.
        warn!("no Wi-Fi network stored: join the setup access point, or use Improv (Bluetooth, USB serial)");
        HEALTH.pass(NETWORK);
    }
    #[cfg(feature = "improv-ble")]
    let bt = board.bt;
    #[cfg(not(feature = "improv-ble"))]
    let bt = ();
    let provisioner = match crate::net::esp::start(spawner, board.wifi, credentials.clone()) {
        Ok(net) => Some(start_services(spawner, net, store, bt, config, credentials.is_none()).await),
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
        spawn(spawner, crate::provision::esp::button_task(button, store));
    }
    #[cfg(all(any(feature = "console", feature = "improv-serial"), not(feature = "esp32")))]
    spawn_console(spawner, board.usb, store, provisioner, config);
    #[cfg(all(any(feature = "console", feature = "improv-serial"), feature = "esp32"))]
    spawn_console(spawner, (board.uart, board.uart_rx), store, provisioner, config);

    App { store, net: provisioner.map(|p| p.net), stats }
}

/// Everything on the network: links, OTA, provisioning.
async fn start_services(
    spawner: Spawner,
    net: Net,
    store: &'static SharedStore,
    bt: Ble,
    config: &'static Config,
    unprovisioned: bool,
) -> crate::provision::esp::Provisioner {
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
        let product = crate::matter::wifi::Product {
            name: config.product_name,
            version: config.firmware.version_text,
            version_number: (v.major as u32) << 16 | (v.minor as u32) << 8 | v.patch as u32,
        };
        spawn(spawner, crate::matter::wifi::matter_task(net, store, product, device));
    }
    match crate::mqtt::esp::broker(store).await {
        Some(broker) => {
            spawn(spawner, crate::mqtt::esp::mqtt_task(net, device, broker));
            spawn(spawner, broker_check_task());
        }
        None => {
            // With the ESPHome API, no broker is the normal setup: nothing to say.
            #[cfg(not(feature = "esphome-api"))]
            warn!("no MQTT broker: set one with `mqtt HOST[:PORT]` on the console");
            HEALTH.pass(BROKER);
        }
    }
    #[cfg(feature = "esphome-api")]
    spawn(spawner, crate::esphome::esp::api_task(net, device, crate::esphome::esp::key(store).await));
    #[cfg(feature = "ota-push")]
    spawn(spawner, crate::ota::esp::serve_task(net, store, config.firmware));
    #[cfg(feature = "ha-update")]
    spawn(spawner, update::update_task(net, store, config.firmware));
    let provisioner = provisioner(net, store, config);
    spawn_provisioning(spawner, bt, provisioner, unprovisioned);
    provisioner
}

/// Bluetooth for Improv: the peripheral with `improv-ble`, nothing without.
#[cfg(feature = "improv-ble")]
type Ble = BT<'static>;
#[cfg(not(feature = "improv-ble"))]
type Ble = ();

/// What provisioning needs.
fn provisioner(net: Net, store: &'static SharedStore, config: &Config) -> crate::provision::esp::Provisioner {
    let info = crate::provision::esp::DeviceInfo {
        firmware: config.firmware.product,
        version: config.firmware.version_text,
        chip: config.chip,
    };
    crate::provision::esp::Provisioner { net, store, info }
}

/// The setup windows (Improv over Bluetooth, the access point), open at once while no network
/// is stored.
#[cfg_attr(not(feature = "improv-ble"), allow(unused_variables))]
fn spawn_provisioning(spawner: Spawner, bt: Ble, p: crate::provision::esp::Provisioner, unprovisioned: bool) {
    #[cfg(feature = "improv-ble")]
    spawn(spawner, crate::provision::esp::window_task(bt, p));
    #[cfg(not(feature = "improv-ble"))]
    spawn(spawner, crate::provision::esp::window_task(p));
    #[cfg(feature = "softap")]
    {
        spawn(spawner, crate::provision::esp::dhcp_task(p));
        spawn(spawner, crate::provision::esp::dns_task(p));
        spawn(spawner, crate::provision::esp::portal_task(p));
        spawn(spawner, crate::provision::esp::portal_job_task(p));
    }
    if unprovisioned {
        crate::provision::esp::open_window();
    }
}

/// The port the console reads: USB-Serial-JTAG, or the ESP32's UART0 and its RX pin.
#[cfg(all(any(feature = "console", feature = "improv-serial"), not(feature = "esp32")))]
type ConsolePort = USB_DEVICE<'static>;
#[cfg(all(any(feature = "console", feature = "improv-serial"), feature = "esp32"))]
type ConsolePort = (UART0<'static>, AnyPin<'static>);

/// The console's serial port: console commands and Improv frames.
#[cfg(any(feature = "console", feature = "improv-serial"))]
fn spawn_console(
    spawner: Spawner,
    port: ConsolePort,
    store: &'static SharedStore,
    p: Option<crate::provision::esp::Provisioner>,
    config: &'static Config,
) {
    #[cfg(not(feature = "esp32"))]
    let (rx, _tx) = UsbSerialJtag::new(port).into_async().split();
    // The bootloader set UART0 up at 115 200 baud; esp-println writes to it directly.
    #[cfg(feature = "esp32")]
    let (rx, _tx) = match Uart::new(port.0, esp_hal::uart::Config::default()) {
        Ok(uart) => uart.with_rx(port.1).into_async().split(),
        Err(e) => return log::error!("console: UART0: {e:?}"),
    };
    spawn(spawner, console::console_task(rx, store, p, config));
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
    crate::mqtt::esp::wait_connected().await;
    HEALTH.pass(BROKER);
}

/// Once healthy: the boot is stable, and an update just installed is kept.
#[embassy_executor::task]
async fn supervisor_task(store: &'static SharedStore) {
    HEALTH.healthy().await;
    crate::ota::esp::confirm(store).await;
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
///
/// In use: `apps/devkit/src/main.rs`.
pub struct SaveLater {
    delay: Duration,
    at: Option<Instant>,
}

impl SaveLater {
    /// Saves `delay` after the last change.
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
