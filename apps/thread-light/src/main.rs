//! iotkit thread light: made with `cargo iotkit new --answers xtask/answers/thread.toml` (pairing
//! and updates: its README). Only the key's path differs: the workspace's `keys/`.
//!
//! `iotkit::start` brings Matter over Thread (paired over Bluetooth), signed updates over Matter
//! (rolled back unless it rejoins Thread and its controller), crash records and the watchdog.
//! No parts yet: `cargo iotkit add` adds one (`cargo iotkit components` lists them).

#![no_std]
#![no_main]

mod led;
mod parts;

use embassy_executor::Spawner;
use embassy_time::Timer;
use iotkit::{Board, Config, PublicKey};

/// Updates must be signed with the secret half (`cargo iotkit keygen`).
static OTA_KEY: PublicKey = iotkit::public_key(include_str!("../../../keys/ota.pub"));

static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
    .manufacturer("iotkit")
    .model("ESP32-C6-DevKitC-1")
    .product_name("iotkit thread light")
    .entities(&[
        // The parts' entities (`cargo iotkit add` adds them here).
        &led::LED,
    ]);

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

    // Written right after the bootloader; its saved state comes once `start` restores it.
    led::spawn_rgb(spawner, p.RMT, p.GPIO8);

    iotkit::start(spawner, board, &CONFIG).await;

    // The parts, one module each in src/parts/ (`cargo iotkit add` adds them here).

    loop {
        Timer::after_secs(3600).await;
    }
}
