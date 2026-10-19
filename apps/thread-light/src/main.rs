//! Matter over Thread on the ESP32-C6-DevKitC-1: the RGB LED as a colour light and the chip
//! temperature. Pair it from a Matter controller with the code in the boot log (over
//! Bluetooth); it then joins the controller's Thread network. Needs a Thread border router.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use esp_hal::{Async, clock::CpuClock, rmt::Rmt, time::Rate, timer::timg::TimerGroup};
// The chip temperature sensor: on the ESP32-C6 only (the H2 has none).
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use iotkit_config::esp::SharedStore;
use iotkit_link_matter::thread::Product;
use iotkit_model::{Device, Entity, Light, LightState};
use log::{error, info, warn};
use smart_leds::{RGB8, SmartLedsWriteAsync, brightness};
use static_cell::StaticCell;
#[cfg(feature = "esp32c6")]
use {
    embassy_futures::select::{Either, select},
    embassy_time::{Duration, Ticker},
    esp_hal::tsens::{self, TemperatureSensor},
    iotkit_model::Sensor,
};

esp_bootloader_esp_idf::esp_app_desc!();

static LED: Light = Light::new("led", "LED");
#[cfg(feature = "esp32c6")]
static TEMPERATURE: Sensor = Sensor::new("chip_temperature", "Chip temperature")
    .unit("°C")
    .device_class("temperature")
    .measurement()
    .precision(1);
#[cfg(feature = "esp32c6")]
static ENTITIES: [&dyn Entity; 2] = [&LED, &TEMPERATURE];
#[cfg(not(feature = "esp32c6"))]
static ENTITIES: [&dyn Entity; 1] = [&LED];

type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let boot = iotkit_diag::esp::boot();
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // OpenThread, Bluetooth and Matter allocate.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 96 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    info!("{} {} booted, reset cause: {:?}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), boot.cause);
    if let Some(crash) = &boot.crash {
        error!("previous run crashed: {crash}");
    }

    iotkit_config::esp::init(peripherals.FLASH);
    static STORE: StaticCell<SharedStore> = StaticCell::new();
    let store = match iotkit_config::esp::open().await {
        Ok(store) => &*STORE.init(Mutex::new(store)),
        Err(e) => panic!("config: {e}"),
    };
    if let Err(e) = iotkit_diag::record_boot(&mut *store.lock().await, &boot).await {
        warn!("could not record the boot: {e}");
    }

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, peripherals.GPIO8, Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led).unwrap());
    #[cfg(feature = "esp32c6")]
    {
        let tsens = TemperatureSensor::new(peripherals.TSENS, tsens::Config::default()).unwrap();
        spawner.spawn(temperature_task(tsens).unwrap());
    }

    static NAME: StaticCell<heapless::String<16>> = StaticCell::new();
    let mac: [u8; 6] = esp_hal::efuse::base_mac_address().as_bytes().try_into().unwrap_or_default();
    let name = NAME.init(iotkit_net::hostname(mac)).as_str();
    static DEVICE: StaticCell<Device> = StaticCell::new();
    let device = &*DEVICE.init(Device {
        name,
        manufacturer: "iotkit",
        model: "ESP32-C6-DevKitC-1",
        sw_version: env!("CARGO_PKG_VERSION"),
        entities: &ENTITIES,
    });
    let product = Product { name: "iotkit thread light", version: env!("CARGO_PKG_VERSION"), version_number: 1 };
    spawner.spawn(
        iotkit_link_matter::thread::matter_task(
            iotkit_link_matter::thread::Peripherals {
                radio: peripherals.IEEE802154,
                bt: peripherals.BT,
                rng: peripherals.RNG,
                adc1: peripherals.ADC1,
            },
            store,
            product,
            name,
            device,
        )
        .unwrap(),
    );

    loop {
        embassy_time::Timer::after_secs(3600).await;
    }
}

#[embassy_executor::task]
async fn led_task(mut led: Led) {
    LED.set(LightState { on: true, brightness: 20, rgb: [0, 0, 255] });
    loop {
        let l = LED.get().unwrap_or_default();
        let color = if l.on { RGB8::new(l.rgb[0], l.rgb[1], l.rgb[2]) } else { RGB8::default() };
        if let Err(e) = led.write(brightness([color].into_iter(), l.brightness)).await {
            warn!("LED write failed: {e:?}");
        }
        LED.set(LED.wait_command().await);
    }
}

#[cfg(feature = "esp32c6")]
#[embassy_executor::task]
async fn temperature_task(tsens: TemperatureSensor<'static>) {
    let mut ticker = Ticker::every(Duration::from_secs(30));
    loop {
        let celsius = tsens.get_temperature().to_celsius();
        TEMPERATURE.set((celsius * 10.0 + 0.5f32.copysign(celsius)) as i32 as f32 / 10.0);
        if let Either::Second(()) = select(ticker.next(), core::future::pending::<()>()).await {}
    }
}
