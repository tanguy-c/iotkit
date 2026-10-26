//! [`start`] over Wi-Fi: the network, MQTT, the ESPHome API, Matter over Wi-Fi, updates over
//! HTTP, the setup windows and the console.

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
#[cfg(feature = "improv-ble")]
use esp_hal::peripherals::BT;
use esp_hal::{
    gpio::{AnyPin, Input, InputConfig, Pull},
    peripherals::{TIMG1, WIFI},
    timer::timg::TimerGroup,
};
#[cfg(all(any(feature = "console", feature = "improv-serial"), feature = "esp32"))]
use esp_hal::{peripherals::UART0, uart::Uart};
#[cfg(all(any(feature = "console", feature = "improv-serial"), not(feature = "esp32")))]
use esp_hal::{peripherals::USB_DEVICE, usb::usb_serial_jtag::UsbSerialJtag};
use log::{error, info, warn};
use static_cell::StaticCell;

#[cfg(any(feature = "console", feature = "improv-serial"))]
use super::console;
#[cfg(feature = "ha-update")]
use super::update;
use super::{App, Config, spawn};
#[cfg(feature = "mqtt")]
use crate::health::BROKER;
use crate::health::{HEALTH, NETWORK, RECORDED, Radio, radio_failed, supervisor_task};
use crate::{Device, Net, SharedStore};

/// The peripherals the kit takes; the app keeps the rest.
pub struct Board {
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

/// Starts the kit (see the [crate] docs). Call first in `main`, right after [`init!`]. Panics
/// without it, or if the `config` partition is missing.
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
    require_checks();

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
    crate::ota::esp::boot(store, fw.version).await;

    spawn(spawner, crate::diag::esp::watchdog_task(TimerGroup::new(board.timg1).wdt, config.watches));
    // Before the app starts: its first command is the state saved before the restart.
    #[cfg(feature = "remember")]
    {
        let saved = crate::remember::esp::restore(store, config.entities).await;
        spawn(spawner, crate::remember::esp::remember_task(store, config.entities, saved));
    }

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
            radio_failed(Radio::WiFi);
            None
        }
    };
    spawn(spawner, supervisor_task(store, fw.version));
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
pub(super) async fn start_services(
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
    // With no link to Home Assistant, the app uses the network on its own.
    #[cfg_attr(not(any(feature = "mqtt", feature = "esphome-api", feature = "matter-wifi")), allow(unused_variables))]
    let device = &*DEVICE.init(Device {
        name: net.hostname,
        manufacturer: config.manufacturer,
        model: config.model,
        sw_version: config.firmware.version_text,
        entities,
    });
    #[cfg(feature = "matter-wifi")]
    {
        let task = crate::matter::wifi::matter_task(net, store, config.product_name, config.firmware, device);
        spawn(spawner, task);
    }
    #[cfg(feature = "mqtt")]
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
    spawn(spawner, crate::esphome::esp::api_task(net, device, store, crate::esphome::esp::key(store).await));
    #[cfg(feature = "ota-push")]
    spawn(spawner, crate::ota::esp::serve_task(net, store, config.firmware));
    #[cfg(all(feature = "ha-update", not(feature = "ota-push")))]
    spawn(spawner, crate::ota::esp::install_task(net, store, config.firmware));
    #[cfg(feature = "ha-update")]
    spawn(spawner, update::update_task(net, store, config.firmware));
    let provisioner = provisioner(net, store, config);
    spawn_provisioning(spawner, bt, provisioner, unprovisioned);
    provisioner
}

/// Bluetooth for Improv: the peripheral with `improv-ble`, nothing without.
#[cfg(feature = "improv-ble")]
pub(super) type Ble = BT<'static>;
#[cfg(not(feature = "improv-ble"))]
pub(super) type Ble = ();

/// What provisioning needs.
pub(super) fn provisioner(
    net: Net,
    store: &'static SharedStore,
    config: &Config,
) -> crate::provision::esp::Provisioner {
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
pub(super) fn spawn_provisioning(
    spawner: Spawner,
    bt: Ble,
    p: crate::provision::esp::Provisioner,
    unprovisioned: bool,
) {
    #[cfg(feature = "improv-ble")]
    spawn(spawner, crate::provision::esp::window_task(bt, p));
    #[cfg(not(feature = "improv-ble"))]
    spawn(spawner, crate::provision::esp::window_task(p));
    #[cfg(feature = "softap")]
    spawn(spawner, crate::provision::esp::portal_job_task(p));
    if unprovisioned {
        crate::provision::esp::open_window();
    }
}

/// The port the console reads: USB-Serial-JTAG, or the ESP32's UART0 and its RX pin.
#[cfg(all(any(feature = "console", feature = "improv-serial"), not(feature = "esp32")))]
pub(super) type ConsolePort = USB_DEVICE<'static>;
#[cfg(all(any(feature = "console", feature = "improv-serial"), feature = "esp32"))]
pub(super) type ConsolePort = (UART0<'static>, AnyPin<'static>);

/// The console's serial port: console commands and Improv frames.
#[cfg(any(feature = "console", feature = "improv-serial"))]
pub(super) fn spawn_console(
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

/// The checks of the network over Wi-Fi, on top of [`RECORDED`].
pub(super) fn require_checks() {
    HEALTH.require(NETWORK);
    #[cfg(feature = "mqtt")]
    HEALTH.require(BROKER);
    #[cfg(feature = "matter-wifi")]
    HEALTH.require(crate::health::MATTER);
    #[cfg(feature = "esphome-api")]
    HEALTH.require(crate::health::ESPHOME);
}

#[embassy_executor::task]
pub(super) async fn network_check_task(net: Net) {
    net.wait_online().await;
    HEALTH.pass(NETWORK);
}

#[cfg(feature = "mqtt")]
#[embassy_executor::task]
pub(super) async fn broker_check_task() {
    crate::mqtt::esp::wait_connected().await;
    HEALTH.pass(BROKER);
}
