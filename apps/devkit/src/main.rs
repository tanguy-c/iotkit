//! ESP32-C6-DevKitC-1 reference firmware: the smoke test for each iotkit crate.
//!
//! Shows up in Home Assistant over MQTT: the RGB LED as a light, a "Color cycle" switch, chip
//! temperature, uptime, Wi-Fi signal and a Restart button. Everything else comes from
//! `iotkit_app::start`: Wi-Fi (or a setup window), `iotkit-xxxx.local`, signed updates
//! (`cargo xtask ota push`), boot counters and the last crash, the USB serial console (`help`).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Instant, Ticker};
use esp_hal::{Async, clock::CpuClock, rmt::Rmt, time::Rate, timer::timg::TimerGroup};
// The chip temperature sensor: on the ESP32-C6 and C3 only.
#[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
use esp_hal::tsens::{self, TemperatureSensor};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use iotkit_app::{
    APP_CHECKS, App, Board, Button, Check, Config, ConsoleExtension, Entity, HEALTH, Light, LightState, PublicKey,
    SaveLater, Sensor, Setting, Switch, TaskWatch,
};
use log::{info, warn};
use smart_leds::{RGB8, SmartLedsWriteAsync, brightness};

esp_bootloader_esp_idf::esp_app_desc!();

/// Updates must be signed with the secret half (`cargo xtask keygen`).
static OTA_KEY: PublicKey = iotkit_app::public_key(include_str!("../../../keys/ota.pub"));

/// Each chip's DevKit pins: the WS2812 RGB LED and the BOOT button (from the boards' user
/// guides; only the ESP32-C6-DevKitC-1 is tested). A macro, so each pin is taken where it is
/// used.
#[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
macro_rules! pin {
    ($p:ident, led) => {
        $p.GPIO8
    };
    ($p:ident, boot) => {
        $p.GPIO9
    };
}
#[cfg(feature = "esp32c5")]
macro_rules! pin {
    ($p:ident, led) => {
        $p.GPIO27
    };
    ($p:ident, boot) => {
        $p.GPIO28
    };
}
#[cfg(feature = "esp32s3")]
macro_rules! pin {
    ($p:ident, led) => {
        $p.GPIO38
    };
    ($p:ident, boot) => {
        $p.GPIO0
    };
}

/// The DevKit's WS2812 RGB LED, driven by an RMT channel.
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

static LED: Light = Light::new("led", "LED");
static CYCLE: Switch = Switch::new("color_cycle", "Color cycle").icon("mdi:palette");
#[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
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
#[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
static ENTITIES: [&dyn Entity; 6] = [&LED, &CYCLE, &TEMPERATURE, &UPTIME, &SIGNAL, &RESTART];
#[cfg(not(any(feature = "esp32c6", feature = "esp32c3")))]
static ENTITIES: [&dyn Entity; 5] = [&LED, &CYCLE, &UPTIME, &SIGNAL, &RESTART];

/// The main loop ran for a while.
const RUNNING: Check = Check(APP_CHECKS);

static MAIN_WATCH: TaskWatch = TaskWatch::new("main", 15_000);
static LED_WATCH: TaskWatch = TaskWatch::new("led", 3_000);
static WATCHES: [&TaskWatch; 2] = [&MAIN_WATCH, &LED_WATCH];

static CONFIG: Config = Config {
    firmware: iotkit_app::firmware!(&OTA_KEY),
    manufacturer: "iotkit",
    model: "ESP32-C6-DevKitC-1",
    chip: "ESP32-C6",
    product_name: "iotkit devkit",
    entities: &ENTITIES,
    watches: &WATCHES,
    checks: &[RUNNING],
    console: Some(ConsoleExtension {
        help: "  starve         stop feeding the LED task's watch (watchdog record within ~4 s)",
        run: console_command,
    }),
};

/// The LED's state, kept across restarts.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedLight {
    on: bool,
    brightness: u8,
    rgb: [u8; 3],
    cycle: bool,
}

