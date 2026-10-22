//! {{project-name}}: the {% if chip == "esp32c6" %}ESP32-C6-DevKitC-1{% else %}DevKit{% endif %}'s RGB LED as a light in Home Assistant (MQTT{% if chip == "esp32c6" %}, and
//! Matter with `--features matter-wifi`{% endif %}), toggled by a short press of BOOT.
//!
//! `iotkit::start` brings the rest: Wi-Fi setup (access point, Improv), signed updates
//! (`cargo iotkit ota push`), crash records, the watchdog, the USB console (`help`).

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Ticker};
use esp_hal::{Async, rmt::Rmt, time::Rate};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use iotkit::{Board, Config, DeviceClass, Light, LightState, PublicKey, Sensor, Unit};
use log::warn;
use smart_leds::{RGB8, SmartLedsWriteAsync, brightness};

/// Updates must be signed with the secret half (`cargo iotkit keygen`).
static OTA_KEY: PublicKey = iotkit::public_key(include_str!("../keys/ota.pub"));

static LED: Light = Light::new("led", "LED");
static UPTIME: Sensor = Sensor::new("uptime", "Uptime").unit(Unit::SECOND).device_class(DeviceClass::DURATION).diagnostic();

static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
    .manufacturer("Me")
    .model("{% if chip == "esp32c6" %}ESP32-C6-DevKitC-1{% elsif chip == "esp32c3" %}ESP32-C3-DevKitM-1{% elsif chip == "esp32c5" %}ESP32-C5-DevKitC-1{% elsif chip == "esp32" %}ESP32-DevKitC{% else %}ESP32-S3-DevKitC-1{% endif %}")
    .product_name("{{project-name}}")
    .entities(&[&LED, &UPTIME]);

/// {% if chip == "esp32" %}A WS2812 LED wired to GPIO2 (the ESP32-DevKitC has none){% else %}The DevKit's WS2812 LED on {% if chip == "esp32s3" %}GPIO38{% elsif chip == "esp32c5" %}GPIO27{% else %}GPIO8{% endif %}{% endif %}, driven by an RMT channel.
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit::init!(p);

    let board = Board {
        wifi: p.WIFI,
{% if chip == "esp32" %}        timg1: p.TIMG1,
        uart: p.UART0,
        uart_rx: p.GPIO3.into(),
{% else %}        bt: p.BT,
        timg1: p.TIMG1,
        usb: p.USB_DEVICE,
{% endif %}
        boot_button: Some(p.{% if chip == "esp32s3" or chip == "esp32" %}GPIO0{% elsif chip == "esp32c5" %}GPIO28{% else %}GPIO9{% endif %}.into()),
    };
    iotkit::start(spawner, board, &CONFIG).await;

    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, p.{% if chip == "esp32s3" %}GPIO38{% elsif chip == "esp32c5" %}GPIO27{% elsif chip == "esp32" %}GPIO2{% else %}GPIO8{% endif %}, Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led).unwrap());

    let mut ticker = Ticker::every(Duration::from_secs(60));
    loop {
        UPTIME.set(Instant::now().as_secs() as f32);
        ticker.next().await;
    }
}

/// Applies commands (Home Assistant, Matter, BOOT) to the LED and reports its state.
#[embassy_executor::task]
async fn led_task(mut led: Led) {
    let mut state = LightState { on: true, brightness: 20, rgb: [0, 128, 255] };
    loop {
        LED.set(state);
        let color = if state.on { RGB8::new(state.rgb[0], state.rgb[1], state.rgb[2]) } else { RGB8::default() };
        if let Err(e) = led.write(brightness([color].into_iter(), state.brightness)).await {
            warn!("LED write failed: {e:?}");
        }
        state = match select(LED.wait_command(), iotkit::provision::esp::wait_press()).await {
            Either::First(wanted) => wanted,
            Either::Second(()) => LightState { on: !state.on, ..state },
        };
    }
}
