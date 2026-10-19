//! ESP32-C6-DevKitC-1 reference firmware: the smoke test for each iotkit crate.
//!
//! Shows up in Home Assistant over MQTT: the RGB LED as a light, a "Color cycle" switch, chip
//! temperature, uptime, Wi-Fi signal and a Restart button. Joins Wi-Fi (or opens a setup
//! window), answers `iotkit-xxxx.local`, accepts signed updates (`cargo xtask ota push`), keeps
//! boot counters and the last crash in flash, and has a USB serial console (`help`).

#![no_std]
#![no_main]

mod console;

use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Ticker};
use esp_hal::{
    Async,
    clock::CpuClock,
    gpio::{Input, InputConfig, Pull},
    rmt::Rmt,
    time::Rate,
    timer::timg::TimerGroup,
    tsens::{self, TemperatureSensor},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use iotkit_config::esp::SharedStore;
use iotkit_diag::{BootStats, TaskWatch, esp::watchdog_task};
use iotkit_image::PublicKey;
use iotkit_model::{Button, Device, Entity, Light, LightState, Sensor, Switch};
use iotkit_net::esp::Net;
use iotkit_ota::esp::Firmware;
use iotkit_provision::esp::{DeviceInfo, Provisioner};
use iotkit_runtime::{Check, Health};
use log::{error, info, warn};
use smart_leds::{RGB8, SmartLedsWriteAsync, brightness};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// Updates must be signed with the secret half (`cargo xtask keygen`).
static OTA_KEY: PublicKey = iotkit_image::public_key(include_str!("../../../keys/ota.pub"));
pub static FIRMWARE: Firmware = iotkit_ota::firmware!(&OTA_KEY);

const DEVICE_INFO: DeviceInfo =
    DeviceInfo { firmware: env!("CARGO_PKG_NAME"), version: env!("CARGO_PKG_VERSION"), chip: "ESP32-C6" };

#[cfg(feature = "matter-wifi")]
const MATTER_PRODUCT: iotkit_link_matter::wifi::Product = iotkit_link_matter::wifi::Product {
    name: "iotkit devkit",
    version: env!("CARGO_PKG_VERSION"),
    version_number: (FIRMWARE.version.major as u32) << 16
        | (FIRMWARE.version.minor as u32) << 8
        | FIRMWARE.version.patch as u32,
};

/// The DevKit's WS2812 LED on GPIO8, driven by an RMT channel.
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

/// The boot was counted in flash.
const RECORDED: Check = Check(0);
/// The main loop ran for a while.
const RUNNING: Check = Check(1);
/// Wi-Fi joined and an IP address assigned.
const NETWORK: Check = Check(2);
/// Connected to the MQTT broker (when one is set).
const BROKER: Check = Check(3);
static HEALTH: Health = Health::new(&[RECORDED, RUNNING, NETWORK, BROKER]);

pub static LED: Light = Light::new("led", "LED");
pub static CYCLE: Switch = Switch::new("color_cycle", "Color cycle").icon("mdi:palette");
static TEMPERATURE: Sensor = Sensor::new("chip_temperature", "Chip temperature")
    .unit("°C")
    .device_class("temperature")
    .measurement()
    .precision(1)
    .diagnostic();
static UPTIME: Sensor = Sensor::new("uptime", "Uptime").unit("s").device_class("duration").diagnostic();
static SIGNAL: Sensor =
    Sensor::new("wifi_signal", "Wi-Fi signal").unit("dBm").device_class("signal_strength").measurement().diagnostic();
static RESTART: Button = Button::new("restart", "Restart").device_class("restart").diagnostic();
/// The LED's state, kept across restarts.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedLight {
    on: bool,
    brightness: u8,
    rgb: [u8; 3],
    cycle: bool,
}

impl iotkit_config::Setting for SavedLight {
    const KEY: u16 = iotkit_config::APP_KEYS;
    const VERSION: u8 = 1;
}

static ENTITIES: [&dyn Entity; 6] = [&LED, &CYCLE, &TEMPERATURE, &UPTIME, &SIGNAL, &RESTART];

