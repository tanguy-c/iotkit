//! The files `cargo iotkit new` writes from the answers: `src/main.rs`, `README.md`,
//! `.cargo/secrets.toml`, and the features in `Cargo.toml`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

mod part;

use part::Uses;
pub use part::{Call, part, ready};

use super::{Indicator, MatterMode, Project, SetupWay};

/// The comments `cargo iotkit add` finds in `src/main.rs`, each above the lines it adds to.
pub const ENTITIES_MARK: &str = "// The parts' entities (`cargo iotkit add` adds them here).";
pub const SPAWNS_MARK: &str = "// The parts, one module each in src/parts/ (`cargo iotkit add` adds them here).";
pub const READS_MARK: &str = "// Read once per wake-up (`cargo iotkit add` adds them here).";

/// `src/parts/mod.rs`'s head.
pub const PARTS_MOD: &str = "//! The parts, one module each: `cargo iotkit add <component>` adds one (`cargo iotkit components`\n//! lists them).\n";

/// The project's Rust files: `src/main.rs`, the board's LED (`src/led.rs`) and a module per part
/// (`src/parts/`).
pub fn sources(p: &Project) -> anyhow::Result<Vec<(String, String)>> {
    let mut parts = Vec::new();
    let mut entities = Vec::new();
    let (mut spawns, mut reads, mut mods) = (String::new(), String::new(), String::new());
    for (i, part) in p.parts.iter().enumerate() {
        let code = part::part(p, i)?;
        entities.extend(code.entities);
        match code.call {
            Call::Spawn(line) => spawns += &format!("    {line}\n"),
            Call::Read(line) => reads += &format!("    {line}\n"),
        }
        let _ = writeln!(mods, "pub mod {};", part.module);
        parts.push((format!("src/parts/{}.rs", part.module), code.file));
    }
    let mut files = vec![("src/main.rs".to_string(), main_rs(p, &entities, &spawns, &reads))];
    if let Some(led) = led_rs(p) {
        files.push(("src/led.rs".into(), led));
    }
    if let Some(i2c) = i2c_rs(p) {
        files.push(("src/i2c.rs".into(), i2c));
    }
    let sep = if mods.is_empty() { "" } else { "\n" };
    files.push(("src/parts/mod.rs".into(), format!("{PARTS_MOD}{sep}{mods}")));
    files.extend(parts);
    Ok(files)
}

/// `src/i2c.rs`: the I²C bus the I²C parts share, when one is on it and its pins are chosen.
/// Plugged in, behind a mutex (each part's task has a `Device` on it); on battery, the parts are
/// read one after the other in `main`, which lends them the bus.
pub fn i2c_rs(p: &Project) -> Option<String> {
    let bus = p.i2c()?;
    let (sda, scl) = (bus.sda, bus.scl);
    let head = format!(
        "//! The I²C bus, SDA on GPIO{sda} and SCL on GPIO{scl} (most breakout boards have its pull-ups), at 100 kHz:\n"
    );
    Some(wrap_comments(&if p.sleepy() {
        format!(
            "{head}//! `main` lends it to each I²C part in turn, once per wake-up.

use esp_hal::{{
    Async,
    i2c::master::{{Config, I2c}},
    peripherals::{{GPIO{sda}, GPIO{scl}, I2C0}},
}};

pub type Bus = I2c<'static, Async>;

/// The bus (from `main`).
pub fn bus(i2c0: I2C0<'static>, sda: GPIO{sda}<'static>, scl: GPIO{scl}<'static>) -> Bus {{
    I2c::new(i2c0, Config::default()).unwrap().with_sda(sda).with_scl(scl).into_async()
}}
"
        )
    } else {
        format!(
            "{head}//! each I²C part's task has a `Device` on it, which takes the bus for one transfer at a time.

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::{{blocking_mutex::raw::NoopRawMutex, mutex::Mutex}};
use esp_hal::{{
    Async,
    i2c::master::{{Config, I2c}},
    peripherals::{{GPIO{sda}, GPIO{scl}, I2C0}},
}};
use static_cell::StaticCell;

/// The bus; its parts' tasks run on `main`'s executor (`NoopRawMutex`).
pub type Bus = Mutex<NoopRawMutex, I2c<'static, Async>>;
/// A part's handle on the bus.
pub type Device = I2cDevice<'static, NoopRawMutex, I2c<'static, Async>>;

/// The bus (from `main`, once).
pub fn bus(i2c0: I2C0<'static>, sda: GPIO{sda}<'static>, scl: GPIO{scl}<'static>) -> &'static Bus {{
    static BUS: StaticCell<Bus> = StaticCell::new();
    BUS.init(Mutex::new(I2c::new(i2c0, Config::default()).unwrap().with_sda(sda).with_scl(scl).into_async()))
}}
"
        )
    }))
}

/// `main`'s line that creates the I²C bus, above the parts that take it.
pub fn bus_line(p: &Project) -> Option<String> {
    let bus = p.i2c()?;
    let m = if p.sleepy() { "mut " } else { "" };
    Some(format!("let {m}bus = i2c::bus(p.I2C0, p.GPIO{}, p.GPIO{});", bus.sda, bus.scl))
}

/// The dependencies the I²C bus needs (`Cargo.toml` lines): plugged in, its mutex.
pub fn bus_dependencies(p: &Project) -> &'static [&'static str] {
    if p.i2c().is_none() || p.sleepy() {
        return &[];
    }
    &[
        "embassy-embedded-hal   = { version = \"0.6\", default-features = false }",
        "embassy-sync           = \"0.8\"",
        "static_cell            = \"2.1\"",
    ]
}

