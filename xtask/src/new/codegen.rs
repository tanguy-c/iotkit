//! The files `cargo iotkit new` writes from the answers: `src/main.rs`, `README.md`,
//! `.cargo/secrets.toml`, and the features in `Cargo.toml`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use super::{
    Indicator, MatterMode, Part, Project, SetupWay,
    parts::{Bus, Component, Stub},
};

/// `src/main.rs`. Each part gets an entity and, in its task (or, on a deep-sleeping device, in
/// `main`), a `TODO` saying what to write.
pub fn main_rs(p: &Project) -> String {
    let sleepy = p.sleepy();
    // The battery level, read once per wake-up (over Thread, the ADC feeds the TRNG: none).
    let battery_pin = sleepy.then_some(p.answers.battery_pin).flatten();
    let led = p.status_led();
    let status = p.answers.status;
    // The RGB LED is a light: plain, or showing the status too (`rgb_status_light`).
    let plain_led = led && status != Indicator::Rgb;
    let mut iotkit: BTreeSet<&str> = ["Board", "Config", "PublicKey"].into();
    let mut time: BTreeSet<&str> = BTreeSet::new();
    let mut statics = String::new();
    let mut ids = Vec::new();
    let mut tasks = String::new();
    let mut spawns = String::new();
    let mut readings = String::new();
    let mut any_pin = false;
    let mut hal: BTreeSet<&str> = BTreeSet::new();
    // Lines in `main` before the spawns (an ADC set up for a part).
    let mut setup = String::new();
    let mut adc_taken = false;

    for part in &p.parts {
        let k = part.kind;
        iotkit.insert(k.ty);
        let builder = k.builder;
        for ty in ["DeviceClass", "Unit"] {
            if builder.contains(&format!("{ty}::")) {
                iotkit.insert(ty);
            }
        }
        let _ = writeln!(
            statics,
            "static {}: {} = {}::{}({:?}, {:?}{}){builder};",
            part.konst, k.ty, k.ty, k.ctor, part.id, part.name, k.new_args
        );
        ids.push(part.konst.clone());
        let on_pin = match part.pin {
            Some(n) => format!(", on GPIO{n}"),
            None => String::new(),
        };
        let c = &part.konst;
        let set = format!("{}, then `{c}.set(..)`", k.todo);

        if sleepy && k.is_reading() {
            readings.push_str(&todo(4, part, &set, true));
            continue;
        }

        let doc_extra = match (sleepy, p.esphome()) {
            (false, _) => "",
            (true, false) => " Commands sent while the device sleeps arrive at its next wake-up.",
            (true, true) => " Commands reach it only while it is awake.",
        };
        let task = format!("{}_task", part.id);
        if let (false, Some(comp), Some(_)) = (sleepy, k.component, part.pin)
            && !(comp == Component::Analog && adc_taken)
        {
            adc_taken |= comp == Component::Analog;
            let (param, arg, body) = component(comp, part, p.board.chip, &mut iotkit, &mut hal, &mut time, &mut setup);
            let _ = writeln!(spawns, "    spawner.spawn({task}({arg}).unwrap());");
            task_fn(&mut tasks, part, &on_pin, doc_extra, &task, &param, &body);
            continue;
        }

        let (param, arg) = match part.pin {
            Some(n) => {
                any_pin = true;
                ("_pin: AnyPin<'static>".to_string(), format!("p.GPIO{n}.into()"))
            }
            None => (String::new(), String::new()),
        };
        let _ = writeln!(spawns, "    spawner.spawn({task}({arg}).unwrap());");
        // Commands: the TODO sits between the command and the state it reports.
        let t = todo(8, part, k.todo, false);
        let body = match k.stub {
            Stub::Reading => {
                time.extend(["Duration", "Ticker"]);
                format!(
                    "    let mut ticker = Ticker::every(Duration::from_secs(30));
    loop {{
{}        ticker.next().await;
    }}",
                    todo(8, part, &set, false)
                )
            }
            Stub::Detect | Stub::Presses => {
                time.insert("Timer");
                let head = match k.stub {
                    Stub::Detect => format!("{set} at each change (`wait_for_any_edge().await`, not the timer)"),
                    _ => k.todo.to_string(),
                };
                format!(
                    "    loop {{
{}        Timer::after_secs(1).await;
    }}",
                    todo(8, part, &head, false)
                )
            }
            Stub::Switch => format!(
                "    {c}.set(false);
    loop {{
        let on = {c}.wait_command().await;
{t}        {c}.set(on);
    }}"
            ),
            Stub::Light => {
                iotkit.insert("LightState");
                format!(
                    "    let mut state = LightState {{ on: false, brightness: 255, rgb: [255, 255, 255] }};
    loop {{
{t}        {c}.set(state);
        state = {c}.wait_command().await;
    }}"
                )
            }
            Stub::Cover => {
                iotkit.extend(["CoverCommand", "CoverState"]);
                format!(
                    "    {c}.set(Some(CoverState::Closed), Some(0));
    loop {{
        let position = match {c}.wait_command().await {{
            CoverCommand::Open => 100,
            CoverCommand::Close => 0,
            CoverCommand::Position(p) => p,
            CoverCommand::Stop => continue,
        }};
{t}        {c}.set(Some(if position == 0 {{ CoverState::Closed }} else {{ CoverState::Open }}), Some(position));
    }}"
                )
            }
            Stub::Lock => {
                iotkit.extend(["LockCommand", "LockState"]);
                format!(
                    "    {c}.set(LockState::Locked);
    loop {{
        let state = match {c}.wait_command().await {{
            LockCommand::Lock => LockState::Locked,
            LockCommand::Unlock | LockCommand::Open => LockState::Unlocked,
        }};
{t}        {c}.set(state);
    }}"
                )
            }
            Stub::Fan => {
                iotkit.insert("FanState");
                format!(
                    "    {c}.set(FanState::OFF);
    loop {{
        let wanted = {c}.wait_command().await;
{t}        {c}.set(wanted);
    }}"
                )
            }
            Stub::Climate => {
                iotkit.extend(["ClimateState", "HvacMode"]);
                format!(
                    "    let mut state = ClimateState {{ mode: HvacMode::Heat, target: 20.0, current: None, action: None }};
    loop {{
{t}        {c}.set(state);
        let wanted = {c}.wait_command().await;
        state.mode = wanted.mode;
        state.target = wanted.target;
    }}"
                )
            }
            Stub::Number => format!(
                "    {c}.set(50.0);
    loop {{
        let value = {c}.wait_command().await;
{t}        {c}.set(value);
    }}"
            ),
            Stub::Select => format!(
                "    {c}.set(0);
    loop {{
        let index = {c}.wait_command().await;
{t}        {c}.set(index);
    }}"
            ),
        };
        if k.ty == "Climate" {
            iotkit.insert("HvacMode");
        }
        task_fn(&mut tasks, part, &on_pin, doc_extra, &task, &param, &body);
    }

    if led {
        iotkit.insert("Light");
        if status != Indicator::Rgb {
            iotkit.insert("LightState");
        }
        // Kept across restarts (never on battery: no status LED there).
        let _ = writeln!(statics, "static LED: Light = Light::new(\"status_led\", \"Status LED\").remember();");
        ids.push("LED".into());
    }
    if battery_pin.is_some() {
        iotkit.insert("Sensor");
        let _ = writeln!(statics, "static BATTERY: Sensor = Sensor::battery(\"battery\", \"Battery\");");
        ids.push("BATTERY".into());
    }
    if sleepy {
        iotkit.insert("SleepConfig");
        time.insert("Duration");
    } else {
        time.insert("Timer");
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
        s.push_str("//! No parts yet: add entities below (https://tanguy-c.github.io/iotkit/entities.html).\n");
    } else {
        s.push_str(
            "//! Each part's code is below: look for `TODO(<entity id>)`. The drivers named there are `no_std`\n\
             //! and embedded-hal 1.0; prefer their async mode.\n",
        );
    }
    s.push_str("\n#![no_std]\n#![no_main]\n\n");

    // Imports.
    s.push_str("use embassy_executor::Spawner;\n");
    if plain_led {
        s.push_str("use embassy_futures::select::{Either, select};\n");
    }
    if !time.is_empty() {
        let _ = writeln!(s, "use embassy_time::{};", group(&time));
    }
    if any_pin {
        hal.insert("gpio::AnyPin");
    }
    if led || status == Indicator::Rgb {
        hal.extend(["Async", "rmt::Rmt", "time::Rate"]);
    }
    if status == Indicator::Led || !p.board.init.is_empty() {
        hal.extend(["gpio::Level", "gpio::Output", "gpio::OutputConfig"]);
    }
    if status == Indicator::Led {
        iotkit.insert("Active");
    }
    if !hal.is_empty() {
        let _ = writeln!(s, "use esp_hal::{};", group(&hal));
    }
    if led || status == Indicator::Rgb {
        s.push_str("use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};\n");
    }
    if battery_pin.is_some() {
        s.push_str("use iotkit::battery::{Battery, BatteryCurve};\n");
    }
    let _ = writeln!(s, "use iotkit::{};", group(&iotkit));

    if plain_led {
        s.push_str("use log::warn;\nuse smart_leds::{RGB8, SmartLedsWriteAsync, brightness};\n");
    } else if status == Indicator::Rgb {
        s.push_str("use smart_leds::RGB8;\n");
    }
    s.push_str(
        "
/// Updates must be signed with the secret half (`cargo iotkit keygen`).
static OTA_KEY: PublicKey = iotkit::public_key(include_str!(\"../keys/ota.pub\"));

",
    );

    // Entities and configuration.
    s.push_str(&statics);
    let _ = writeln!(
        s,
        "
static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
    .manufacturer({:?})
    .model({:?})
    .product_name({:?})
    .entities(&[{}]);",
        p.answers.maker,
        p.board.model,
        p.answers.name,
        ids.iter().map(|i| format!("&{i}")).collect::<Vec<_>>().join(", ")
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
    if led || status == Indicator::Rgb {
        let pin = p.board.rgb_led.expect("status LED on a board with one");
        let _ = writeln!(
            s,
            "
/// The board's WS2812 LED on GPIO{pin}, driven by an RMT channel.
type Led = RmtSmartLeds<'static, {{ buffer_size::<RGB8>(1) }}, Async, RGB8, color_order::Grb>;"
        );
    }

    // main. A part sharing the ADC with the battery reading needs `mut p` (to reborrow ADC1).
    let shared_adc = battery_pin.is_some() && p.parts.iter().any(|part| part.kind.analog);
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
        let shared = if shared_adc { "\n    // A part needs the ADC too: `p.ADC1.reborrow()` here." } else { "" };
        let _ = write!(
            s,
            "
    // Before Wi-Fi starts: the radio's load lowers the voltage. 2.0: two equal resistors
    // between the battery and GPIO{pin} (TODO(battery): your divider's ratio).{shared}
    Battery::new(p.ADC1, p.GPIO{pin}, 2.0).curve(BatteryCurve::{curve}).report(&BATTERY, None);
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
    let rgb_led = |s: &mut String, task: &str| {
        let pin = p.board.rgb_led.unwrap_or_default();
        let _ = write!(
            s,
            "
    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, p.GPIO{pin}, Rate::from_mhz(80)).unwrap();
    spawner.spawn({task}(led).unwrap());
"
        );
    };
    if led {
        // Before `start`, which keeps the CPU a while (Matter loads its state): until the first
        // write, the LED shows whatever a reset left on its data line.
        s.push_str("\n    // Written right after the bootloader; its saved state comes once `start` restores it.");
        rgb_led(&mut s, "led_task");
        s.push('\n');
    }
    if sleepy {
        s.push_str("    let app = iotkit::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;\n");
    } else {
        s.push_str("    iotkit::start(spawner, board, &CONFIG).await;\n");
    }
    if status == Indicator::Rgb && !led {
        rgb_led(&mut s, "status_task");
    }
    if let (Indicator::Led, Some(pin)) = (status, p.answers.status_pin) {
        // Dark until the status task runs.
        let dark = if p.answers.status_active == Some(super::Level::Low) { "High" } else { "Low" };
        let _ = write!(
            s,
            "
    let led = Output::new(p.GPIO{pin}, Level::{dark}, OutputConfig::default());
    spawner.spawn(status_task(led).unwrap());
"
        );
    }
    if !spawns.is_empty() {
        s.push('\n');
        s.push_str(&setup);
        s.push_str(&spawns);
    }
    if sleepy {
        if !readings.is_empty() {
            s.push_str("\n    // Read once per wake-up.\n");
            s.push_str(&readings);
        }
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
    s.push_str(&tasks);
    match status {
        Indicator::Off | Indicator::Auto => {}
        // The light shows it (`led_task`).
        Indicator::Rgb if led => {}
        Indicator::Rgb => s.push_str(
            "
/// The board's RGB LED shows the status (`iotkit::status`): blue blinking while it waits to be set up.
#[embassy_executor::task]
async fn status_task(led: Led) {
    // `Show::All` while developing; `Show::Pairing` for a release: lit only to set it up.
    iotkit::components::status_rgb(led, 1, 32, iotkit::status::Show::All).await
}
",
        ),
        Indicator::Led => s.push_str(
            &"
/// The LED shows the status (`iotkit::status`): slow blinking while it waits to be set up.
#[embassy_executor::task]
async fn status_task(led: Output<'static>) {
    // `Show::All` while developing; `Show::Pairing` for a release: lit only to set it up.
    iotkit::components::status_led(led, Active::ACTIVE, iotkit::status::Show::All).await
}
"
            .replace("ACTIVE", if p.answers.status_active == Some(super::Level::Low) { "Low" } else { "High" }),
        ),
    }
    if led && !plain_led {
        let _ = write!(
            s,
            "
/// The status LED, as a light{}; a short press of BOOT toggles it.
/// The status shows over it (`iotkit::status`): blue blinking while it waits to be set up.
#[embassy_executor::task]
async fn led_task(led: Led) {{
    // `Show::All` while developing; `Show::Pairing` for a release: only to set it up.
    iotkit::components::rgb_status_light(&LED, led, 1, iotkit::status::Show::All, true).await
}}
",
            if p.thread() { " over Matter" } else { " in Home Assistant" }
        );
    }
    if plain_led {
        let _ = write!(
            s,
            "
/// The status LED, as a light{}; a short press of BOOT toggles it.",
            if p.thread() { " over Matter" } else { " in Home Assistant" }
        );
        s.push_str(
            "
#[embassy_executor::task]
async fn led_task(mut led: Led) {
    // Dark until its saved state comes back (or a command): brightness and colour for \"on\".
    let mut state = LightState { on: false, brightness: 20, rgb: [0, 128, 255] };
    loop {
        LED.set(state);
        let color = if state.on { RGB8::new(state.rgb[0], state.rgb[1], state.rgb[2]) } else { RGB8::default() };
        if let Err(e) = led.write(brightness([color].into_iter(), state.brightness)).await {
            warn!(\"LED write failed: {e:?}\");
        }
        state = match select(LED.wait_command(), iotkit::provision::gesture::wait_press()).await {
            Either::First(wanted) => wanted,
            Either::Second(()) => LightState { on: !state.on, ..state },
        };
    }
}
",
        );
    }
    wrap_comments(&s)
}

/// A `//` comment line at `indent` (wrapped by [`wrap_comments`]).
fn comment(indent: usize, text: &str) -> String {
    format!("{:indent$}// {text}\n", "")
}

/// A part's `TODO(<id>)`: `head`, then a setup hint per bus and the driver crates. In `main`
/// (`in_main`), the pin is still `p.GPIOn`; in a task it is `_pin`.
fn todo(indent: usize, part: &Part, head: &str, in_main: bool) -> String {
    let k = part.kind;
    let unpinned =
        if part.pin.is_none() && !k.buses.is_empty() { " (no pin chosen yet: pass one from `main`)" } else { "" };
    let mut s = comment(indent, &format!("TODO({}): {head}{unpinned}.", part.id));
    for &bus in k.buses {
        let pin = match part.pin {
            Some(n) if in_main || bus == Bus::Adc => format!("p.GPIO{n}"),
            Some(_) => "_pin".into(),
            None => "<pin>".into(),
        };
        s += &comment(indent, &format!("  {}", bus.hint(&pin)));
    }
    if !k.drivers.is_empty() {
        s += &comment(indent, &format!("  drivers: {}", k.drivers));
    }
    s
}

/// Wraps `//` comments longer than 100 columns (rustfmt does not), keeping their indent. An
/// indented line (a `TODO`'s hint, code-like) wraps only past `MAX_WIDTH`, 2 columns further.
fn wrap_comments(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let text = trimmed.strip_prefix("// ").unwrap_or_default();
        let lead = text.len() - text.trim_start().len();
        let width = if lead > 0 { MAX_WIDTH } else { 100 };
        if !trimmed.starts_with("// ") || line.chars().count() <= width {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let first = format!("{:indent$}// {:lead$}", "", "");
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

/// The template's `Cargo.toml` with the features the answers need.
/// A part's task.
fn task_fn(tasks: &mut String, part: &Part, on_pin: &str, doc_extra: &str, task: &str, param: &str, body: &str) {
    let _ = write!(
        tasks,
        "
/// {}{on_pin}.{doc_extra}
#[embassy_executor::task]
async fn {task}({param}) {{
{body}
}}
",
        part.name
    );
}

/// A ready-made part on its pin: the task's parameters, the spawn argument and the task's
/// body. Adds the imports it needs, and its ADC set-up to `setup`.
fn component<'a>(
    comp: Component,
    part: &Part,
    chip: &str,
    iotkit: &mut BTreeSet<&'a str>,
    hal: &mut BTreeSet<&'a str>,
    time: &mut BTreeSet<&'a str>,
    setup: &mut String,
) -> (String, String, String) {
    let (c, id) = (&part.konst, &part.id);
    let pin = part.pin.expect("ready-made parts have a pin");
    iotkit.insert("components");
    match comp {
        Component::Relay => {
            iotkit.insert("Active");
            hal.extend(["gpio::Level", "gpio::Output", "gpio::OutputConfig"]);
            (
                "pin: Output<'static>".into(),
                format!("Output::new(p.GPIO{pin}, Level::Low, OutputConfig::default())"),
                format!(
                    "    // `Active::Low` for a relay board switched by a low input.\n    components::relay(&{c}, pin, Active::High).await"
                ),
            )
        }
        Component::Input { .. } | Component::Button => {
            let (pull_up, active_low) = match comp {
                Component::Input { pull_up, active_low } => (pull_up, active_low),
                _ => (true, true),
            };
            iotkit.insert("Active");
            hal.extend(["gpio::Input", "gpio::InputConfig"]);
            let config = if pull_up {
                hal.insert("gpio::Pull");
                "InputConfig::default().with_pull(Pull::Up)"
            } else {
                "InputConfig::default()"
            };
            let active = if active_low { "Low" } else { "High" };
            let call = if comp == Component::Button {
                format!("components::button(&{c}, pin, Active::{active}, components::PressTiming::DEFAULT)")
            } else {
                time.insert("Duration");
                format!("components::binary_input(&{c}, pin, Active::{active}, Duration::from_millis(50))")
            };
            ("pin: Input<'static>".into(), format!("Input::new(p.GPIO{pin}, {config})"), format!("    {call}.await"))
        }
        Component::Analog => {
            iotkit.extend(["components::filter::Clamp", "components::filter::Filter", "components::filter::Linear"]);
            // The ESP32's ADC has no async mode: a reading takes microseconds, so it is polled.
            let blocking = chip == "esp32";
            let mode = if blocking { "Blocking" } else { "Async" };
            hal.insert(mode);
            hal.extend([
                "analog::adc::Adc",
                "analog::adc::AdcConfig",
                "analog::adc::AdcPin",
                "analog::adc::Attenuation",
            ]);
            time.insert("Duration");
            let _ = writeln!(
                setup,
                "    let mut adc_config = AdcConfig::new();\n    let {id}_pin = adc_config.enable_pin(p.GPIO{pin}, Attenuation::_11dB);"
            );
            (
                format!(
                    "\n    mut adc: Adc<'static, esp_hal::peripherals::ADC1<'static>, {mode}>,\n    mut pin: AdcPin<esp_hal::peripherals::GPIO{pin}<'static>, esp_hal::peripherals::ADC1<'static>>,\n"
                ),
                format!("Adc::new(p.ADC1, adc_config){}, {id}_pin", if blocking { "" } else { ".into_async()" }),
                format!(
                    "    // TODO({id}): calibrate: the raw readings dry (0 %) and in water (100 %).
    let percent = Linear::new((3000.0, 0.0), (1200.0, 100.0)).then(Clamp::new(0.0, 100.0));
    components::poll(&{c}, Duration::from_secs(30), percent, async || {{
        {read}
    }})
    .await",
                    read = if blocking {
                        "loop {\n            if let Ok(raw) = adc.read_oneshot(&mut pin) {\n                break Some(f32::from(raw));\n            }\n        }"
                    } else {
                        "Some(f32::from(adc.read_oneshot(&mut pin).await))"
                    }
                ),
            )
        }
    }
}

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
        "on battery (a Matter ICD)".into()
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

/// The project's README: what it is, the wiring, and how to flash, set up and pair it.
pub fn readme(p: &Project) -> String {
    let a = &p.answers;
    let mut s = String::new();
    let _ = writeln!(s, "# {}\n", a.name);
    let _ = writeln!(s, "{} Made with `cargo iotkit new`.\n", summary(p));

    s.push_str("## Parts and wiring\n\n| Part | Pin | Connect | In Home Assistant | Over Matter |\n| --- | --- | --- | --- | --- |\n");
    for part in &p.parts {
        let pin = part.pin.map_or("to choose".into(), |n| format!("GPIO{n}"));
        let matter = match (a.matter != MatterMode::Off, part.kind.matter) {
            (true, Some(m)) => m,
            (true, None) if p.thread() => "not shown (no Matter type)",
            (true, None) => "Home Assistant only",
            (false, _) => "-",
        };
        let _ = writeln!(
            s,
            "| {} | {pin} | {} | {} `{}` | {matter} |",
            part.name,
            part.kind.wiring,
            platform(part.kind.ty),
            part.id
        );
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
        s.push_str(
            "\nNo parts yet: add entities in `src/main.rs` (see https://tanguy-c.github.io/iotkit/entities.html).\n",
        );
    } else {
        s.push_str("\nThe code for each part is a `TODO(<id>)` in `src/main.rs`: write the driver there.\n");
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
            "No parts yet: entities are added in `src/main.rs`."
        } else {
            "Each part's code is a `TODO(<entity id>)` in `src/main.rs`."
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
- One task per part, never stalled: await a `Timer` or a pin edge. Short blocking bus reads are fine; no busy loops.
- Report states with `.set(..)`; take commands with `.wait_command().await`.
- Prefer the ready-made parts in `iotkit::components` (relay, binary_input, button, poll,
  dimmer, fan, rgb_light); `poll` takes any driver in its read closure.
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