impl Setting for SavedLight {
    const KEY: u16 = iotkit_app::config::APP_KEYS;
    const VERSION: u8 = 1;
}

/// `starve` was typed: the LED task stops feeding its watch.
static STARVE_LED: AtomicBool = AtomicBool::new(false);

fn console_command(cmd: &str) -> bool {
    match cmd {
        "starve" => {
            info!("LED task stops feeding its watch");
            STARVE_LED.store(true, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // The Wi-Fi and Bluetooth drivers allocate.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 96 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);

    let board = Board {
        flash: p.FLASH,
        wifi: p.WIFI,
        bt: p.BT,
        timg1: p.TIMG1,
        usb: p.USB_DEVICE,
        boot_button: Some(pin!(p, boot).into()),
    };
    let app = iotkit_app::start(spawner, board, &CONFIG).await;
    // Rollback test: a build with `DEVKIT_PANIC_AT_BOOT=1` never gets healthy.
    if option_env!("DEVKIT_PANIC_AT_BOOT").is_some() {
        panic!("DEVKIT_PANIC_AT_BOOT is set");
    }

    #[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
    {
        let tsens = TemperatureSensor::new(p.TSENS, tsens::Config::default()).unwrap();
        spawner.spawn(temperature_task(tsens).unwrap());
    }
    spawner.spawn(restart_task().unwrap());
    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, pin!(p, led), Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led, app).unwrap());

    let mut ticker = Ticker::every(Duration::from_secs(10));
    let mut ticks = 0u32;
    loop {
        MAIN_WATCH.feed();
        ticker.next().await;
        HEALTH.pass(RUNNING);
        if ticks.is_multiple_of(6) {
            info!("alive, up {} s", Instant::now().as_secs());
            UPTIME.set(Instant::now().as_secs() as f32);
            SIGNAL.set(iotkit_app::net::esp::rssi().map_or(f32::NAN, |r| r as f32));
        }
        ticks += 1;
    }
}

#[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
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
    iotkit_app::diag::esp::restart().await;
}

#[embassy_executor::task]
async fn led_task(mut led: Led, app: App) {
    use iotkit_app::provision::esp::{HELD, IDENTIFY, wait_press};
    const COLORS: [RGB8; 4] = [RGB8::new(255, 0, 0), RGB8::new(0, 255, 0), RGB8::new(0, 0, 255), RGB8::new(0, 0, 0)];
    let saved = app.load::<SavedLight>().await;
    let saved = saved.unwrap_or(SavedLight { on: true, brightness: 10, rgb: [0, 255, 0], cycle: false });
    LED.set(LightState { on: saved.on, brightness: saved.brightness, rgb: saved.rgb });
    CYCLE.set(saved.cycle);
    let mut ticker = Ticker::every(Duration::from_millis(800));
    let mut save = SaveLater::new(Duration::from_secs(3));
    let mut step = 0usize;
    // Ticks left of white flashes (Improv "identify").
    let mut identify = 0u8;
    loop {
        match select4(LED.wait_command(), CYCLE.wait_command(), wait_press(), ticker.next()).await {
            Either4::First(state) => {
                LED.set(state);
                save.changed();
            }
            Either4::Second(on) => {
                CYCLE.set(on);
                save.changed();
            }
            // A short press of BOOT toggles the LED.
            Either4::Third(()) => {
                let l = LED.get().unwrap_or_default();
                LED.set(LightState { on: !l.on, ..l });
                save.changed();
            }
            Either4::Fourth(()) => {
                step += 1;
                if !STARVE_LED.load(Ordering::Relaxed) {
                    LED_WATCH.feed();
                }
                if save.due() {
                    let l = LED.get().unwrap_or_default();
                    let saved =
                        SavedLight { on: l.on, brightness: l.brightness, rgb: l.rgb, cycle: CYCLE.get() == Some(true) };
                    app.save(&saved).await;
                }
            }
        }
        if IDENTIFY.try_take().is_some() {
            identify = 6;
        }
        let light = LED.get().unwrap_or_default();
        let (color, level) = match (HELD.load(Ordering::Relaxed), identify) {
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