/// `src/main.rs`: the setup, `iotkit::start`, and the parts' entities and spawns (each part's code
/// is in its module).
fn main_rs(p: &Project, entities: &[String], spawns: &str, reads: &str) -> String {
    let sleepy = p.sleepy();
    // The battery level, read once per wake-up (over Thread, the ADC feeds the TRNG: none).
    let battery_pin = sleepy.then_some(p.answers.battery_pin).flatten();
    let led = p.status_led();
    let status = p.answers.status;
    let mut uses = Uses::default();
    uses.extend(["embassy_executor::Spawner", "iotkit::Board", "iotkit::Config", "iotkit::PublicKey"]);
    if !p.board.init.is_empty() {
        uses.extend(["esp_hal::gpio::Level", "esp_hal::gpio::Output", "esp_hal::gpio::OutputConfig"]);
    }
    if battery_pin.is_some() {
        uses.extend(["iotkit::Sensor", "iotkit::battery::Battery", "iotkit::battery::BatteryCurve"]);
    }
    if sleepy {
        uses.extend(["iotkit::SleepConfig", "embassy_time::Duration"]);
    } else {
        uses.add("embassy_time::Timer");
    }

    let mut s = String::new();
    // Header.
    let _ = writeln!(s, "//! {}: made with `cargo iotkit new` (wiring and setup: README.md).", p.answers.name);
    let _ = writeln!(s, "//!");
    if sleepy {
        let _ = writeln!(
            s,
            "//! A battery device: it wakes up every {} min, reports, and sleeps (`iotkit::start_sleepy`).",
            p.answers.report_every_min
        );
    } else if p.thread() {
        s.push_str(
            "//! `iotkit::start` brings Matter over Thread (paired over Bluetooth), signed updates over Matter\n\
             //! (rolled back unless it rejoins Thread and its controller), crash records and the watchdog.\n",
        );
        if p.icd() {
            s.push_str(
                "//! On battery (`matter-icd`): a sleepy end device, whose radio polls its parent every few\n\
                 //! seconds; the chip sleeps in between.\n",
            );
        }
    } else {
        let _ = writeln!(
            s,
            "//! `iotkit::start` brings Wi-Fi setup (access point, Improv), Home Assistant, signed updates,"
        );
        let _ = writeln!(s, "//! crash records, the watchdog and the USB console (`help`).");
    }
    if p.parts.is_empty() {
        s.push_str("//! No parts yet: `cargo iotkit add` adds one (`cargo iotkit components` lists them).\n");
    } else {
        s.push_str(
            "//! Each part is a module in `src/parts/`: look for `TODO(<entity id>)`. The drivers named there are\n\
             //! `no_std` and embedded-hal 1.0; prefer their async mode.\n",
        );
    }
    s.push_str("\n#![no_std]\n#![no_main]\n\n");
    if p.i2c().is_some() {
        s.push_str("mod i2c;\n");
    }
    if led || status != Indicator::Off {
        s.push_str("mod led;\n");
    }
    s.push_str("mod parts;\n\n");
    s.push_str(&uses.lines());
    s.push_str(
        "
/// Updates must be signed with the secret half (`cargo iotkit keygen`).
static OTA_KEY: PublicKey = iotkit::public_key(include_str!(\"../keys/ota.pub\"));
",
    );

    // Entities and configuration.
    if battery_pin.is_some() {
        s.push_str("\nstatic BATTERY: Sensor = Sensor::battery(\"battery\", \"Battery\");\n");
    }
    let mut list = format!("        {ENTITIES_MARK}\n");
    for e in entities {
        let _ = writeln!(list, "        {e},");
    }
    if led {
        list.push_str("        &led::LED,\n");
    }
    if battery_pin.is_some() {
        list.push_str("        &BATTERY,\n");
    }
    let _ = writeln!(
        s,
        "
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
    .manufacturer({:?})
    .model({:?})
    .product_name({:?})
    .entities(&[
{list}    ]);",
        p.answers.maker, p.board.model, p.answers.name,
    );
    if sleepy {
        let _ = writeln!(
            s,
            "
/// How often it wakes up, and how long it may try to reach Home Assistant.
static SLEEP: SleepConfig = SleepConfig {{ interval: Duration::from_secs({}), max_awake: Duration::from_secs(30) }};",
            u64::from(p.answers.report_every_min) * 60
        );
    }

    // main. A part sharing the ADC with the battery reading needs `mut p` (to reborrow ADC1); a
    // ready-made one takes it after the battery reading.
    let shared_adc = battery_pin.is_some() && p.parts.iter().any(|part| part.kind.analog());
    let adc_taken = shared_adc
        && p.parts
            .iter()
            .zip(ready(p))
            .any(|(part, ready)| ready && part.kind.code.as_ref().is_some_and(|c| c.takes.contains(&"ADC1")));
    let _ = write!(
        s,
        "
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {{
    iotkit::init!({}p);
",
        if shared_adc { "mut " } else { "" }
    );
    // The board's own switches, before the radio starts (`boards/<id>.toml`).
    for i in &p.board.init {
        let _ = write!(
            s,
            "    // GPIO{}: {}; held for as long as `main` runs.\n    let _gpio{} = Output::new(p.GPIO{}, Level::{}, OutputConfig::default());\n",
            i.pin,
            i.why,
            i.pin,
            i.pin,
            if i.high { "High" } else { "Low" }
        );
    }
    if let Some(pin) = battery_pin {
        let curve = if p.answers.battery_curve == super::Curve::TwoAa { "ALKALINE_2AA" } else { "LIPO" };
        let (shared, adc) = match (shared_adc, adc_taken) {
            (true, false) => ("\n    // A part needs the ADC too: `p.ADC1.reborrow()` here.", "p.ADC1"),
            (true, true) => ("", "p.ADC1.reborrow()"),
            (false, _) => ("", "p.ADC1"),
        };
        let _ = write!(
            s,
            "
    // Before Wi-Fi starts: the radio's load lowers the voltage. 2.0: two equal resistors
    // between the battery and GPIO{pin} (TODO(battery): your divider's ratio).{shared}
    Battery::new({adc}, p.GPIO{pin}, 2.0).curve(BatteryCurve::{curve}).report(&BATTERY, None);
"
        );
    }
    let _ = write!(
        s,
        "
    let board = Board {{
{}        boot_button: Some(p.GPIO{}.into()),
    }};
",
        if p.thread() {
            "        radio: p.IEEE802154,\n        bt: p.BT,\n        rng: p.RNG,\n        adc1: p.ADC1,\n        timg1: p.TIMG1,\n"
                .to_string()
        } else {
            // Bluetooth for its setup; the USB port (on the ESP32 UART0) for the console or Improv.
            let setup = p.setup();
            let serial = p.answers.console || setup.contains(&SetupWay::Serial);
            let mut fields = String::from("        wifi: p.WIFI,\n");
            if setup.contains(&SetupWay::Bluetooth) {
                fields.push_str("        bt: p.BT,\n");
            }
            fields.push_str("        timg1: p.TIMG1,\n");
            match (serial, p.board.chip == "esp32") {
                (true, true) => fields.push_str("        uart: p.UART0,\n        uart_rx: p.GPIO3.into(),\n"),
                (true, false) => fields.push_str("        usb: p.USB_DEVICE,\n"),
                (false, _) => {}
            }
            fields
        },
        p.button_pin()
    );
    let rgb = p.board.rgb_led.unwrap_or_default();
    if led {
        // Before `start`, which keeps the CPU a while (Matter loads its state): until the first
        // write, the LED shows whatever a reset left on its data line.
        let _ = writeln!(
            s,
            "\n    // Written right after the bootloader; its saved state comes once `start` restores it.\n    led::spawn_rgb(spawner, p.RMT, p.GPIO{rgb});\n"
        );
    }
    if sleepy {
        s.push_str("    let app = iotkit::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;\n");
    } else {
        s.push_str("    iotkit::start(spawner, board, &CONFIG).await;\n");
    }
    if status == Indicator::Rgb && !led {
        let _ = writeln!(s, "    led::spawn_rgb(spawner, p.RMT, p.GPIO{rgb});");
    }
    if let (Indicator::Led, Some(pin)) = (status, p.answers.status_pin) {
        let _ = writeln!(s, "    led::spawn_status(spawner, p.GPIO{pin});");
    }
    let bus = bus_line(p).map(|l| format!("\n    {l}\n")).unwrap_or_default();
    let _ = write!(s, "{}\n    {SPAWNS_MARK}\n{spawns}", if sleepy { "" } else { &bus });
    if sleepy {
        let _ = write!(s, "{bus}\n    {READS_MARK}\n{reads}");
        s.push_str(
            "
    // Sends the states, waits for commands that came while asleep, then sleeps.
    app.settled().await;
    app.sleep().await
}
",
        );
    } else {
        s.push_str(
            "
    loop {
        Timer::after_secs(3600).await;
    }
}
",
        );
    }
    wrap_comments(&s)
}

/// `src/led.rs`: the board's RGB LED as a light, and what shows the status. `None`: neither.
fn led_rs(p: &Project) -> Option<String> {
    let led = p.status_led();
    let status = p.answers.status;
    // The RGB LED is a light: plain, or showing the status too (`rgb_status_light`).
    let plain_led = led && status != Indicator::Rgb;
    if !led && matches!(status, Indicator::Off | Indicator::Auto) {
        return None;
    }
    let link = if p.thread() { " over Matter" } else { " in Home Assistant" };
    let mut uses = Uses::default();
    uses.add("embassy_executor::Spawner");
    let mut s = String::from(if led {
        "//! The board's RGB LED, a light; the status shows over it (`iotkit::status`).\n"
    } else {
        "//! What shows the status (`iotkit::status`): pairing, connecting, updates.\n"
    });
    let mut items = String::new();
    if led {
        uses.add("iotkit::Light");
        // Kept across restarts (never on battery: no status LED there).
        items.push_str("\npub static LED: Light = Light::new(\"status_led\", \"Status LED\").remember();\n");
    }
    if let (true, Some(pin)) = (led || status == Indicator::Rgb, p.board.rgb_led) {
        uses.extend([
            "esp_hal::Async",
            "esp_hal::rmt::Rmt",
            "esp_hal::time::Rate",
            "esp_hal_smartled::RmtSmartLeds",
            "esp_hal_smartled::WS2812_TIMING",
            "esp_hal_smartled::buffer_size",
            "esp_hal_smartled::color_order",
            "smart_leds::RGB8",
            "esp_hal::peripherals::RMT",
        ]);
        uses.add(format!("esp_hal::peripherals::GPIO{pin}"));
        let task = if led { "led_task" } else { "status_task" };
        let _ = write!(
            items,
            "
/// The board's WS2812 LED on GPIO{pin}, driven by an RMT channel.
type Led = RmtSmartLeds<'static, {{ buffer_size::<RGB8>(1) }}, Async, RGB8, color_order::Grb>;

/// Starts it (from `main`).
pub fn spawn_rgb(spawner: Spawner, rmt: RMT<'static>, pin: GPIO{pin}<'static>) {{
    let rmt = Rmt::new(rmt, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, pin, Rate::from_mhz(80)).unwrap();
    spawner.spawn({task}(led).unwrap());
}}
"
        );
    }
    match (status, p.answers.status_pin) {
        (Indicator::Rgb, _) if led => {}
        (Indicator::Rgb, _) => items.push_str(
            "
/// The board's RGB LED shows the status (`iotkit::status`): blue blinking while it waits to be set up.
#[embassy_executor::task]
async fn status_task(led: Led) {
    // `Show::All` while developing; `Show::Pairing` for a release: lit only to set it up.
    iotkit::components::status_rgb(led, 1, 32, iotkit::status::Show::All).await
}
",
        ),
        (Indicator::Led, Some(pin)) => {
            let low = p.answers.status_active == Some(super::Level::Low);
            uses.extend([
                "esp_hal::gpio::Level",
                "esp_hal::gpio::Output",
                "esp_hal::gpio::OutputConfig",
                "iotkit::Active",
            ]);
            uses.add(format!("esp_hal::peripherals::GPIO{pin}"));
            // Dark until the status task runs.
            let _ = write!(
                items,
                "
/// Starts the status LED on GPIO{pin} (from `main`).
pub fn spawn_status(spawner: Spawner, pin: GPIO{pin}<'static>) {{
    let led = Output::new(pin, Level::{}, OutputConfig::default());
    spawner.spawn(status_task(led).unwrap());
}}

/// The LED shows the status (`iotkit::status`): slow blinking while it waits to be set up.
#[embassy_executor::task]
async fn status_task(led: Output<'static>) {{
    // `Show::All` while developing; `Show::Pairing` for a release: lit only to set it up.
    iotkit::components::status_led(led, Active::{}, iotkit::status::Show::All).await
}}
",
                if low { "High" } else { "Low" },
                if low { "Low" } else { "High" }
            );
        }
        _ => {}
    }
    if led && !plain_led {
        let _ = write!(
            items,
            "
/// The status LED, as a light{link}; a short press of BOOT toggles it.
/// The status shows over it (`iotkit::status`): blue blinking while it waits to be set up.
#[embassy_executor::task]
async fn led_task(led: Led) {{
    // `Show::All` while developing; `Show::Pairing` for a release: only to set it up.
    iotkit::components::rgb_status_light(&LED, led, 1, iotkit::status::Show::All, true).await
}}
"
        );
    }
    if plain_led {
        uses.extend([
            "embassy_futures::select::Either",
            "embassy_futures::select::select",
            "iotkit::LightState",
            "log::warn",
            "smart_leds::SmartLedsWriteAsync",
            "smart_leds::brightness",
        ]);
        let _ = write!(
            items,
            "
/// The status LED, as a light{link}; a short press of BOOT toggles it.
#[embassy_executor::task]
async fn led_task(mut led: Led) {{
    // Dark until its saved state comes back (or a command): brightness and colour for \"on\".
    let mut state = LightState {{ on: false, brightness: 20, rgb: [0, 128, 255] }};
    loop {{
        LED.set(state);
        let color = if state.on {{ RGB8::new(state.rgb[0], state.rgb[1], state.rgb[2]) }} else {{ RGB8::default() }};
        if let Err(e) = led.write(brightness([color].into_iter(), state.brightness)).await {{
            warn!(\"LED write failed: {{e:?}}\");
        }}
        state = match select(LED.wait_command(), iotkit::provision::gesture::wait_press()).await {{
            Either::First(wanted) => wanted,
            Either::Second(()) => LightState {{ on: !state.on, ..state }},
        }};
    }}
}}
"
        );
    }
    s.push('\n');
    s.push_str(&uses.lines());
    s.push_str(&items);
    Some(wrap_comments(&s))
}

/// A `//` comment line at `indent` (wrapped by [`wrap_comments`]).
fn comment(indent: usize, text: &str) -> String {
    format!("{:indent$}// {text}\n", "")
}

/// Wraps `//`, `///` and `//!` comments longer than 100 columns (rustfmt does not), keeping their
/// indent. An indented line (a `TODO`'s hint, code-like) wraps only past `MAX_WIDTH`, 2 columns
/// further.
fn wrap_comments(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let Some((mark, text)) = ["//! ", "/// ", "// "].iter().find_map(|m| Some((*m, trimmed.strip_prefix(m)?)))
        else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let lead = text.len() - text.trim_start().len();
        let width = if lead > 0 { MAX_WIDTH } else { 100 };
        if line.chars().count() <= width {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let first = format!("{:indent$}{mark}{:lead$}", "", "");
        let next = if lead > 0 { format!("{first}  ") } else { first.clone() };
        let mut current = first;
        let mut empty = true;
        for word in text.trim_start().split(' ') {
            if !empty && current.chars().count() + 1 + word.chars().count() > width {
                out.push_str(current.trim_end());
                out.push('\n');
                current = next.clone();
                empty = true;
            }
            if !empty {
                current.push(' ');
            }
            current.push_str(word);
            empty = false;
        }
        out.push_str(&current);
        out.push('\n');
    }
    out
}

/// The generated project's `rustfmt.toml` `max_width`.
pub const MAX_WIDTH: usize = 120;

/// The `use` tree of `items` (paths such as `gpio::Input`), shared prefixes merged.
fn group(items: &BTreeSet<&str>) -> String {
    let mut nested: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for item in items {
        if let Some((head, rest)) = item.split_once("::") {
            nested.entry(head).or_default().insert(rest);
        }
    }
    let mut out = Vec::new();
    for item in items.iter().filter(|i| !i.contains("::")) {
        match nested.get_mut(item) {
            Some(rest) => {
                rest.insert("self");
            }
            None => out.push(item.to_string()),
        }
    }
    out.extend(nested.iter().map(|(head, rest)| format!("{head}::{}", group(rest))));
    if out.len() == 1 { out.remove(0) } else { format!("{{{}}}", out.join(", ")) }
}

/// The `[env]` lines of the update answers, for `.cargo/config.toml`.
pub fn update_env(p: &Project) -> String {
    let a = &p.answers;
    let mut s = String::new();
    if a.updates && a.auto_update.is_some() {
        let _ = writeln!(
            s,
            "# Updates install without a click (*Auto-update* in Home Assistant over Wi-Fi).\nIOTKIT_AUTO_UPDATE = \"{}\"",
            if p.auto_update() { "on" } else { "off" }
        );
    }
    if let Some(time) = a.update_time.as_deref().filter(|_| p.auto_update()) {
        let what = if p.thread() {
            "When the updates of the daily checks install, in the controller's time (an hour after a restart without it)"
        } else {
            "When, until set in Home Assistant: `HH:MM`, or `wake` (at once)"
        };
        let _ = writeln!(s, "# {what}.\nIOTKIT_UPDATE_TIME = \"{time}\"");
    }
    s
}

/// `.cargo/secrets.toml` (git-ignored): what the builder gave now, read at build time.
pub fn secrets(p: &Project) -> Option<String> {
    let a = &p.answers;
    let mut s = String::from(
        "# Read at build time (git-ignored). The device stores the network and the broker at its first\n\
         # boot; `factory-reset` on the console forgets them. See secrets.toml.example.\n[env]\n",
    );
    let before = s.len();
    let mut set = |key: &str, value: &str| {
        let _ = writeln!(s, "{key} = {}", toml::Value::String(value.into()));
    };
    if let Some(ssid) = &a.wifi_ssid {
        set("WIFI_SSID", ssid);
        set("WIFI_PASS", a.wifi_password.as_deref().unwrap_or(""));
    }
    if let Some(host) = &a.mqtt_host {
        set("MQTT_HOST", host);
        if let Some(user) = &a.mqtt_user {
            set("MQTT_USER", user);
            set("MQTT_PASS", a.mqtt_password.as_deref().unwrap_or(""));
        }
    }
    if let Some(url) = &a.updates_url {
        set("OTA_MANIFEST_URL", url);
    }
    if let Some(setup) = p.matter_setup {
        set("MATTER_PASSCODE", &setup.passcode.to_string());
        set("MATTER_DISCRIMINATOR", &setup.discriminator.to_string());
    }
    if let Some(key) = &p.esphome_key {
        set("ESPHOME_API_KEY", key);
    }
    (s.len() > before).then_some(s)
}

/// The template's `Cargo.toml` with the features the answers need (its own and its parts').
pub fn cargo_toml(rendered: &str, p: &Project) -> anyhow::Result<String> {
    let mut app_features = vec![p.board.chip];
    let updates = p.answers.updates;
    if p.thread() {
        app_features.push("matter-thread");
        if updates {
            app_features.push("matter-ota");
        }
        if p.icd() {
            app_features.push("matter-icd");
        }
    } else if updates {
        app_features.push("ha-update");
    }
    if p.sleepy() {
        app_features.push("deep-sleep");
    }
    if p.answers.status == Indicator::Rgb {
        app_features.push("smartled");
    }
    if p.status_led() {
        app_features.push("remember");
    }
    if p.esphome() {
        app_features.push("esphome-api");
    }
    for f in part::features(p) {
        if !app_features.contains(&f) {
            app_features.push(f);
        }
    }
    let source = match p.iotkit_path.as_str() {
        "" => format!("git = {:?}", crate::new::IOTKIT_GIT),
        path => format!("path = {:?}", format!("{path}/crates/iotkit")),
    };
    // Thread builds without Wi-Fi; the ESP32 without `improv-ble`, which does not link on it yet;
    // the ESPHome API without MQTT; then only the setup ways, console and updates asked for.
    let setup = p.setup();
    let wifi: Vec<&str> = [
        (setup.contains(&SetupWay::Bluetooth), "improv-ble"),
        (!p.esphome(), "mqtt"),
        (true, "wifi"),
        (setup.contains(&SetupWay::AccessPoint), "softap"),
        (setup.contains(&SetupWay::Serial), "improv-serial"),
        (p.answers.console, "console"),
        (updates, "ota-push"),
    ]
    .into_iter()
    .filter_map(|(on, f)| on.then_some(f))
    .collect();
    let all = ["improv-ble", "mqtt", "wifi", "softap", "improv-serial", "console", "ota-push"];
    let defaults = if p.thread() {
        "default-features = false, "
    } else if wifi != all {
        app_features.extend(wifi);
        "default-features = false, "
    } else {
        ""
    };
    let line = format!(
        "iotkit = {{ {source}, {defaults}features = [{}] }}",
        app_features.iter().map(|f| format!("{f:?}")).collect::<Vec<_>>().join(", ")
    );
    let mut out = String::new();
    let mut replaced = false;
    let mut in_features = false;
    for l in rendered.lines() {
        if l.starts_with('[') {
            in_features = l == "[features]";
            if in_features {
                out.push_str("[features]\n");
                if p.board.matter() && !p.thread() {
                    let link = if p.esphome() { "the ESPHome API" } else { "MQTT" };
                    let _ = writeln!(out, "## Matter over Wi-Fi, next to {link} (needs the 8 MB partition table).");
                    out.push_str("matter-wifi = [\"iotkit/matter-wifi\"]\n");
                    if p.answers.matter != MatterMode::Off {
                        out.push_str("default     = [\"matter-wifi\"]\n");
                    }
                }
                out.push('\n');
                continue;
            }
        }
        if in_features {
            continue;
        }
        if l.starts_with("iotkit ") || l.starts_with("iotkit=") {
            out.push_str(&line);
            replaced = true;
        } else if l.starts_with("embassy-time") && !bus_dependencies(p).is_empty() {
            let _ = writeln!(out, "{l}");
            out.push_str(&bus_dependencies(p).join("\n"));
        } else if l.starts_with("esp-hal-smartled") && p.thread() {
            // Its 802.15.4 and Bluetooth drivers, for the chip.
            let _ = writeln!(out, "{l}");
            let _ = write!(
                out,
                "esp-radio              = {{ version = \"1.0.0-beta.1\", features = [\"{}\", \"ble\", \"ieee802154\", \"esp-alloc\", \"log-04\", \"unstable\"] }}",
                p.board.chip
            );
        } else {
            out.push_str(l);
        }
        out.push('\n');
    }
    anyhow::ensure!(replaced, "the template's Cargo.toml has no `iotkit` line");
    Ok(out)
}

/// One sentence: the board, the power and what it talks to (README and AGENTS.md).
fn summary(p: &Project) -> String {
    let a = &p.answers;
    let power = if p.sleepy() {
        format!("on battery, reporting every {} min", a.report_every_min)
    } else if p.icd() {
        format!("on battery (a Matter ICD), reading its sensors every {} min", a.report_every_min)
    } else {
        "plugged in".into()
    };
    let link = if a.matter != MatterMode::Off && !p.thread() {
        format!("{} and Matter", p.ha_link())
    } else {
        p.ha_link().into()
    };
    format!("A smart device on an {}, {power}, for {link}.", p.board.model)
}

/// A part's row in the README's wiring table.
pub fn readme_row(p: &Project, part: &super::Part) -> String {
    let matter: Vec<&str> = part
        .entities
        .iter()
        .map(|e| match (p.answers.matter != MatterMode::Off, e.spec.matter) {
            (true, Some(m)) => m,
            (true, None) if p.thread() => "not shown (no Matter type)",
            (true, None) => "Home Assistant only",
            (false, _) => "-",
        })
        .collect();
    let mut matter_cell = matter.clone();
    matter_cell.dedup();
    let entities: Vec<String> = part.entities.iter().map(|e| format!("{} `{}`", platform(e.spec.ty), e.id)).collect();
    format!(
        "| {} | {} | {} | {} | {} |",
        part.name,
        part.pin_cell(),
        part.kind.wiring,
        entities.join(", "),
        matter_cell.join(", ")
    )
}

/// The I²C bus's row in the README's wiring table, when it has parts.
pub fn readme_bus_row(p: &Project) -> Option<String> {
    let bus = p.i2c()?;
    Some(format!(
        "| I²C bus | GPIO{} (SDA), GPIO{} (SCL) | SDA and SCL of each I²C part, and its 3.3 V and GND | - | - |",
        bus.sda, bus.scl
    ))
}

/// The project's README: what it is, the wiring, and how to flash, set up and pair it.
pub fn readme(p: &Project) -> String {
    let a = &p.answers;
    let mut s = String::new();
    let _ = writeln!(s, "# {}\n", a.name);
    let _ = writeln!(s, "{} Made with `cargo iotkit new`.\n", summary(p));

    s.push_str("## Parts and wiring\n\n| Part | Pin | Connect | In Home Assistant | Over Matter |\n| --- | --- | --- | --- | --- |\n");
    for part in &p.parts {
        let _ = writeln!(s, "{}", readme_row(p, part));
    }
    if let Some(row) = readme_bus_row(p) {
        let _ = writeln!(s, "{row}");
    }
    if p.status_led() {
        let _ = writeln!(
            s,
            "| Status LED | GPIO{} | on the board | Light `status_led`{} | {} |",
            p.board.rgb_led.unwrap_or_default(),
            if a.status == Indicator::Rgb { ", shows the status too" } else { "" },
            if a.matter != MatterMode::Off { "Extended colour light" } else { "-" }
        );
    }
    if let (true, Some(pin)) = (p.battery(), a.battery_pin) {
        let _ = writeln!(
            s,
            "| Battery level | GPIO{pin} | the middle of a divider from the battery (two equal resistors, 100 kΩ+) | Sensor `battery` | {} |",
            if a.matter != MatterMode::Off { "Power source (battery)" } else { "-" }
        );
    }
    match (p.answers.status, a.status_pin) {
        (Indicator::Led, Some(pin)) if p.board.plain_led.is_some_and(|(led, _)| led == pin) => {
            let _ = writeln!(s, "| Status LED | GPIO{pin} | on the board | - | - |");
        }
        (Indicator::Led, Some(pin)) => {
            let _ = writeln!(s, "| Status LED | GPIO{pin} | a LED and a resistor (330 Ω) to GND | - | - |");
        }
        (Indicator::Rgb, _) if !p.status_led() => {
            let _ = writeln!(s, "| Status LED | GPIO{} | on the board | - | - |", p.board.rgb_led.unwrap_or_default());
        }
        _ => {}
    }
    let button = if p.button_pin() == p.board.boot_pin {
        format!("BOOT (GPIO{})", p.board.boot_pin)
    } else {
        format!("The button on GPIO{} (to GND)", p.button_pin())
    };
    let _ = writeln!(
        s,
        "\n{button} is the setup button: hold it 3 s to open {}, press it to close it, 10 s to factory reset.",
        if p.thread() { "a Matter pairing window" } else { "the setup window" }
    );
    if p.sleepy() {
        s.push_str(
            "Until it has reached Home Assistant once, and for 10 minutes after holding the button 3 s, it stays awake\n\
             (pairing mode). A button on a low-power pin also wakes it: a press sends a report.\n",
        );
    }
    if p.answers.status != Indicator::Off {
        s.push_str(
            "The status LED blinks slowly while it waits to be set up, fast while it connects, twice while it updates.\n",
        );
    }
    if p.parts.is_empty() {
        s.push_str("\nNo parts yet: `cargo iotkit add` adds one (`cargo iotkit components` lists them).\n");
    } else {
        s.push_str(
            "\nEach part is a module in `src/parts/`; the code left to write is a `TODO(<id>)` there.\n\
             `cargo iotkit add` adds a part (`cargo iotkit components` lists them).\n",
        );
    }

    s.push_str(
        "\n## Build and flash\n\n```bash\ncargo iotkit flash --release   # over USB, then the serial monitor\n```\n\n",
    );
    if p.thread() {
        s.push_str("OpenThread is C code: the build needs `clang` and `cmake`.\n\n");
    }
    s.push_str("`keys/ota.key` signs the updates: keep it (it is git-ignored); a device only installs updates signed with it.\n\n");

    if p.thread() {
        s.push_str(
            "## Thread\n\nThe device has no Wi-Fi: pair it over Bluetooth from a Matter controller with a Thread border router\n\
             (e.g. Home Assistant with its OpenThread Border Router add-on); it then joins that controller's\n\
             Thread network.\n",
        );
    } else if a.wifi_ssid.is_some() {
        s.push_str("## Wi-Fi and Home Assistant\n\n");
        s.push_str("The Wi-Fi network is built in (`.cargo/secrets.toml`) and stored at the first boot.\n");
    } else {
        s.push_str("## Wi-Fi and Home Assistant\n\n");
        s.push_str(
            "At its first boot the device opens a setup window: join its access point `iotkit-XXXX` from a phone, or use\nImprov (https://improv-wifi.com) over USB or Bluetooth.\n",
        );
    }
    match &a.mqtt_host {
        _ if p.thread() => {}
        _ if p.esphome() => {
            s.push_str("Home Assistant reaches it over the ESPHome API, with no broker: *Settings > Devices* shows it as\ndiscovered (or *Add integration > ESPHome* with its address, port 6053). ");
            match &p.esphome_key {
                // Not the key itself: the README is committed, secrets.toml is not.
                Some(_) => s.push_str("It asks for the encryption key:\n`ESPHOME_API_KEY` in `.cargo/secrets.toml`, the same on every device flashed from this project.\n"),
                None => s.push_str("It asks for the encryption key:\neach device makes its own at its first boot, and `api-key` on the USB console shows it.\n"),
            }
        }
        Some(host) => {
            let _ = writeln!(s, "It connects to the MQTT broker at `{host}`; Home Assistant finds it by itself.");
        }
        None => s.push_str(
            "Set the MQTT broker (Home Assistant's Mosquitto add-on) with `mqtt 192.168.1.10` on the USB console;\nHome Assistant finds the device by itself.\n",
        ),
    }
    if p.sleepy() && p.esphome() {
        s.push_str(
            "\nAsleep, the device does not answer: Home Assistant knows it sleeps and keeps its last states. At each wake-up it\nconnects and gets the new readings; commands reach the device only while it is awake.\n",
        );
    } else if p.sleepy() {
        s.push_str(
            "\nAsleep, the device does not answer: commands from Home Assistant are kept by the broker and applied at the\nnext wake-up. Its readings show as unavailable after twice the interval without news.\n",
        );
    } else if p.icd() {
        s.push_str(
            "\nOn battery it is a sleepy Thread device: its radio asks its parent for messages every 5 s, so a command\ntakes up to 5 s (`IOTKIT_ICD_POLL_MS` at build time), and the chip sleeps in between. For now its receiver\nstays on between polls (openthread 0.4): a charge lasts days, not months yet.\n",
        );
    }

    if a.matter != MatterMode::Off {
        s.push_str("\n## Matter\n\n");
        match p.matter_setup {
            Some(setup) => {
                let _ = writeln!(
                    s,
                    "Pairing code **{}**, QR code in `matter-pairing.svg` (print it as a label; both are git-ignored, as the code is in\n`.cargo/secrets.toml`). Every device flashed from this project has this code.",
                    setup.manual_code_pretty()
                );
            }
            // Thread builds have no console.
            None if p.thread() => s.push_str("Each device draws its pairing code at its first boot: it is in the boot log.\n"),
            None => s.push_str(
                "Each device draws its pairing code at its first boot: it is in the boot log, and `matter` on the console prints it.\n",
            ),
        }
        s.push_str(if p.thread() {
            "\nIn Home Assistant: *Settings > Devices > Add device > Matter*, then the code. Holding BOOT 3 s lets another\ncontroller pair for 15 minutes.\n"
        } else {
            "\nIn Home Assistant: *Settings > Devices > Add device > Matter*, then the code. `matter pair` on the console lets\nanother controller pair for 15 minutes.\n"
        });
    }

    if !a.updates {
        s.push_str("\n## Updates\n\nBuilt without updates (smaller): flash each new version over USB.\n");
        return s;
    }
    if p.thread() {
        let _ = write!(
            s,
            "\n## Updates\n\n```bash\n# bump `version` in Cargo.toml, then:\ncargo iotkit ota build --release   # dist/{0}/{0}-<version>.ota, and its .json for a provider\n```\n\n\
             A Matter OTA provider serves the `.ota` file: Home Assistant's Matter server with `--ota-provider-dir` set to\n\
             `dist/{0}/` offers it on the device's update entity. The device checks the signature, installs it, and keeps it once it\n\
             rejoins Thread and its controller (else the previous firmware comes back).\n",
            a.package
        );
        s.push_str(&if let Some(time) = a.update_time.as_deref().filter(|_| p.auto_update()) {
            format!(
                "It also asks its controllers' providers for an update every day, downloads what they offer, and installs it at\n\
                 {time} in the controller's time: Home Assistant's Matter Server gives it with its `time_sync` option. Without the\n\
                 time (a restart) it waits for it, an hour at most. An update its controller announces (*Install*) installs at\n\
                 once. Home Assistant sets no daily provider: with it, updates install when clicked.\n"
            )
        } else if p.auto_update() {
            "It also asks its controllers' providers for an update every day, and installs what they offer.\n".into()
        } else {
            "It installs only the updates its controller announces (`IOTKIT_AUTO_UPDATE = \"off\"`).\n".into()
        });
        return s;
    }
    s.push_str("\n## Updates\n\n```bash\n# bump `version` in Cargo.toml, then:\ncargo iotkit ota push --host <device-ip>      # straight to the device\n");
    match &a.updates_url {
        Some(url) => {
            let base = url.rsplit_once('/').map_or(url.as_str(), |(b, _)| b);
            let base = base.rsplit_once('/').map_or(base, |(b, _)| b);
            let _ = writeln!(
                s,
                "cargo iotkit ota publish --to <served-folder> --base-url {base}   # Home Assistant offers it\n```\n\nThe device checks `{url}`."
            );
        }
        None => s.push_str(
            "cargo iotkit ota publish --to <served-folder>  # Home Assistant offers it (`update-url URL` on the console)\n```\n",
        ),
    }
    if p.auto_update() {
        let time = a.update_time.as_deref().unwrap_or("02:00");
        s.push_str(&if p.esphome() && time != "wake" {
            format!(
                "\n*Auto-update* is on: a release installs by itself at {time} (*Update time* in Home Assistant).\n"
            )
        } else {
            "\n*Auto-update* is on: a release installs by itself once found (checked every 6 h).\n".into()
        });
    }
    s
}

/// `AGENTS.md`, for AI coding agents (opt-in): the device, commands, rules and where the API is.
pub fn agents_md(p: &Project) -> String {
    let a = &p.answers;
    let mut s = format!(
        "# AGENTS.md

Instructions for AI coding agents. Humans: see README.md.

## Device

{}. {}
Pins and wiring: the table in README.md. {}
",
        a.name,
        summary(p),
        if p.parts.is_empty() {
            "No parts yet: `cargo iotkit add <component>` adds one."
        } else {
            "Each part is a module in `src/parts/`, its code left to write a `TODO(<entity id>)`."
        }
    );
    if p.sleepy() {
        s.push_str(
            "On battery (`iotkit::start_sleepy`): parts are read once per wake-up in `main`, then it sleeps.\n\
             Read analog values before `start_sleepy`, as the battery level: Wi-Fi's load lowers the voltage.\n",
        );
    } else if p.icd() {
        s.push_str(
            "On battery (`matter-icd`, a sleepy Thread device): always on, its CPU sleeps whenever nothing runs, so\n\
             wait on interrupts or long timers rather than polling.\n",
        );
    }
    if p.thread() {
        s.push_str(
            "Matter over Thread (`matter-thread`, `matter-ota`): no Wi-Fi, MQTT or console; the 8 MB partition table;\n\
             OpenThread needs `clang` and `cmake`. Updates: `cargo iotkit ota build`, served by a Matter OTA provider.\n",
        );
    } else if a.matter != MatterMode::Off {
        s.push_str("Matter (`matter-wifi` feature) needs the 8 MB partition table.\n");
    }
    if p.esphome() {
        s.push_str(
            "Home Assistant over the ESPHome API (`esphome-api` feature), not MQTT: never set a broker as well.\n",
        );
    }
    if p.board.chip == "esp32c6" {
        s.push_str(
            "Never start the C6's LP core (`LpCore`, `load_lp_code!`): it overwrites iotkit's RTC records (rollback, crash record).\n",
        );
    }
    if p.board.xtensa() {
        s.push_str("Xtensa chip: the `esp` toolchain; run `. ~/export-esp.sh` in each new shell before cargo.\n");
    }
    let source = match p.iotkit_path.as_str() {
        "" => "`~/.cargo/git/checkouts/iotkit-*/crates/`".to_string(),
        path => format!("`{path}/crates/`"),
    };
    let _ = write!(
        s,
        "
## Commands

- Check: `cargo clippy --release -- -D warnings` (must pass).
- Build and check the signed image: `cargo iotkit build --release`, then `cargo iotkit verify`.
- `cargo iotkit flash` and `cargo iotkit ota push` touch hardware: only when the user asks.

## Rules

- `no_std`. Drivers must be `no_std`, preferably embedded-hal-async 1.0.
- The heap is fixed (for Wi-Fi and Bluetooth): avoid `alloc`.
- One module per part in `src/parts/` (`cargo iotkit add <component>` writes one: `cargo iotkit
  components` lists them), one task each, never stalled: await a `Timer` or a pin edge. Short blocking bus reads are fine; no busy loops.
- Report states with `.set(..)`; take commands with `.wait_command().await`.
- Prefer the ready-made parts in `iotkit::components` (relay, binary_input, button, poll,
  dimmer, fan, rgb_light; I²C chips in `components::i2c`); `poll` takes any driver in its read closure.
- Versions are pinned (esp-hal ~1.2.2): check APIs in the docs or source below, not from memory.
- Leave `partitions*.csv`, `[profile.*]` and `keys/` alone.
- Bump `version` in `Cargo.toml` for each OTA update.

## Secrets

Never read or print `keys/ota.key` or `.cargo/secrets.toml`.

## API

- iotkit: the guide https://tanguy-c.github.io/iotkit/ (index: `llms.txt` there); `cargo doc -p iotkit`
  (HTML in `target/*/doc/iotkit/`); source in {source}.
- esp-hal: `~/.cargo/registry/src/*/esp-hal-1.2.2/`.
- Examples: {} in {}.
",
        if p.thread() {
            "`apps/thread-light` (Matter over Thread)"
        } else {
            "`apps/devkit` (plugged in) and `apps/sleepy` (battery)"
        },
        crate::new::IOTKIT_GIT
    );
    s
}

/// Home Assistant's name for an entity type.
fn platform(ty: &str) -> &'static str {
    match ty {
        "Sensor" => "Sensor",
        "BinarySensor" => "Binary sensor",
        "Switch" => "Switch",
        "Light" => "Light",
        "Cover" => "Cover",
        "Valve" => "Valve",
        "Lock" => "Lock",
        "Fan" => "Fan",
        "Climate" => "Climate",
        "Number" => "Number",
        "Select" => "Select",
        "Event" => "Event",
        _ => "Entity",
    }
}
