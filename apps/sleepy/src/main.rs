//! A deep-sleeping sensor on the ESP32-C6-DevKitC-1: wakes up every minute, sends the chip
//! temperature and the number of wake-ups over MQTT (or the ESPHome API, feature `esphome-api`),
//! then sleeps.
//!
//! Over MQTT, "Stay awake" (a switch in Home Assistant, retained by the broker) keeps it awake
//! for maintenance: the USB console, OTA pushes; updates from Home Assistant install at the next
//! wake-up. Over the ESPHome API nothing holds a command while it sleeps, so there is no such
//! switch: hold the button 3 s while it is awake (pairing mode, 10 minutes). The RGB LED
//! shows the pairing family (`iotkit::status`): blue blinking while pairing, dark otherwise.
//!
//! BOOT (GPIO9) cannot wake the chip: a button from GPIO0-7 to ground can (`boot_button`), a
//! short press then wakes it for a report, a 3 s hold in pairing mode.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;
use esp_hal::{
    Async,
    rmt::Rmt,
    time::Rate,
    tsens::{self, TemperatureSensor},
};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
#[cfg(not(feature = "esphome-api"))]
use iotkit::Switch;
use iotkit::{Board, Config, DeviceClass, PublicKey, Sensor, SleepConfig, Unit};
#[cfg(not(feature = "esphome-api"))]
use log::info;
use smart_leds::RGB8;

static OTA_KEY: PublicKey = iotkit::public_key(include_str!("../../../keys/ota.pub"));

static TEMPERATURE: Sensor = Sensor::new("chip_temperature", "Chip temperature")
    .unit(Unit::CELSIUS)
    .device_class(DeviceClass::TEMPERATURE)
    .measurement()
    .precision(1);
static WAKEUPS: Sensor = Sensor::new("wakeups", "Wake-ups").measurement().diagnostic();
#[cfg(not(feature = "esphome-api"))]
static STAY_AWAKE: Switch = Switch::new("stay_awake", "Stay awake").icon("mdi:sleep-off").config();
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
    .model("ESP32-C6-DevKitC-1")
    .product_name("iotkit sleepy")
    .entities(ENTITIES);
#[cfg(not(feature = "esphome-api"))]
static ENTITIES: &[&dyn iotkit::Entity] = &[&TEMPERATURE, &WAKEUPS, &STAY_AWAKE];
#[cfg(feature = "esphome-api")]
static ENTITIES: &[&dyn iotkit::Entity] = &[&TEMPERATURE, &WAKEUPS];

static SLEEP: SleepConfig = SleepConfig { interval: Duration::from_secs(60), max_awake: Duration::from_secs(30) };

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit::init!(p);

    // Measured first: before the radio warms the chip.
    let tsens = TemperatureSensor::new(p.TSENS, tsens::Config::default()).unwrap();
    let celsius = tsens.get_temperature().to_celsius();

    let board = Board { wifi: p.WIFI, bt: p.BT, timg1: p.TIMG1, usb: p.USB_DEVICE, boot_button: Some(p.GPIO9.into()) };
    let app = iotkit::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;
    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async();
    spawner.spawn(led_task(Led::new(WS2812_TIMING, rmt.channel0, p.GPIO8, Rate::from_mhz(80)).unwrap()).unwrap());
    TEMPERATURE.set((celsius * 10.0 + 0.5f32.copysign(celsius)) as i32 as f32 / 10.0);
    WAKEUPS.set(app.boots as f32);
    // Until Home Assistant says otherwise (its retained command comes right after connecting).
    #[cfg(not(feature = "esphome-api"))]
    {
        STAY_AWAKE.set(false);
        spawner.spawn(stay_awake_task().unwrap());
    }

    app.settled().await;
    #[cfg(not(feature = "esphome-api"))]
    while STAY_AWAKE.get() == Some(true) {
        info!("staying awake (\"Stay awake\" is on)");
        embassy_time::Timer::after_secs(5).await;
    }
    app.sleep().await
}

type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

#[embassy_executor::task]
async fn led_task(led: Led) {
    // Lit only to set it up: no blink at each wake-up.
    iotkit::components::status_rgb(led, 1, 32, iotkit::status::Show::Pairing).await
}

#[cfg(not(feature = "esphome-api"))]
#[embassy_executor::task]
async fn stay_awake_task() {
    loop {
        let on = STAY_AWAKE.wait_command().await;
        STAY_AWAKE.set(on);
    }
}
