//! iotkit thread light: made with `cargo iotkit new --answers xtask/answers/thread.toml` (pairing
//! and updates: its README). Only the key's path differs: the workspace's `keys/`.
//!
//! `iotkit::start` brings Matter over Thread (paired over Bluetooth), signed updates over Matter
//! (rolled back unless it rejoins Thread and its controller), crash records and the watchdog.
//! No parts yet: add entities below (https://tanguy-c.github.io/iotkit/entities.html).

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_time::Timer;
use esp_hal::{Async, rmt::Rmt, time::Rate};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use iotkit::{Board, Config, Light, LightState, PublicKey};
use log::warn;
use smart_leds::{RGB8, SmartLedsWriteAsync, brightness};

/// Updates must be signed with the secret half (`cargo iotkit keygen`).
static OTA_KEY: PublicKey = iotkit::public_key(include_str!("../../../keys/ota.pub"));

static LED: Light = Light::new("status_led", "Status LED");

static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
    .manufacturer("iotkit")
    .model("ESP32-C6-DevKitC-1")
    .product_name("iotkit thread light")
    .entities(&[&LED]);

/// The board's WS2812 LED on GPIO8, driven by an RMT channel.
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit::init!(p);

    let board = Board {
        radio: p.IEEE802154,
        bt: p.BT,
        rng: p.RNG,
        adc1: p.ADC1,
        timg1: p.TIMG1,
        boot_button: Some(p.GPIO9.into()),
    };
    iotkit::start(spawner, board, &CONFIG).await;

    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, p.GPIO8, Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led).unwrap());

    loop {
        Timer::after_secs(3600).await;
    }
}

/// The status LED, as a light over Matter; a short press of BOOT toggles it.
#[embassy_executor::task]
async fn led_task(mut led: Led) {
    let mut state = LightState { on: true, brightness: 20, rgb: [0, 128, 255] };
    loop {
        LED.set(state);
        let color = if state.on { RGB8::new(state.rgb[0], state.rgb[1], state.rgb[2]) } else { RGB8::default() };
        if let Err(e) = led.write(brightness([color].into_iter(), state.brightness)).await {
            warn!("LED write failed: {e:?}");
        }
        state = match select(LED.wait_command(), iotkit::provision::gesture::wait_press()).await {
            Either::First(wanted) => wanted,
            Either::Second(()) => LightState { on: !state.on, ..state },
        };
    }
}
