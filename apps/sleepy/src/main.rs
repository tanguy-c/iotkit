//! A deep-sleeping sensor on the ESP32-C6-DevKitC-1: wakes up every minute, sends the chip
//! temperature and the number of wake-ups over MQTT, then sleeps.
//!
//! "Stay awake" (a switch in Home Assistant, retained) keeps it awake for maintenance: the USB
//! console, OTA pushes. Updates from Home Assistant install at the next wake-up.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;
use esp_hal::tsens::{self, TemperatureSensor};
use iotkit::{Board, Config, DeviceClass, PublicKey, Sensor, SleepConfig, Switch, Unit};
use log::info;

static OTA_KEY: PublicKey = iotkit::public_key(include_str!("../../../keys/ota.pub"));

static TEMPERATURE: Sensor = Sensor::new("chip_temperature", "Chip temperature")
    .unit(Unit::CELSIUS)
    .device_class(DeviceClass::TEMPERATURE)
    .measurement()
    .precision(1);
static WAKEUPS: Sensor = Sensor::new("wakeups", "Wake-ups").measurement().diagnostic();
static STAY_AWAKE: Switch = Switch::new("stay_awake", "Stay awake").icon("mdi:sleep-off").config();
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
    .model("ESP32-C6-DevKitC-1")
    .product_name("iotkit sleepy")
    .entities(&[&TEMPERATURE, &WAKEUPS, &STAY_AWAKE]);

static SLEEP: SleepConfig = SleepConfig { interval: Duration::from_secs(60), max_awake: Duration::from_secs(30) };

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit::init!(p);

    // Measured first: before the radio warms the chip.
    let tsens = TemperatureSensor::new(p.TSENS, tsens::Config::default()).unwrap();
    let celsius = tsens.get_temperature().to_celsius();

    let board = Board { wifi: p.WIFI, bt: p.BT, timg1: p.TIMG1, usb: p.USB_DEVICE, boot_button: Some(p.GPIO9.into()) };
    let app = iotkit::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;
    TEMPERATURE.set((celsius * 10.0 + 0.5f32.copysign(celsius)) as i32 as f32 / 10.0);
    WAKEUPS.set(app.boots as f32);
    // Until Home Assistant says otherwise (its retained command comes right after connecting).
    STAY_AWAKE.set(false);
    spawner.spawn(stay_awake_task().unwrap());

    app.settled().await;
    while STAY_AWAKE.get() == Some(true) {
        info!("staying awake (\"Stay awake\" is on)");
        embassy_time::Timer::after_secs(5).await;
    }
    app.sleep().await
}

#[embassy_executor::task]
async fn stay_awake_task() {
    loop {
        let on = STAY_AWAKE.wait_command().await;
        STAY_AWAKE.set(on);
    }
}
