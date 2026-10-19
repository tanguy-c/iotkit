//! ESP32-C6-DevKitC-1: logs a heartbeat and cycles the RGB LED.
//! The smoke test for the template, the tooling and, later, each iotkit crate.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Ticker};
use esp_backtrace as _;
use esp_hal::{Async, clock::CpuClock, rmt::Rmt, time::Rate, timer::timg::TimerGroup};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use log::{info, warn};
use smart_leds::{RGB8, SmartLedsWriteAsync, brightness};

esp_bootloader_esp_idf::esp_app_desc!();

/// The DevKit's WS2812 LED on GPIO8, driven by an RMT channel.
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("{} {} booted", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, peripherals.GPIO8, Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led).unwrap());

    let mut ticker = Ticker::every(Duration::from_secs(10));
    loop {
        ticker.next().await;
        info!("alive, up {} s", Instant::now().as_secs());
    }
}

#[embassy_executor::task]
async fn led_task(mut led: Led) {
    const COLORS: [RGB8; 4] = [RGB8::new(255, 0, 0), RGB8::new(0, 255, 0), RGB8::new(0, 0, 255), RGB8::new(0, 0, 0)];
    let mut ticker = Ticker::every(Duration::from_millis(800));
    for color in COLORS.iter().cycle() {
        // 10/255 is plenty for a bare LED on a desk.
        if let Err(e) = led.write(brightness([*color].into_iter(), 10)).await {
            warn!("LED write failed: {e:?}");
        }
        ticker.next().await;
    }
}
