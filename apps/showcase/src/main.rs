//! One entity of each kind, simulated on an ESP32-C6-DevKitC-1: to check how they show up and
//! behave in Home Assistant (MQTT, and Matter with `--features matter-wifi`).
//!
//! The blind and the valve move 10 % a second, the thermostat heats or cools the room by
//! 0.1 °C a second, the lock takes a second, and a short press of BOOT triggers the button event
//! (`press`; three in a row: `triple_press`). The board's RGB LED is a real light, driven by
//! `components::rgb_light`.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_hal::{Async, rmt::Rmt, time::Rate};
use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};
use iotkit_app::{
    BinarySensor, Board, Climate, ClimateState, Config, Cover, CoverCommand, CoverState, DeviceClass, Entity, Event,
    Fan, FanState, HvacAction, HvacMode, Light, Lock, LockCommand, LockState, Number, PublicKey, Select, Sensor,
    TextSensor, Unit, Valve,
};
use log::info;
use smart_leds::RGB8;

static OTA_KEY: PublicKey = iotkit_app::public_key(include_str!("../../../keys/ota.pub"));

static BLIND: Cover = Cover::new("blind", "Blind").device_class(DeviceClass::BLIND).position();
static VALVE: Valve = Valve::new("water_valve", "Water valve").device_class(DeviceClass::WATER).position();
static DOOR: Lock = Lock::new("door", "Front door");
static FAN: Fan = Fan::new("fan", "Fan").speed().presets(&["sleep", "breeze"]).oscillation().direction();
static THERMOSTAT: Climate =
    Climate::new("thermostat", "Thermostat", &[HvacMode::Heat, HvacMode::Cool, HvacMode::HeatCool, HvacMode::Off]);
static SETPOINT: Number =
    Number::new("brightness_limit", "Brightness limit", 0.0, 100.0, 5.0).unit(Unit::PERCENT).slider();
static MODE: Select = Select::new("scene", "Scene", &["day", "evening", "night"]);
static STATUS: TextSensor = TextSensor::new("status", "Status");
static BUTTON: Event = Event::new("button", "Button", &["press", "triple_press"]).device_class(DeviceClass::BUTTON);
static HUMIDITY: Sensor = Sensor::humidity("humidity", "Humidity");
static PRESSURE: Sensor = Sensor::pressure("pressure", "Pressure");
static LUX: Sensor = Sensor::illuminance("illuminance", "Illuminance");
static CO2: Sensor = Sensor::carbon_dioxide("co2", "CO2");
static BATTERY: Sensor = Sensor::battery("battery", "Battery");
static MOTION: BinarySensor = BinarySensor::new("motion", "Motion").device_class(DeviceClass::OCCUPANCY);
static LEAK: BinarySensor = BinarySensor::new("leak", "Leak").device_class(DeviceClass::MOISTURE);
static SMOKE: BinarySensor = BinarySensor::new("smoke", "Smoke").device_class(DeviceClass::SMOKE);

static LIGHT: Light = Light::new("light", "Light");

static ENTITIES: &[&dyn Entity] = &[
    &LIGHT,
    &BLIND,
    &VALVE,
    &DOOR,
    &FAN,
    &THERMOSTAT,
    &SETPOINT,
    &MODE,
    &STATUS,
    &BUTTON,
    &HUMIDITY,
    &PRESSURE,
    &LUX,
    &CO2,
    &BATTERY,
    &MOTION,
    &LEAK,
    &SMOKE,
];

static CONFIG: Config =
    Config::new(iotkit_app::firmware!(&OTA_KEY)).model("Showcase").product_name("iotkit showcase").entities(ENTITIES);

/// The board's WS2812 LED on GPIO8.
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

#[embassy_executor::task]
async fn light_task(led: Led) {
    iotkit_app::components::rgb_light(&LIGHT, led, 1).await
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    iotkit_app::init!(p);

    let board = Board {
        flash: p.FLASH,
        wifi: p.WIFI,
        bt: p.BT,
        timg1: p.TIMG1,
        usb: p.USB_DEVICE,
        boot_button: Some(p.GPIO9.into()),
    };
    iotkit_app::start(spawner, board, &CONFIG).await;

    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, p.GPIO8, Rate::from_mhz(80)).unwrap();
    spawner.spawn(light_task(led).unwrap());

    STATUS.set("starting");
    spawner.spawn(motion_task().unwrap());
    spawner.spawn(lock_task().unwrap());
    spawner.spawn(fan_task().unwrap());
    spawner.spawn(thermostat_task().unwrap());
    spawner.spawn(settings_task().unwrap());
    spawner.spawn(button_task().unwrap());

    // Slowly changing readings.
    let mut ticker = Ticker::every(Duration::from_secs(10));
    let mut t = 0u32;
    loop {
        let wave = (t % 20) as f32 / 20.0;
        HUMIDITY.set(40.0 + 20.0 * wave);
        PRESSURE.set(1013.0 + 5.0 * wave);
        LUX.set(100.0 + 900.0 * wave);
        CO2.set(450.0 + 800.0 * wave);
        BATTERY.set(100.0 - (t % 100) as f32);
        MOTION.set(t.is_multiple_of(3));
        LEAK.set(false);
        SMOKE.set(false);
        STATUS.set(if t.is_multiple_of(2) { "idle" } else { "measuring" });
        t += 1;
        ticker.next().await;
    }
}

