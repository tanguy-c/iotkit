//! A chip-temperature sensor over MQTT with only what it uses: `iotkit-app` without its default
//! features but `improv-serial` (Wi-Fi setup from a browser over USB) and `ota-push`. No
//! Bluetooth stack, setup access point or console commands: about 300 KB less flash than devkit.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Ticker};
use esp_hal::tsens::{self, TemperatureSensor};
use iotkit_app::{Board, Config, PublicKey, Sensor};

static OTA_KEY: PublicKey = iotkit_app::public_key(include_str!("../../../keys/ota.pub"));

static TEMPERATURE: Sensor = Sensor::temperature("chip_temperature", "Chip temperature");

static CONFIG: Config = Config::new(iotkit_app::firmware!(&OTA_KEY))
    .model("ESP32-C6-DevKitC-1")
    .product_name("iotkit lean")
    .entities(&[&TEMPERATURE]);

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit_app::init!(p);
    // No `bt` (improv-ble off); `usb` stays for Improv over serial.
    let board = Board { flash: p.FLASH, wifi: p.WIFI, timg1: p.TIMG1, usb: p.USB_DEVICE, boot_button: None };
    iotkit_app::start(spawner, board, &CONFIG).await;

    let tsens = TemperatureSensor::new(p.TSENS, tsens::Config::default()).unwrap();
    let mut ticker = Ticker::every(Duration::from_secs(30));
    loop {
        TEMPERATURE.set(tsens.get_temperature().to_celsius());
        ticker.next().await;
    }
}