static MAIN_WATCH: TaskWatch = TaskWatch::new("main", 15_000);
pub static LED_WATCH: TaskWatch = TaskWatch::new("led", 3_000);
static WATCHES: [&TaskWatch; 2] = [&MAIN_WATCH, &LED_WATCH];

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // First: the crash record must be taken before anything can overwrite it.
    let boot = iotkit_diag::esp::boot();
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // The Wi-Fi and Bluetooth drivers allocate.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 96 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("{} {} booted, reset cause: {:?}", FIRMWARE.product, FIRMWARE.version, boot.cause);
    if let Some(crash) = &boot.crash {
        error!("previous run crashed: {crash}");
    }

    iotkit_config::esp::init(peripherals.FLASH);
    static STORE: StaticCell<SharedStore> = StaticCell::new();
    let store = match iotkit_config::esp::open().await {
        Ok(store) => &*STORE.init(Mutex::new(store)),
        Err(e) => panic!("config: {e}"),
    };
    match iotkit_diag::record_boot(&mut *store.lock().await, &boot).await {
        Ok((stats, last)) => {
            info!("boot #{}, {} crashes ({} in a row)", stats.boots, stats.crashes, stats.crashes_in_a_row);
            if let Some(last) = last {
                info!("last crash: {last}");
            }
            HEALTH.pass(RECORDED);
        }
        Err(e) => warn!("could not record the boot: {e}"),
    }
    // After the boot is recorded (a crashing update still counts its crashes): may switch back
    // to the previous firmware and restart.
    iotkit_ota::esp::boot(store).await;
    // Rollback test: a build with `DEVKIT_PANIC_AT_BOOT=1` never gets healthy.
    if option_env!("DEVKIT_PANIC_AT_BOOT").is_some() {
        panic!("DEVKIT_PANIC_AT_BOOT is set");
    }

    spawner.spawn(watchdog_task(TimerGroup::new(peripherals.TIMG1).wdt, &WATCHES).unwrap());

    let tsens = TemperatureSensor::new(peripherals.TSENS, tsens::Config::default()).unwrap();
    spawner.spawn(temperature_task(tsens).unwrap());
    spawner.spawn(restart_task().unwrap());

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, peripherals.GPIO8, Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led, store).unwrap());

    let credentials = iotkit_net::esp::credentials(store).await;
    if credentials.is_none() {
        // No network until provisioned: the health check cannot wait for it.
        warn!("no Wi-Fi network stored: join the setup access point, or use Improv (Bluetooth, USB serial)");
        HEALTH.pass(NETWORK);
    }
    let provisioner = match iotkit_net::esp::start(spawner, peripherals.WIFI, credentials.clone()) {
        Ok(net) => {
            spawner.spawn(network_check_task(net).unwrap());
            static DEVICE: StaticCell<Device> = StaticCell::new();
            let device = &*DEVICE.init(Device {
                name: net.hostname,
                manufacturer: "iotkit",
                model: "ESP32-C6-DevKitC-1",
                sw_version: env!("CARGO_PKG_VERSION"),
                entities: &ENTITIES,
            });
            #[cfg(feature = "matter-wifi")]
            spawner.spawn(iotkit_link_matter::wifi::matter_task(net, store, MATTER_PRODUCT, device).unwrap());
            match iotkit_link_mqtt::esp::broker(store).await {
                Some(broker) => {
                    spawner.spawn(iotkit_link_mqtt::esp::mqtt_task(net, device, broker).unwrap());
                    spawner.spawn(broker_check_task().unwrap());
                }
                None => {
                    warn!("no MQTT broker: set one with `mqtt HOST[:PORT]` on the console");
                    HEALTH.pass(BROKER);
                }
            }
            spawner.spawn(iotkit_ota::esp::serve_task(net, store, FIRMWARE).unwrap());
            let provisioner = Provisioner { net, store, info: DEVICE_INFO };
            spawner.spawn(iotkit_provision::esp::window_task(peripherals.BT, provisioner).unwrap());
            spawner.spawn(iotkit_provision::esp::dhcp_task(provisioner).unwrap());
            spawner.spawn(iotkit_provision::esp::dns_task(provisioner).unwrap());
            spawner.spawn(iotkit_provision::esp::portal_task(provisioner).unwrap());
            spawner.spawn(iotkit_provision::esp::portal_job_task(provisioner).unwrap());
            if credentials.is_none() {
                iotkit_provision::esp::open_window();
            }
            Some(provisioner)
        }
        Err(e) => {
            error!("Wi-Fi: {e:?}");
            HEALTH.pass(NETWORK);
            HEALTH.pass(BROKER);
            None
        }
    };
    spawner.spawn(supervisor_task(store).unwrap());
    // BOOT is GPIO9 on the DevKit.
    let boot_button = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(iotkit_provision::esp::button_task(boot_button, store).unwrap());

    let (usb_rx, _usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();
    spawner.spawn(console::console_task(usb_rx, store, provisioner).unwrap());

    let mut ticker = Ticker::every(Duration::from_secs(10));
    let mut ticks = 0u32;
    loop {
        MAIN_WATCH.feed();
        ticker.next().await;
        HEALTH.pass(RUNNING);
        if ticks.is_multiple_of(6) {
            info!("alive, up {} s", Instant::now().as_secs());
            UPTIME.set(Instant::now().as_secs() as f32);
            SIGNAL.set(iotkit_net::esp::rssi().map_or(f32::NAN, |r| r as f32));
        }
        ticks += 1;
    }
}