/// The blind and the valve, 10 % a second towards their target.
#[embassy_executor::task]
async fn motion_task() {
    let (mut blind, mut blind_to) = (0u8, 0u8);
    let (mut valve, mut valve_to) = (0u8, 0u8);
    BLIND.set(Some(CoverState::Closed), Some(0));
    VALVE.set(Some(CoverState::Closed), Some(0));
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        match select(select(BLIND.wait_command(), VALVE.wait_command()), ticker.next()).await {
            Either::First(Either::First(c)) => blind_to = target(c, blind),
            Either::First(Either::Second(c)) => valve_to = target(c, valve),
            Either::Second(()) => {
                blind = step(blind, blind_to);
                valve = step(valve, valve_to);
            }
        }
        BLIND.set(Some(moving(blind, blind_to)), Some(blind));
        VALVE.set(Some(moving(valve, valve_to)), Some(valve));
    }
}

fn target(c: CoverCommand, at: u8) -> u8 {
    match c {
        CoverCommand::Open => 100,
        CoverCommand::Close => 0,
        CoverCommand::Stop => at,
        CoverCommand::Position(p) => p,
    }
}

fn step(at: u8, to: u8) -> u8 {
    if at < to { (at + 10).min(to) } else { at.saturating_sub(10).max(to) }
}

fn moving(at: u8, to: u8) -> CoverState {
    match (at, to) {
        (a, t) if a < t => CoverState::Opening,
        (a, t) if a > t => CoverState::Closing,
        (0, _) => CoverState::Closed,
        (100, _) => CoverState::Open,
        _ => CoverState::Stopped,
    }
}

#[embassy_executor::task]
async fn lock_task() {
    DOOR.set(LockState::Locked);
    loop {
        let c = DOOR.wait_command().await;
        let (during, after) = match c {
            LockCommand::Lock => (LockState::Locking, LockState::Locked),
            LockCommand::Unlock | LockCommand::Open => (LockState::Unlocking, LockState::Unlocked),
        };
        DOOR.set(during);
        Timer::after_secs(1).await;
        DOOR.set(after);
        info!("door: {}", after.as_str());
    }
}

#[embassy_executor::task]
async fn fan_task() {
    FAN.set(FanState::OFF);
    loop {
        let wanted = FAN.wait_command().await;
        FAN.set(wanted);
    }
}

/// The room warms up or cools down by 0.1 °C a second towards the target.
#[embassy_executor::task]
async fn thermostat_task() {
    let mut state = ClimateState { mode: HvacMode::Heat, target: 21.0, current: Some(19.0), action: None };
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        if let Either::First(wanted) = select(THERMOSTAT.wait_command(), ticker.next()).await {
            state.mode = wanted.mode;
            state.target = wanted.target;
        }
        let room = state.current.unwrap_or(19.0);
        let (action, room) = match state.mode {
            HvacMode::Heat | HvacMode::HeatCool if room < state.target - 0.05 => (HvacAction::Heating, room + 0.1),
            HvacMode::Cool | HvacMode::HeatCool if room > state.target + 0.05 => (HvacAction::Cooling, room - 0.1),
            HvacMode::Off => (HvacAction::Off, room),
            _ => (HvacAction::Idle, room),
        };
        // Rounded to 0.1: no publish for noise.
        state.current = Some((room * 10.0 + 0.5) as i32 as f32 / 10.0);
        state.action = Some(action);
        THERMOSTAT.set(state);
    }
}

#[embassy_executor::task]
async fn settings_task() {
    SETPOINT.set(80.0);
    MODE.set(0);
    loop {
        match select(SETPOINT.wait_command(), MODE.wait_command()).await {
            Either::First(v) => SETPOINT.set(v),
            Either::Second(i) => {
                MODE.set(i);
                info!("scene: {}", MODE.option().unwrap_or("?"));
            }
        }
    }
}

/// A short press of BOOT: `press`; three within two seconds: `triple_press`.
#[embassy_executor::task]
async fn button_task() {
    let mut presses: [Instant; 3] = [Instant::MIN; 3];
    loop {
        iotkit_app::provision::esp::wait_press().await;
        presses.rotate_left(1);
        presses[2] = Instant::now();
        if presses[2] - presses[0] < Duration::from_secs(2) {
            BUTTON.trigger(1);
            presses = [Instant::MIN; 3];
        } else {
            BUTTON.trigger(0);
        }
    }
}
