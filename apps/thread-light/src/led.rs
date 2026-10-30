//! The board's RGB LED, a light; the status shows over it (`iotkit::status`).

use embassy_executor::Spawner;
use esp_hal::{
    Async,
    peripherals::{GPIO8, RMT},
    rmt::Rmt,
    time::Rate,
};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use iotkit::Light;
use smart_leds::RGB8;

pub static LED: Light = Light::new("status_led", "Status LED").remember();

/// The board's WS2812 LED on GPIO8, driven by an RMT channel.
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

/// Starts it (from `main`).
pub fn spawn_rgb(spawner: Spawner, rmt: RMT<'static>, pin: GPIO8<'static>) {
    let rmt = Rmt::new(rmt, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, pin, Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led).unwrap());
}

/// The status LED, as a light over Matter; a short press of BOOT toggles it.
/// The status shows over it (`iotkit::status`): blue blinking while it waits to be set up.
#[embassy_executor::task]
async fn led_task(led: Led) {
    // `Show::All` while developing; `Show::Pairing` for a release: only to set it up.
    iotkit::components::rgb_status_light(&LED, led, 1, iotkit::status::Show::All, true).await
}