#[embassy_executor::task]
async fn broker_check_task() {
    iotkit_link_mqtt::esp::wait_connected().await;
    HEALTH.pass(BROKER);
}

#[embassy_executor::task]
async fn temperature_task(tsens: TemperatureSensor<'static>) {
    let mut ticker = Ticker::every(Duration::from_secs(30));
    loop {
        // Rounded to the published precision: no publish for a change too small to show.
        let celsius = tsens.get_temperature().to_celsius();
        TEMPERATURE.set((celsius * 10.0 + 0.5f32.copysign(celsius)) as i32 as f32 / 10.0);
        ticker.next().await;
    }
}

#[embassy_executor::task]
async fn restart_task() {
    RESTART.wait_press().await;
    info!("restart requested from Home Assistant");
    iotkit_diag::esp::restart().await;
}

#[embassy_executor::task]
async fn network_check_task(net: Net) {
    net.stack.wait_config_up().await;
    HEALTH.pass(NETWORK);
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

#[embassy_executor::task]
async fn led_task(mut led: Led, store: &'static SharedStore) {
    const COLORS: [RGB8; 4] = [RGB8::new(255, 0, 0), RGB8::new(0, 255, 0), RGB8::new(0, 0, 255), RGB8::new(0, 0, 0)];
    let saved = store.lock().await.load::<SavedLight>().await.ok().flatten();
    let saved = saved.unwrap_or(SavedLight { on: true, brightness: 10, rgb: [0, 255, 0], cycle: false });
    LED.set(LightState { on: saved.on, brightness: saved.brightness, rgb: saved.rgb });
    CYCLE.set(saved.cycle);
    let mut ticker = Ticker::every(Duration::from_millis(800));
    // Saved a few seconds after the last change: a dragged slider writes the flash once.
    let mut save_at: Option<Instant> = None;
    let mut step = 0usize;
    // Ticks left of white flashes (Improv "identify").
    let mut identify = 0u8;
    loop {
        match select3(LED.wait_command(), CYCLE.wait_command(), ticker.next()).await {
            Either3::First(state) => {
                LED.set(state);
                save_at = Some(Instant::now() + Duration::from_secs(3));
            }
            Either3::Second(on) => {
                CYCLE.set(on);
                save_at = Some(Instant::now() + Duration::from_secs(3));
            }
            Either3::Third(()) => {
                step += 1;
                if !console::starve_led() {
                    LED_WATCH.feed();
                }
                if save_at.is_some_and(|at| Instant::now() >= at) {
                    save_at = None;
                    let l = LED.get().unwrap_or_default();
                    let saved =
                        SavedLight { on: l.on, brightness: l.brightness, rgb: l.rgb, cycle: CYCLE.get() == Some(true) };
                    if let Err(e) = store.lock().await.save(&saved).await {
                        warn!("could not save the LED state: {e}");
                    }
                }
            }
        }
        if iotkit_provision::esp::IDENTIFY.try_take().is_some() {
            identify = 6;
        }
        let light = LED.get().unwrap_or_default();
        let held = iotkit_provision::esp::HELD.load(core::sync::atomic::Ordering::Relaxed);
        let (color, level) = match (held, identify) {
            // BOOT held: blue from 3 s (release to provision), red from 10 s (factory reset).
            (1, _) => (RGB8::new(0, 0, 255), 10),
            (2, _) => (RGB8::new(255, 0, 0), 10),
            (_, 0) if CYCLE.get() == Some(true) => (COLORS[step % COLORS.len()], light.brightness.max(1)),
            (_, 0) if light.on => (RGB8::new(light.rgb[0], light.rgb[1], light.rgb[2]), light.brightness),
            (_, 0) => (RGB8::default(), 0),
            (_, n) => {
                identify -= 1;
                (if n % 2 == 0 { RGB8::new(255, 255, 255) } else { RGB8::default() }, 10)
            }
        };
        if let Err(e) = led.write(brightness([color].into_iter(), level)).await {
            warn!("LED write failed: {e:?}");
        }
    }
}
