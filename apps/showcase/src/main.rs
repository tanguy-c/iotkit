//! One entity of each kind, simulated on an ESP32-C6-DevKitC-1: to check how they show up and
//! behave in Home Assistant (MQTT, and Matter with `--features matter-wifi`).
//!
//! The blind and the valve move 10 % a second, the thermostat heats or cools the room by
//! 0.1 °C a second, the lock takes a second, and a short press of BOOT triggers the button event
//! (`press`; three in a row: `triple_press`).

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_hal::{clock::CpuClock, timer::timg::TimerGroup};
use iotkit_app::{
    BinarySensor, Board, Climate, ClimateState, Config, Cover, CoverCommand, CoverState, Entity, Event, Fan, FanState,
    HvacAction, HvacMode, Lock, LockCommand, LockState, Number, PublicKey, Select, Sensor, TextSensor, Valve,
};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

static OTA_KEY: PublicKey = iotkit_app::public_key(include_str!("../../../keys/ota.pub"));

static BLIND: Cover = Cover::new("blind", "Blind").device_class("blind").position();
static VALVE: Valve = Valve::new("water_valve", "Water valve").device_class("water").position();
static DOOR: Lock = Lock::new("door", "Front door");
static FAN: Fan = Fan::new("fan", "Fan").speed().presets(&["sleep", "breeze"]).oscillation().direction();
static THERMOSTAT: Climate =
    Climate::new("thermostat", "Thermostat", &[HvacMode::Heat, HvacMode::Cool, HvacMode::HeatCool, HvacMode::Off]);
static SETPOINT: Number = Number::new("brightness_limit", "Brightness limit", 0.0, 100.0, 5.0).unit("%").slider();
static MODE: Select = Select::new("scene", "Scene", &["day", "evening", "night"]);
static STATUS: TextSensor = TextSensor::new("status", "Status");
static BUTTON: Event = Event::new("button", "Button", &["press", "triple_press"]).device_class("button");
static HUMIDITY: Sensor =
    Sensor::new("humidity", "Humidity").unit("%").device_class("humidity").measurement().precision(1);
static PRESSURE: Sensor =
    Sensor::new("pressure", "Pressure").unit("hPa").device_class("pressure").measurement().precision(1);
static LUX: Sensor = Sensor::new("illuminance", "Illuminance").unit("lx").device_class("illuminance").measurement();
static CO2: Sensor = Sensor::new("co2", "CO2").unit("ppm").device_class("carbon_dioxide").measurement();
static BATTERY: Sensor = Sensor::new("battery", "Battery").unit("%").device_class("battery").measurement();
static MOTION: BinarySensor = BinarySensor::new("motion", "Motion").device_class("occupancy");
static LEAK: BinarySensor = BinarySensor::new("leak", "Leak").device_class("moisture");
static SMOKE: BinarySensor = BinarySensor::new("smoke", "Smoke").device_class("smoke");

static ENTITIES: [&dyn Entity; 17] = [
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

static CONFIG: Config = Config {
    firmware: iotkit_app::firmware!(&OTA_KEY),
    manufacturer: "iotkit",
    model: "Showcase",
    chip: "ESP32-C6",
    product_name: "iotkit showcase",
    entities: &ENTITIES,
    watches: &[],
    checks: &[],
    console: None,
};

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 96 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);

    let board = Board {
        flash: p.FLASH,
        wifi: p.WIFI,
        bt: p.BT,
        timg1: p.TIMG1,
        usb: p.USB_DEVICE,
        boot_button: Some(p.GPIO9.into()),
    };
    iotkit_app::start(spawner, board, &CONFIG).await;

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
