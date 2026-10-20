//! The files `cargo iotkit new` writes from the answers: `src/main.rs`, `README.md`,
//! `.cargo/secrets.toml`, and the features in `Cargo.toml`.

use std::{collections::BTreeSet, fmt::Write as _};

use super::{
    MatterMode, Part, Project,
    parts::{Bus, Stub},
};

/// `src/main.rs`. Each part gets an entity and, in its task (or, on a battery device, in
/// `main`), a `TODO` saying what to write.
pub fn main_rs(p: &Project) -> String {
    let battery = p.battery();
    let led = p.status_led();
    let mut iotkit: BTreeSet<&str> = ["Board", "Config", "PublicKey"].into();
    let mut time: BTreeSet<&str> = BTreeSet::new();
    let mut statics = String::new();
    let mut ids = Vec::new();
    let mut tasks = String::new();
    let mut spawns = String::new();
    let mut readings = String::new();
    let mut any_pin = false;

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

        if battery && k.is_reading() {
            readings.push_str(&todo(4, part, &set, true));
            continue;
        }

        let (param, arg) = match part.pin {
            Some(n) => {
                any_pin = true;
                ("_pin: AnyPin<'static>".to_string(), format!("p.GPIO{n}.into()"))
            }
            None => (String::new(), String::new()),
        };
        let task = format!("{}_task", part.id);
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
        let doc_extra = if battery { " Commands sent while the device sleeps arrive at its next wake-up." } else { "" };
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

    if led {
        iotkit.extend(["Light", "LightState"]);
        let _ = writeln!(statics, "static LED: Light = Light::new(\"status_led\", \"Status LED\");");
        ids.push("LED".into());
    }
    if battery {
        iotkit.insert("SleepConfig");
        time.insert("Duration");
        if p.answers.battery_pin.is_some() {
            iotkit.insert("Sensor");
            let _ = writeln!(statics, "static BATTERY: Sensor = Sensor::battery(\"battery\", \"Battery\");");
            ids.push("BATTERY".into());
        }
    } else {
        time.insert("Timer");
    }

    let mut s = String::new();
    // Header.
    let _ = writeln!(s, "//! {}: made with `cargo iotkit new` (wiring and setup: README.md).", p.answers.name);
    let _ = writeln!(s, "//!");
    if battery {
        let _ = writeln!(
            s,
            "//! A battery device: it wakes up every {} min, reports, and sleeps (`iotkit_app::start_sleepy`).",
            p.answers.report_every_min
        );
    } else {
        let _ = writeln!(
            s,
            "//! `iotkit_app::start` brings Wi-Fi setup (access point, Improv), Home Assistant, signed updates,"
        );
        let _ = writeln!(s, "//! crash records, the watchdog and the USB console (`help`).");
    }
    s.push_str(
        "//! Each part's code is below: look for `TODO(<entity id>)`. The drivers named there are `no_std`\n\
         //! and embedded-hal 1.0; prefer their async mode.\n\n#![no_std]\n#![no_main]\n\n",
    );

    // Imports.
    s.push_str("use embassy_executor::Spawner;\n");
    if led {
        s.push_str("use embassy_futures::select::{Either, select};\n");
    }
    if !time.is_empty() {
        let _ = writeln!(s, "use embassy_time::{};", group(&time));
    }
    let mut hal: BTreeSet<&str> = BTreeSet::new();
    if any_pin {
        hal.insert("gpio::AnyPin");
    }
    if led {
        hal.extend(["Async", "rmt::Rmt", "time::Rate"]);
    }
    if !hal.is_empty() {
        let _ = writeln!(s, "use esp_hal::{};", group(&hal));
    }
    if led {
        s.push_str("use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};\n");
    }
    if battery && p.answers.battery_pin.is_some() {
        s.push_str("use iotkit_app::battery::{Battery, BatteryCurve};\n");
    }
    let _ = writeln!(s, "use iotkit_app::{};", group(&iotkit));
    if led {
        s.push_str("use log::warn;\nuse smart_leds::{RGB8, SmartLedsWriteAsync, brightness};\n");
    }
    s.push_str(
        "
/// Updates must be signed with the secret half (`cargo iotkit keygen`).
static OTA_KEY: PublicKey = iotkit_app::public_key(include_str!(\"../keys/ota.pub\"));

",
    );

    // Entities and configuration.
    s.push_str(&statics);
    let _ = writeln!(
        s,
        "
static CONFIG: Config = Config::new(iotkit_app::firmware!(&OTA_KEY))
    .manufacturer({:?})
    .model({:?})
    .product_name({:?})
    .entities(&[{}]);",
        p.answers.maker,
        p.board.model,
        p.answers.name,
        ids.iter().map(|i| format!("&{i}")).collect::<Vec<_>>().join(", ")
    );
    if battery {
        let _ = writeln!(
            s,
            "
/// How often it wakes up, and how long it may try to reach Home Assistant.
static SLEEP: SleepConfig = SleepConfig {{ interval: Duration::from_secs({}), max_awake: Duration::from_secs(30) }};",
            u64::from(p.answers.report_every_min) * 60
        );
    }
    if led {
        let pin = p.board.rgb_led.expect("status LED on a board with one");
        let _ = writeln!(
            s,
            "
/// The board's WS2812 LED on GPIO{pin}, driven by an RMT channel.
type Led = RmtSmartLeds<'static, {{ buffer_size::<RGB8>(1) }}, Async, RGB8, color_order::Grb>;"
        );
    }

    // main. A part sharing the ADC with the battery reading needs `mut p` (to reborrow ADC1).
    let shared_adc = battery && p.answers.battery_pin.is_some() && p.parts.iter().any(|part| part.kind.analog);
    let _ = write!(
        s,
        "
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {{
    iotkit_app::init!({}p);
",
        if shared_adc { "mut " } else { "" }
    );
    if battery && let Some(pin) = p.answers.battery_pin {
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
        flash: p.FLASH,
        wifi: p.WIFI,
        bt: p.BT,
        timg1: p.TIMG1,
        usb: p.USB_DEVICE,
        boot_button: Some(p.GPIO{}.into()),
    }};
",
        p.board.boot_pin
    );
    if battery {
        s.push_str("    let app = iotkit_app::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;\n");
    } else {
        s.push_str("    iotkit_app::start(spawner, board, &CONFIG).await;\n");
    }
    if led {
        let pin = p.board.rgb_led.unwrap_or_default();
        let _ = write!(
            s,
            "
    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async();
    let led = Led::new(WS2812_TIMING, rmt.channel0, p.GPIO{pin}, Rate::from_mhz(80)).unwrap();
    spawner.spawn(led_task(led).unwrap());
"
        );
    }
    if !spawns.is_empty() {
        s.push('\n');
        s.push_str(&spawns);
    }
    if battery {
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
    if led {
        s.push_str(
            "
/// The status LED, as a light in Home Assistant; a short press of BOOT toggles it.
#[embassy_executor::task]
async fn led_task(mut led: Led) {
    let mut state = LightState { on: true, brightness: 20, rgb: [0, 128, 255] };
    loop {
        LED.set(state);
        let color = if state.on { RGB8::new(state.rgb[0], state.rgb[1], state.rgb[2]) } else { RGB8::default() };
        if let Err(e) = led.write(brightness([color].into_iter(), state.brightness)).await {
            warn!(\"LED write failed: {e:?}\");
        }
        state = match select(LED.wait_command(), iotkit_app::provision::esp::wait_press()).await {
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

fn group(items: &BTreeSet<&str>) -> String {
    if items.len() == 1 {
        items.iter().next().expect("one").to_string()
    } else {
        format!("{{{}}}", items.iter().copied().collect::<Vec<_>>().join(", "))
    }
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
    (s.len() > before).then_some(s)
}

/// The template's `Cargo.toml` with the features the answers need.
pub fn cargo_toml(rendered: &str, p: &Project) -> anyhow::Result<String> {
    let mut app_features = vec![p.board.chip, "ha-update"];
    if p.battery() {
        app_features.push("deep-sleep");
    }
    let source = match p.iotkit_path.as_str() {
        "" => format!("git = {:?}", crate::new::IOTKIT_GIT),
        path => format!("path = {:?}", format!("{path}/crates/iotkit-app")),
    };
    let line = format!(
        "iotkit-app = {{ {source}, features = [{}] }}",
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
                if p.board.matter() {
                    out.push_str("## Matter over Wi-Fi, next to MQTT (needs the 8 MB partition table).\n");
                    out.push_str("matter-wifi = [\"iotkit-app/matter-wifi\"]\n");
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
        if l.starts_with("iotkit-app ") || l.starts_with("iotkit-app=") {
            out.push_str(&line);
            replaced = true;
        } else {
            out.push_str(l);
        }
        out.push('\n');
    }
    anyhow::ensure!(replaced, "the template's Cargo.toml has no `iotkit-app` line");
    Ok(out)
}

/// One sentence: the board, the power and what it talks to (README and AGENTS.md).
fn summary(p: &Project) -> String {
    let a = &p.answers;
    let power = if p.battery() {
        format!("on battery, reporting every {} min", a.report_every_min)
    } else {
        "plugged in".into()
    };
    let link = if a.matter != MatterMode::Off { "Home Assistant (MQTT) and Matter" } else { "Home Assistant (MQTT)" };
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
            (true, None) => "MQTT only",
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
            "| Status LED | GPIO{} | on the board | Light `status_led` | {} |",
            p.board.rgb_led.unwrap_or_default(),
            if a.matter != MatterMode::Off { "Extended colour light" } else { "-" }
        );
    }
    if let (true, Some(pin)) = (p.battery(), a.battery_pin) {
        let _ = writeln!(
            s,
            "| Battery level | GPIO{pin} | the middle of a divider from the battery (two equal resistors, 100 kΩ+) | Sensor `battery` | - |"
        );
    }
    let _ = writeln!(
        s,
        "\nBOOT (GPIO{}) is the setup button: hold it 3 s to open the setup window, 10 s to factory reset.",
        p.board.boot_pin
    );
    if p.parts.is_empty() {
        s.push_str("\nNo parts yet: add entities in `src/main.rs` (see the iotkit README).\n");
    } else {
        s.push_str("\nThe code for each part is a `TODO(<id>)` in `src/main.rs`: write the driver there.\n");
    }

    s.push_str(
        "\n## Build and flash\n\n```bash\ncargo iotkit flash --release   # over USB, then the serial monitor\n```\n\n",
    );
    s.push_str("`keys/ota.key` signs the updates: keep it (it is git-ignored); a device only installs updates signed with it.\n\n");

    s.push_str("## Wi-Fi and Home Assistant\n\n");
    if a.wifi_ssid.is_some() {
        s.push_str("The Wi-Fi network is built in (`.cargo/secrets.toml`) and stored at the first boot.\n");
    } else {
        s.push_str(
            "At its first boot the device opens a setup window: join its access point `iotkit-XXXX` from a phone, or use\nImprov (https://improv-wifi.com) over USB or Bluetooth.\n",
        );
    }
    match &a.mqtt_host {
        Some(host) => {
            let _ = writeln!(s, "It connects to the MQTT broker at `{host}`; Home Assistant finds it by itself.");
        }
        None => s.push_str(
            "Set the MQTT broker (Home Assistant's Mosquitto add-on) with `mqtt 192.168.1.10` on the USB console;\nHome Assistant finds the device by itself.\n",
        ),
    }
    if p.battery() {
        s.push_str(
            "\nAsleep, the device does not answer: commands from Home Assistant are kept by the broker and applied at the\nnext wake-up. Its readings show as unavailable after twice the interval without news.\n",
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
            None => s.push_str(
                "Each device draws its pairing code at its first boot: it is in the boot log, and `matter` on the console prints it.\n",
            ),
        }
        s.push_str(
            "\nIn Home Assistant: *Settings > Devices > Add device > Matter*, then the code. `matter pair` on the console lets\nanother controller pair for 15 minutes.\n",
        );
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
Pins and wiring: the table in README.md. Each part's code is a `TODO(<entity id>)` in `src/main.rs`.
",
        a.name,
        summary(p)
    );
    if p.battery() {
        s.push_str(
            "On battery (`iotkit_app::start_sleepy`): parts are read once per wake-up in `main`, then it sleeps.\n\
             Read analog values before `start_sleepy`, as the battery level: Wi-Fi's load lowers the voltage.\n",
        );
    }
    if a.matter != MatterMode::Off {
        s.push_str("Matter (`matter-wifi` feature) needs the 8 MB partition table.\n");
    }
    if p.board.xtensa() {
        s.push_str("ESP32-S3 (Xtensa): the `esp` toolchain; run `. ~/export-esp.sh` in each new shell before cargo.\n");
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
- Versions are pinned (esp-hal ~1.2.2): check APIs in the docs or source below, not from memory.
- Leave `partitions*.csv`, `[profile.*]` and `keys/` alone.
- Bump `version` in `Cargo.toml` for each OTA update.

## Secrets

Never read or print `keys/ota.key` or `.cargo/secrets.toml`.

## API

- iotkit: `cargo doc -p iotkit-app` (HTML in `target/*/doc/iotkit_app/`); source in {source}.
- esp-hal: `~/.cargo/registry/src/*/esp-hal-1.2.2/`.
- Examples: `apps/devkit` (plugged in) and `apps/sleepy` (battery) in {}.
",
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
