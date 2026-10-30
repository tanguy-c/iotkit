//! `cargo iotkit new`: a new device project from a few questions (or an answers file).

pub mod add;
mod boards;
mod codegen;
pub mod matter;
mod parts;
mod render;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use boards::{Board, PinCheck, PinUse, boards, check_pin};
use inquire::{Confirm, InquireError, MultiSelect, Password, PasswordDisplayMode, Select, Text, validator::Validation};
use parts::{Group, Kind, kinds, slug};
use serde::{Deserialize, Serialize};

/// What the questions ask; also the format of `--answers FILE` (TOML).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Answers {
    /// What the device is called ("Garden sensor"): Home Assistant's and Matter's name.
    pub name: String,
    /// The folder and package name; from `name` if empty.
    pub package: String,
    /// "Your name or brand": the manufacturer Home Assistant shows.
    pub maker: String,
    /// A board id from `boards.rs`.
    pub board: String,
    /// `4mb` or `8mb`, for another board.
    pub flash_size: Option<String>,
    /// How it talks to the home: Wi-Fi, or Matter over Thread.
    pub transport: Transport,
    pub power: Power,
    /// Battery: how often it wakes up (deep sleep), or reads its sensors (a Matter ICD).
    pub report_every_min: u32,
    /// Battery: the ADC1 pin behind a divider, to report the charge.
    pub battery_pin: Option<u8>,
    pub battery_curve: Curve,
    /// The board's RGB LED as a light (plugged-in devkits); with `status = "rgb"` it shows the
    /// status too.
    pub status_led: bool,
    /// What shows the device's status (pairing, connecting, updates): `auto` (default), `off`,
    /// `rgb` or `led`.
    pub status: Indicator,
    /// With `status = "led"`: the LED's pin (to ground through a resistor).
    pub status_pin: Option<u8>,
    /// With `status = "led"`: `low` if a low level lights the LED (default: the board's own LED's
    /// polarity on its pin, else `high`).
    pub status_active: Option<Level>,
    /// The setup button (to ground; default: the board's BOOT). On battery, a low-power pin
    /// wakes the chip with a press.
    pub button_pin: Option<u8>,
    pub wifi_ssid: Option<String>,
    pub wifi_password: Option<String>,
    pub mqtt_host: Option<String>,
    pub mqtt_user: Option<String>,
    pub mqtt_password: Option<String>,
    /// Home Assistant's ESPHome integration instead of MQTT.
    pub esphome: EsphomeMode,
    /// A fixed key to reuse (base64, as Home Assistant shows it; else drawn); with `esphome = "fixed"`.
    pub esphome_key: Option<String>,
    pub matter: MatterMode,
    /// A fixed code to reuse (else drawn); with `matter = "fixed"`.
    pub matter_passcode: Option<u32>,
    pub matter_discriminator: Option<u16>,
    /// Signed updates: over Wi-Fi from Home Assistant and `cargo iotkit ota push`, over Thread from
    /// a Matter OTA provider. Off, a smaller firmware, flashed over USB only.
    pub updates: bool,
    /// The manifest Home Assistant's updates come from.
    pub updates_url: Option<String>,
    /// Updates install without a click (`IOTKIT_AUTO_UPDATE`); unset, the link's default: on over
    /// Thread and on a battery device over the ESPHome API, off on the others.
    pub auto_update: Option<bool>,
    /// When, over the ESPHome API (`IOTKIT_UPDATE_TIME`): `HH:MM` on the half hour, or `wake`.
    pub update_time: Option<String>,
    /// How it gets its Wi-Fi network at the first boot (default: every way the chip has).
    pub setup: Option<Vec<SetupWay>>,
    /// The USB console (`help`, `info`, `mqtt HOST`): for development, off by default.
    pub console: bool,
    /// The I²C bus its I²C parts share (`[i2c]`, `sda` and `scl`); left out: to choose.
    pub i2c: Option<I2cPins>,
    pub parts: Vec<PartAnswer>,
    /// Writes AGENTS.md, for AI coding agents.
    pub ai_agents: bool,
}

impl Default for Answers {
    fn default() -> Self {
        Self {
            name: "My device".into(),
            package: String::new(),
            maker: "Me".into(),
            board: "esp32c6-devkitc-1".into(),
            flash_size: None,
            transport: Transport::Wifi,
            power: Power::Plugged,
            report_every_min: 5,
            battery_pin: None,
            battery_curve: Curve::Lipo,
            status_led: true,
            status: Indicator::Auto,
            status_pin: None,
            status_active: None,
            button_pin: None,
            wifi_ssid: None,
            wifi_password: None,
            mqtt_host: None,
            mqtt_user: None,
            mqtt_password: None,
            esphome: EsphomeMode::Off,
            esphome_key: None,
            matter: MatterMode::Off,
            matter_passcode: None,
            matter_discriminator: None,
            updates: true,
            updates_url: None,
            auto_update: None,
            update_time: None,
            setup: None,
            console: false,
            i2c: None,
            parts: Vec::new(),
            ai_agents: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// Wi-Fi: Home Assistant over MQTT or the ESPHome API, Matter optional, updates over HTTP.
    Wifi,
    /// Matter over Thread (an 802.15.4 chip): paired over Bluetooth, updates over Matter.
    Thread,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Power {
    Plugged,
    Battery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Curve {
    Lipo,
    #[serde(rename = "2aa")]
    TwoAa,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatterMode {
    Off,
    /// One code for the project, built in: shown now, printable on a label.
    Fixed,
    /// Each device draws its own at its first boot.
    PerDevice,
}

/// What shows the device's status (`iotkit::status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Indicator {
    /// The board's RGB LED if it has one and is not on battery, else nothing.
    Auto,
    Off,
    /// The board's RGB LED (`components::status_rgb`).
    Rgb,
    /// A LED on `status_pin` (`components::status_led`).
    Led,
}

/// A pin's level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    High,
    Low,
}

/// A way to give the device its Wi-Fi network at the first boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupWay {
    /// Its access point and page (`softap`).
    AccessPoint,
    /// Improv over Bluetooth (`improv-ble`: about 300 KB of flash).
    Bluetooth,
    /// Improv over USB (`improv-serial`: ESP Web Tools).
    Serial,
}

impl SetupWay {
    fn label(self) -> &'static str {
        match self {
            Self::AccessPoint => "Its access point and page, from a phone",
            Self::Bluetooth => "Improv over Bluetooth (about 300 KB of flash)",
            Self::Serial => "Improv over USB (ESP Web Tools, improv-wifi.com)",
        }
    }

    /// The ways a chip has (the ESP32: no Bluetooth setup yet).
    pub fn all(chip: &str) -> Vec<Self> {
        let mut all = vec![Self::AccessPoint, Self::Bluetooth, Self::Serial];
        if chip == "esp32" {
            all.retain(|w| *w != Self::Bluetooth);
        }
        all
    }
}

/// The ESPHome native API (`esphome-api`): Home Assistant connects to the device, no broker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EsphomeMode {
    /// MQTT.
    Off,
    /// One encryption key for the project, built in: shown now.
    Fixed,
    /// Each device makes its own at its first boot (`api-key` on the console).
    PerDevice,
}

/// The I²C bus's pins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct I2cPins {
    pub sda: u8,
    pub scl: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartAnswer {
    /// A component's id (`components/<id>.toml`).
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its first pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<u8>,
    /// Its pins by role (`{ sda = 4, scl = 5 }`), for a part with several.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, u8>,
    /// An I²C chip's address, if not its default (`0x45`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u8>,
}

/// The answers, checked and completed.
pub struct Project {
    pub answers: Answers,
    pub board: &'static Board,
    pub package: String,
    pub flash: String,
    pub parts: Vec<Part>,
    pub matter_setup: Option<matter::Setup>,
    /// The built-in ESPHome API key (base64).
    pub esphome_key: Option<String>,
    /// A local checkout; empty for the git repository.
    pub iotkit_path: String,
    pub warnings: Vec<String>,
}

pub struct Part {
    pub kind: &'static Kind,
    pub name: String,
    /// Its id (`living_room_temperature`): its entity's, or with several the start of theirs;
    /// its `TODO(<id>)`.
    pub id: String,
    /// Its module in `src/parts/`: the id, unless a Rust keyword.
    pub module: String,
    /// One per `kind.pins`: `None` to choose.
    pub pins: Vec<Option<u8>>,
    /// One per `kind.entities`.
    pub entities: Vec<Entity>,
    /// An I²C chip's address.
    pub address: Option<u8>,
}

pub struct Entity {
    pub spec: &'static parts::EntitySpec,
    pub id: String,
    /// Its `static` (`LIVING_ROOM_TEMPERATURE`).
    pub konst: String,
    pub name: String,
}

impl Part {
    /// The README's pin cell: `GPIO4`, `GPIO4 (sda), GPIO5 (scl)`, `to choose`, `I²C 0x44`.
    pub fn pin_cell(&self) -> String {
        if let Some(address) = self.address {
            return format!("I²C {address:#04x}");
        }
        let cells: Vec<String> = self
            .kind
            .pins
            .iter()
            .zip(&self.pins)
            .map(|(spec, pin)| {
                let pin = pin.map_or("to choose".into(), |n| format!("GPIO{n}"));
                if self.kind.pins.len() > 1 { format!("{pin} ({})", spec.role) } else { pin }
            })
            .collect();
        if cells.is_empty() { "-".into() } else { cells.join(", ") }
    }
}

impl Project {
    pub fn battery(&self) -> bool {
        self.answers.power == Power::Battery
    }

    /// On battery without Matter: wakes up, reports and deep-sleeps (`start_sleepy`), over MQTT or
    /// the ESPHome API.
    pub fn sleepy(&self) -> bool {
        self.battery() && self.answers.matter == MatterMode::Off
    }

    /// On battery with Matter: stays reachable as a Matter ICD (`matter-icd`), a sleepy end device
    /// over Thread.
    pub fn icd(&self) -> bool {
        self.battery() && self.answers.matter != MatterMode::Off
    }

    pub fn esphome(&self) -> bool {
        self.answers.esphome != EsphomeMode::Off
    }

    /// Matter over Thread, no Wi-Fi.
    pub fn thread(&self) -> bool {
        self.answers.transport == Transport::Thread
    }

    /// How Home Assistant reaches it, for summaries.
    pub fn ha_link(&self) -> &'static str {
        if self.thread() {
            "Matter over Thread"
        } else if self.esphome() {
            "Home Assistant (ESPHome API)"
        } else {
            "Home Assistant (MQTT)"
        }
    }

    /// The setup ways, defaults filled in.
    pub fn setup(&self) -> Vec<SetupWay> {
        self.answers.setup.clone().unwrap_or_else(|| SetupWay::all(self.board.chip))
    }

    /// Updates install without a click: the answer, else the link's default.
    pub fn auto_update(&self) -> bool {
        self.answers.updates && self.answers.auto_update.unwrap_or(self.thread() || (self.sleepy() && self.esphome()))
    }

    /// The board's RGB LED is a light (showing the status too with `status = "rgb"`).
    pub fn status_led(&self) -> bool {
        self.answers.status_led && !self.battery() && self.board.rgb_led.is_some()
    }

    /// The I²C bus, when a part is on it and its pins are chosen.
    pub fn i2c(&self) -> Option<I2cPins> {
        self.answers.i2c.filter(|_| self.parts.iter().any(|p| p.kind.i2c.is_some()))
    }

    /// How often a sensor's task reads it: plugged in every 30 s; a Matter ICD every
    /// `report_every_min`, as each report wakes its radio.
    pub fn read_every_secs(&self) -> u64 {
        if self.icd() { u64::from(self.answers.report_every_min) * 60 } else { 30 }
    }

    /// The setup button's pin.
    pub fn button_pin(&self) -> u8 {
        self.answers.button_pin.unwrap_or(self.board.boot_pin)
    }

    /// Checks the answers: board, pins, what the chip can do.
    pub fn new(mut answers: Answers, iotkit_path: String) -> Result<Self> {
        let Some(board) = boards::board(&answers.board) else {
            let ids: Vec<_> = boards().iter().map(|b| b.id).collect();
            bail!("unknown board `{}` (one of {ids:?})", answers.board);
        };
        let package = if answers.package.is_empty() { slug(&answers.name, '-') } else { answers.package.clone() };
        ensure!(
            !package.is_empty() && package.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "package name `{package}`: letters, digits, `-` and `_` only"
        );
        let flash = match (board.flash, &answers.flash_size) {
            (Some(f), _) => f.to_string(),
            (None, Some(f)) if f == "4mb" || f == "8mb" => f.clone(),
            (None, Some(f)) => bail!("flash size `{f}`: 4mb or 8mb"),
            (None, None) => "4mb".into(),
        };
        let battery = answers.power == Power::Battery;
        if answers.status == Indicator::Auto {
            // The board's LED; on battery a status is asked for (a WS2812 draws even dark).
            answers.status = match (board.rgb_led, board.plain_led) {
                _ if battery => Indicator::Off,
                (Some(_), _) => Indicator::Rgb,
                (None, Some((pin, _))) => {
                    answers.status_pin = Some(pin);
                    Indicator::Led
                }
                (None, None) => Indicator::Off,
            };
        }
        // The board's own LED keeps its polarity.
        if answers.status == Indicator::Led && answers.status_active.is_none() {
            let low = board.plain_led.is_some_and(|(pin, low)| low && answers.status_pin == Some(pin));
            answers.status_active = Some(if low { Level::Low } else { Level::High });
        }
        if answers.transport == Transport::Thread {
            ensure!(
                board.thread(),
                "Thread needs an 802.15.4 radio: an ESP32-C6 or ESP32-H2 (not the {})",
                board.chip_label
            );
            ensure!(
                !(battery && answers.battery_pin.is_some()),
                "on Thread, the ADC is the random number generator's entropy: no battery level yet (remove `battery_pin`)"
            );
            ensure!(
                answers.matter != MatterMode::Off,
                "Thread devices talk Matter: set `matter = \"fixed\"` or `\"per-device\"`"
            );
            // Matter over Thread fits a 4 MB layout's 1.9 MB slot (about 1.6 MB, updates included).
            ensure!(
                answers.wifi_ssid.is_none() && answers.mqtt_host.is_none() && answers.esphome == EsphomeMode::Off,
                "Thread has no Wi-Fi, MQTT or ESPHome API: remove `wifi_ssid`, `mqtt_host` and `esphome`"
            );
            ensure!(
                answers.updates_url.is_none(),
                "Thread devices update over Matter (`cargo iotkit ota build`, then a Matter OTA provider): remove `updates_url`"
            );
            ensure!(
                answers.setup.is_none() && !answers.console,
                "Thread has no Wi-Fi setup or console: remove `setup` and `console`"
            );
        } else {
            ensure!(board.chip != "esp32h2", "the ESP32-H2 has no Wi-Fi: set `transport = \"thread\"`");
        }
        if answers.matter != MatterMode::Off && answers.transport == Transport::Wifi {
            ensure!(
                board.matter(),
                "Matter needs an ESP32-C6 (the {} has not the RAM or the drivers yet)",
                board.chip_label
            );
            ensure!(
                flash == "8mb",
                "Matter over Wi-Fi needs 8 MB of flash (about 2.1 MB per update slot); Matter over Thread fits in 4 MB"
            );
        }
        if answers.esphome != EsphomeMode::Off {
            ensure!(
                answers.mqtt_host.is_none(),
                "the ESPHome API or MQTT, not both (Home Assistant would show the device twice): remove `mqtt_host`"
            );
        }
        if board.chip == "esp32" {
            ensure!(!battery, "deep sleep is not supported on the ESP32 yet: choose `power = \"plugged\"`");
        }
        if let Some(setup) = &answers.setup {
            ensure!(
                !(board.chip == "esp32" && setup.contains(&SetupWay::Bluetooth)),
                "setup over Bluetooth does not link on the ESP32 yet: remove `bluetooth` from `setup`"
            );
            ensure!(
                !setup.is_empty() || answers.wifi_ssid.is_some(),
                "with no `setup` way, the Wi-Fi network is built in: set `wifi_ssid`"
            );
        }
        ensure!(
            answers.updates || (answers.updates_url.is_none() && answers.auto_update != Some(true)),
            "`updates = false`: remove `updates_url` and `auto_update`"
        );
        // A broker not built in is given on the setup page or the console (Improv gives only the
        // network).
        let access_point = answers.setup.as_ref().is_none_or(|s| s.contains(&SetupWay::AccessPoint));
        ensure!(
            answers.console
                || access_point
                || answers.transport == Transport::Thread
                || answers.esphome != EsphomeMode::Off
                || answers.mqtt_host.is_some(),
            "with no setup page and no console the MQTT broker is built in: set `mqtt_host`"
        );
        ensure!(
            answers.console || answers.esphome != EsphomeMode::PerDevice,
            "a key drawn by each device is read on the console (`api-key`): set `console = true`, or `esphome = \"fixed\"`"
        );
        if let Some(time) = &answers.update_time {
            ensure!(update_time_ok(time), "update_time `{time}`: `HH:MM` on the half hour (02:00, 13:30), or `wake`");
            ensure!(answers.updates, "`update_time` with `updates = false`: remove it");
            let thread = answers.transport == Transport::Thread;
            ensure!(
                answers.esphome != EsphomeMode::Off || thread,
                "`update_time` needs the ESPHome API or Matter (the time comes from the controller): remove it"
            );
            ensure!(!thread || time != "wake", "`update_time = \"wake\"` is for deep sleep over the ESPHome API");
            ensure!(
                !thread || answers.auto_update != Some(false),
                "`update_time` is for updates installed by themselves: remove it, or `auto_update = false`"
            );
        }
        ensure!((1..=24 * 60).contains(&answers.report_every_min), "report_every_min: 1 to 1440");
        // iotkit's `matter-icd` over Wi-Fi waits for an esp-radio release (crates/iotkit/src/lib.rs).
        ensure!(
            !(battery && answers.matter != MatterMode::Off && answers.transport == Transport::Wifi),
            "Matter over Wi-Fi on battery is not supported yet: choose `transport = \"thread\"`, or no Matter (deep sleep over MQTT or the ESPHome API)"
        );

        let mut warnings = Vec::new();
        if answers.esphome != EsphomeMode::Off && answers.matter != MatterMode::Off {
            warnings.push(
                "with Matter, Home Assistant does not discover the ESPHome API: add the device by its address".into(),
            );
        }
        if answers.transport == Transport::Wifi && answers.updates && answers.updates_url.is_none() {
            warnings.push(
                "no `updates_url`: Home Assistant offers no updates (`cargo iotkit ota push` still works)".into(),
            );
        }
        // The board's own: its switches, set in `main`.
        let mut taken: Vec<u8> = board.init.iter().map(|i| i.pin).collect();
        let led = answers.status_led && !battery;
        if let (true, Some(pin)) = (led || answers.status == Indicator::Rgb, board.rgb_led) {
            taken.push(pin);
        }
        match answers.status {
            Indicator::Rgb => {
                ensure!(
                    board.rgb_led.is_some(),
                    "the {} has no RGB LED: set `status = \"led\"` and `status_pin`",
                    board.model
                );
                if battery {
                    warnings.push("a WS2812 draws about 1 mA even dark: on battery, a plain LED (`status = \"led\"`) lasts longer".into());
                }
            }
            Indicator::Led => ensure!(answers.status_pin.is_some(), "`status = \"led\"` needs `status_pin`"),
            Indicator::Off | Indicator::Auto => {
                ensure!(answers.status_pin.is_none(), "`status_pin` needs `status = \"led\"`")
            }
        }
        let take = |pin: u8, kind: PinUse, what: &str, taken: &mut Vec<u8>, warnings: &mut Vec<String>| -> Result<()> {
            match check_pin(board, pin, kind, taken) {
                PinCheck::Ok => {}
                PinCheck::Warn(w) => warnings.push(format!("{what}: {w}")),
                PinCheck::Refused(why) => bail!("{what}: {why}"),
            }
            taken.push(pin);
            Ok(())
        };
        if battery && let Some(pin) = answers.battery_pin {
            take(pin, PinUse::Analog, "Battery level", &mut taken, &mut warnings)?;
        }
        match (answers.status, answers.status_pin) {
            // Wired by the board's maker: no warning for its strapping pin.
            (Indicator::Led, Some(pin)) if board.plain_led.is_some_and(|(led, _)| led == pin) => taken.push(pin),
            (Indicator::Led, Some(pin)) => take(pin, PinUse::Output, "Status LED", &mut taken, &mut warnings)?,
            _ => {}
        }
        let button = answers.button_pin.unwrap_or(board.boot_pin);
        if button != board.boot_pin {
            take(button, PinUse::Input, "Setup button", &mut taken, &mut warnings)?;
        }
        let sleeps = battery && answers.matter == MatterMode::Off;
        if sleeps && !board.wake_pins().is_some_and(|r| r.contains(&button)) {
            warnings.push(match board.wake_pins() {
                Some(r) => format!(
                    "the setup button (GPIO{button}) cannot wake the chip: a button on GPIO{} to GPIO{} (`button_pin`) can",
                    r.start(),
                    r.end()
                ),
                None => format!("the setup button (GPIO{button}) cannot wake the chip"),
            });
        }
        if let Some(bus) = answers.i2c {
            take(bus.sda, PinUse::Output, "I²C SDA", &mut taken, &mut warnings)?;
            take(bus.scl, PinUse::Output, "I²C SCL", &mut taken, &mut warnings)?;
        }
        let mut ids: Vec<String> = vec!["firmware".into(), "status_led".into(), "battery".into()];
        let unique = |base: String, ids: &mut Vec<String>| {
            let mut id = base.clone();
            let mut n = 2;
            while ids.contains(&id) {
                id = format!("{base}_{n}");
                n += 1;
            }
            ids.push(id.clone());
            id
        };
        let mut parts: Vec<Part> = Vec::new();
        for a in &answers.parts {
            let Some(kind) = parts::kind(&a.kind) else {
                let ids: Vec<_> = kinds().iter().map(|k| k.id).collect();
                bail!("unknown part `{}` (one of {ids:?}; `cargo iotkit components` describes them)", a.kind);
            };
            let name = a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| kind.name.to_string());
            let base = slug(&name, '_');
            let base = if base.is_empty() { kind.id.to_string() } else { base };
            let id = unique(base, &mut ids);
            // One entity: the part's id and name; several: theirs after the part's.
            let entities: Vec<Entity> = match kind.entities.as_slice() {
                [one] => vec![Entity { spec: one, id: id.clone(), konst: id.to_uppercase(), name: name.clone() }],
                many => many
                    .iter()
                    .map(|spec| {
                        let id = unique(format!("{id}_{}", spec.key), &mut ids);
                        Entity { spec, konst: id.to_uppercase(), id, name: format!("{name} {}", spec.name) }
                    })
                    .collect(),
            };
            let address = match &kind.i2c {
                None => {
                    ensure!(a.address.is_none(), "{name}: not on the I²C bus (remove `address`)");
                    None
                }
                Some(i2c) => {
                    let address = a.address.unwrap_or(i2c.addresses[0]);
                    let others: Vec<String> = i2c.addresses.iter().map(|a| format!("{a:#04x}")).collect();
                    ensure!(
                        i2c.addresses.contains(&address),
                        "{name}: address {address:#04x}: the {} answers at {}",
                        kind.id,
                        others.join(" or ")
                    );
                    if let Some(other) = parts.iter().find(|p| p.address == Some(address)) {
                        bail!(
                            "{name} and {} are both at {address:#04x} on the I²C bus: give one another `address` ({})",
                            other.name,
                            others.join(" or ")
                        );
                    }
                    Some(address)
                }
            };
            // Without Wi-Fi, ADC1 feeds the random number generator.
            ensure!(
                !(answers.transport == Transport::Thread && kind.pin_use() == PinUse::Analog),
                "{name}: on Thread, the ADC is the random number generator's entropy: no analog parts yet"
            );
            if let Some(role) = a.pins.keys().find(|r| !kind.pins.iter().any(|s| s.role == r.as_str())) {
                let roles: Vec<_> = kind.pins.iter().map(|s| s.role).collect();
                bail!("{name}: no pin `{role}` (its pins: {roles:?})");
            }
            ensure!(a.pin.is_none() || !kind.pins.is_empty(), "{name}: takes no pin (remove `pin`)");
            let mut pins = Vec::new();
            for (i, spec) in kind.pins.iter().enumerate() {
                let pin = a.pins.get(spec.role).copied().or(if i == 0 { a.pin } else { None });
                if let Some(pin) = pin {
                    let what = if kind.pins.len() > 1 { format!("{name} ({})", spec.role) } else { name.clone() };
                    take(pin, spec.pin_use, &what, &mut taken, &mut warnings)?;
                }
                pins.push(pin);
            }
            if battery && answers.matter == MatterMode::Off && !kind.is_reading() {
                let when = match answers.esphome {
                    // Home Assistant has nothing to send them to while it sleeps.
                    EsphomeMode::Off => "commands apply at the next wake-up",
                    _ => "commands reach it only while it is awake",
                };
                warnings.push(format!("{name}: on a battery device, {when}"));
            }
            if answers.transport == Transport::Thread && kind.entities.iter().all(|e| e.matter.is_none()) {
                warnings.push(format!("{name}: Matter has no such device type, so it does not show over Thread"));
            }
            let module = if parts::ident(&id) { id.clone() } else { format!("{id}_part") };
            parts.push(Part { kind, module, name, id, pins, entities, address });
        }

        let matter_setup = match answers.matter {
            MatterMode::Fixed => Some(match (answers.matter_passcode, answers.matter_discriminator) {
                (Some(passcode), Some(discriminator)) => {
                    let s = matter::Setup { passcode, discriminator };
                    ensure!(s.valid(), "matter_passcode / matter_discriminator: not a valid Matter code");
                    s
                }
                _ => matter::Setup::random()?,
            }),
            _ => None,
        };
        let esphome_key = match (answers.esphome, &answers.esphome_key) {
            (EsphomeMode::Fixed, Some(key)) => {
                ensure!(valid_key(key), "esphome_key: 32 bytes in base64 (44 characters, ending with `=`)");
                Some(key.clone())
            }
            (EsphomeMode::Fixed, None) => Some(random_key()?),
            _ => None,
        };
        let light = answers.status_led && !battery && board.rgb_led.is_some();
        if answers.matter != MatterMode::Off && parts.is_empty() && !light {
            warnings.push("no part: over Matter the device has nothing a controller can add: add one".into());
        }
        Ok(Self { answers, board, package, flash, parts, matter_setup, esphome_key, iotkit_path, warnings })
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A new ESPHome API key, in base64 as `iotkit::esphome::ApiKey::parse` reads it.
fn random_key() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
    Ok(base64(&bytes))
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    let mut s = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            s.push(if i <= chunk.len() { char::from(BASE64[(n >> (18 - 6 * i) & 63) as usize]) } else { '=' });
        }
    }
    s
}

/// 32 bytes in base64, as the firmware takes it (the last character's 2 low bits unused).
fn valid_key(key: &str) -> bool {
    let b = key.as_bytes();
    b.len() == 44
        && b[43] == b'='
        && b[..43].iter().all(|c| BASE64.contains(c))
        && BASE64.iter().position(|&c| c == b[42]).is_some_and(|v| v & 3 == 0)
}

/// Where projects get iotkit from, unless `--iotkit-path` names a local checkout.
pub const IOTKIT_GIT: &str = "https://github.com/tanguy-c/iotkit";

pub struct NewArgs {
    pub dir: Option<PathBuf>,
    pub answers: Option<PathBuf>,
    pub defaults: bool,
    pub iotkit_path: Option<PathBuf>,
}

pub fn run(args: NewArgs) -> Result<()> {
    // "": the git repository.
    let iotkit = match args.iotkit_path {
        Some(dir) => {
            ensure!(
                dir.join("crates/iotkit").is_dir(),
                "no iotkit checkout at {} (`--iotkit-path DIR`: the folder with crates/)",
                dir.display()
            );
            dir.canonicalize()?.to_string_lossy().into_owned()
        }
        None => String::new(),
    };
    let mut answers = match &args.answers {
        Some(path) => toml::from_str(&fs::read_to_string(path).with_context(|| path.display().to_string())?)
            .with_context(|| format!("answers file {}", path.display()))?,
        None => Answers::default(),
    };
    if let Some(dir) = &args.dir
        && answers.package.is_empty()
        && let Some(name) = dir.file_name()
    {
        answers.package = name.to_string_lossy().into_owned();
        if args.answers.is_none() {
            answers.name = answers.package.replace(['-', '_'], " ");
        }
    }
    let project = if args.answers.is_some() || args.defaults {
        Project::new(answers, iotkit)?
    } else {
        match ask(answers, iotkit) {
            Ok(p) => p,
            Err(e) if is_cancel(&e) => {
                println!("Cancelled: nothing written.");
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    };
    let dir = args.dir.unwrap_or_else(|| PathBuf::from(&project.package));
    write(&project, &dir)?;
    report(&project, &dir);
    Ok(())
}

fn is_cancel(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<InquireError>(),
        Some(InquireError::OperationCanceled | InquireError::OperationInterrupted)
    )
}

/// Writes the project, its signing keys, and the Matter label.
fn write(p: &Project, dir: &Path) -> Result<()> {
    ensure!(!dir.exists() || fs::read_dir(dir)?.next().is_none(), "{} already exists and is not empty", dir.display());
    for (path, source) in render::FILES {
        let mut text = render::render(path, source, &p.package, p.board.chip, &p.iotkit_path)?;
        match *path {
            "Cargo.toml" => text = codegen::cargo_toml(&text, p)?,
            // Always generated (below): the template's has its placeholder maker and entity ids.
            "src/main.rs" => continue,
            ".cargo/config.toml" => {
                if p.flash != default_flash(p.board.chip) {
                    text = text.replace(
                        &format!(
                            "--flash-size {} --partition-table {}",
                            default_flash(p.board.chip),
                            table(default_flash(p.board.chip))
                        ),
                        &format!("--flash-size {} --partition-table {}", p.flash, table(&p.flash)),
                    );
                }
                // `[env]` is the last table.
                text.push_str(&codegen::update_env(p));
            }
            _ => {}
        }
        if *path == "Cargo.toml" && p.flash != default_flash(p.board.chip) {
            text = text.replace(
                &format!("flash-size = \"{}\"", default_flash(p.board.chip)),
                &format!("flash-size = \"{}\"", p.flash),
            );
        }
        put(&dir.join(path), &text)?;
    }
    for (path, text) in codegen::sources(p)? {
        put(&dir.join(path), &text)?;
    }
    // The generated code, in the project's style (best effort: rustfmt may be missing); it
    // follows `mod` to the other files.
    rustfmt(dir, "src/main.rs");
    put(&dir.join(add::RECORD), &record(&p.answers)?)?;
    put(&dir.join("README.md"), &codegen::readme(p))?;
    if p.answers.ai_agents {
        put(&dir.join("AGENTS.md"), &codegen::agents_md(p))?;
    }
    if let Some(secrets) = codegen::secrets(p) {
        put(&dir.join(".cargo/secrets.toml"), &secrets)?;
    }
    if let Some(setup) = p.matter_setup {
        put(&dir.join("matter-pairing.svg"), &setup.qr_svg()?)?;
    }
    crate::keygen(&dir.join(crate::DEFAULT_KEY), false)?;
    Ok(())
}

/// The flash size the template's Liquid picks for a chip.
fn default_flash(chip: &str) -> &'static str {
    if matches!(chip, "esp32c3" | "esp32") { "4mb" } else { "8mb" }
}

fn table(flash: &str) -> &'static str {
    if flash == "4mb" { "partitions-4mb.csv" } else { "partitions.csv" }
}

/// Formats a generated file in the project's style (best effort: rustfmt may be missing).
fn rustfmt(dir: &Path, file: &str) {
    let _ = std::process::Command::new("rustfmt").args(["--edition", "2024", file]).current_dir(dir).status();
}

/// `iotkit.toml`: the answers, without what `.cargo/secrets.toml` holds.
fn record(a: &Answers) -> Result<String> {
    let mut a = a.clone();
    a.wifi_ssid = None;
    a.wifi_password = None;
    a.mqtt_user = None;
    a.mqtt_password = None;
    a.esphome_key = None;
    a.matter_passcode = None;
    a.matter_discriminator = None;
    Ok(format!(
        "# How `cargo iotkit new` made this project (its secrets are in .cargo/secrets.toml). `cargo iotkit add`\n\
         # reads it and adds its parts here.\n{}",
        toml::to_string(&a)?
    ))
}

fn put(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text).with_context(|| path.display().to_string())
}

fn report(p: &Project, dir: &Path) {
    println!("\nCreated {} in {}", p.answers.name, dir.display());
    for w in &p.warnings {
        println!("  note: {w}");
    }
    if let Some(setup) = p.matter_setup
        && let Ok(qr) = setup.qr_terminal()
    {
        println!("\nMatter pairing code {} (also in matter-pairing.svg):\n{qr}", setup.manual_code_pretty());
    }
    if let Some(key) = &p.esphome_key {
        println!("\nESPHome encryption key (Home Assistant asks for it; also in .cargo/secrets.toml):\n  {key}");
    }
    let flash = if p.board.xtensa() { "  (Xtensa: run `. ~/export-esp.sh` first)" } else { "" };
    println!(
        "\nNext:\n  cd {}\n  cargo iotkit flash --release{flash}\n\nWiring and setup: README.md. {}",
        dir.display(),
        if p.parts.is_empty() {
            "Add parts with `cargo iotkit add` (`cargo iotkit components` lists them)."
        } else {
            "Each part is a module in src/parts/, its code left to write a TODO."
        }
    );
    if p.answers.ai_agents {
        println!("AI coding agents: AGENTS.md.");
    }
}

// The questions.

fn ask(mut a: Answers, iotkit_path: String) -> Result<Project> {
    println!("A few questions about your device (Enter keeps the default, Esc cancels).\n");

    // 1. What it is.
    a.name = Text::new("What are you making?")
        .with_default(&a.name)
        .with_help_message("Its name in Home Assistant, e.g. \"Garden sensor\" or \"Bedroom blind\"")
        .with_validator(|s: &str| {
            Ok(if slug(s, '-').is_empty() {
                Validation::Invalid("a name with letters".into())
            } else {
                Validation::Valid
            })
        })
        .prompt()?;
    if a.package.is_empty() {
        a.package = slug(&a.name, '-');
    }
    a.maker = Text::new("Your name or brand?")
        .with_default(&a.maker)
        .with_help_message("Shown as the manufacturer in Home Assistant")
        .prompt()?;

    // 2. The board.
    let labels: Vec<&str> = boards().iter().map(|b| b.label).collect();
    let start = boards().iter().position(|b| b.id == a.board).unwrap_or(0);
    let label = Select::new("Which board?", labels.clone())
        .with_starting_cursor(start)
        .with_page_size(12)
        .with_help_message("Type to search (a maker, a chip, a name). Matter needs a C6 with 8 MB.")
        .prompt()?;
    let board = &boards()[labels.iter().position(|l| *l == label).unwrap_or(0)];
    a.board = board.id.into();
    if board.flash.is_none() {
        let f = Select::new("How much flash memory does it have?", vec!["4 MB", "8 MB or more"])
            .with_help_message("On the module's label or in the board's datasheet (N4 = 4 MB, N8 = 8 MB)")
            .prompt()?;
        a.flash_size = Some(if f.starts_with('4') { "4mb" } else { "8mb" }.into());
    }
    a.transport = if !board.wifi() {
        println!("  (the {} has no Wi-Fi: Matter over Thread)", board.chip_label);
        Transport::Thread
    } else if board.thread()
        && Select::new(
            "Which network?",
            vec!["Wi-Fi: Home Assistant (MQTT or ESPHome), Matter optional", "Thread: Matter only, low power mesh"],
        )
        .with_help_message("Thread needs a border router (e.g. Home Assistant's OpenThread Border Router add-on)")
        .prompt()?
        .starts_with("Thread")
    {
        Transport::Thread
    } else {
        Transport::Wifi
    };
    let thread = a.transport == Transport::Thread;

    // 3. Power.
    let (battery_label, battery_help) = if thread {
        (
            "Battery: a sleepy Thread device, reachable within seconds",
            "Its radio listens every few seconds and the chip sleeps in between: months on a battery",
        )
    } else {
        (
            "Battery: wakes up, reports, sleeps",
            "A battery device sleeps between reports to last months, and answers commands only when awake",
        )
    };
    let battery = Select::new("How is it powered?", vec!["Plugged in (USB or mains): always on", battery_label])
        .with_help_message(battery_help)
        .prompt()?
        .starts_with("Battery");
    a.power = if battery { Power::Battery } else { Power::Plugged };

    // 4. Connectivity.
    println!("\nHow it talks to your home:");
    if thread {
        println!("  (Thread: paired over Bluetooth from a Matter controller, updated over Matter)");
    } else if Confirm::new("Enter your Wi-Fi now?")
        .with_default(false)
        .with_help_message(
            "Saved in .cargo/secrets.toml (not committed). Otherwise you set it from a phone at the first boot.",
        )
        .prompt()?
    {
        a.wifi_ssid = Some(Text::new("Wi-Fi name (SSID)?").with_validator(non_empty).prompt()?);
        a.wifi_password = Some(
            Password::new("Wi-Fi password?")
                .with_display_mode(PasswordDisplayMode::Masked)
                .without_confirmation()
                .prompt()?,
        );
    }
    a.esphome = EsphomeMode::Off;
    if !thread
        && Select::new(
            "How does Home Assistant reach it?",
            vec![
                "MQTT, through a broker (the Mosquitto add-on)",
                "ESPHome API: Home Assistant's ESPHome integration, no broker",
            ],
        )
        .with_help_message(if battery {
            "Both show the same entities. On battery, MQTT's broker keeps commands until the device wakes up; over ESPHome they only reach it awake."
        } else {
            "Both show the same entities. ESPHome: Home Assistant finds the device and connects to it directly."
        })
        .prompt()?
        .starts_with("ESPHome")
    {
        let fixed = Select::new(
            "Which encryption key?",
            vec!["One key built in: shown now", "A new key made by each device at its first boot"],
        )
        .with_help_message(
            "Home Assistant asks for it when adding the device. A device's own: `api-key` on the console, or the app shows `iotkit::esphome::api_key()`",
        )
        .prompt()?
        .starts_with("One");
        a.esphome = if fixed { EsphomeMode::Fixed } else { EsphomeMode::PerDevice };
    }
    let broker = if a.esphome != EsphomeMode::Off || thread {
        String::new()
    } else {
        Text::new("Home Assistant's MQTT broker (IP or host name)?")
            .with_help_message(
                "The Mosquitto add-on: Home Assistant's address. Leave empty to give it on the setup page (or the console: `mqtt HOST`).",
            )
            .prompt()?
    };
    if !broker.trim().is_empty() {
        a.mqtt_host = Some(broker.trim().into());
        let user =
            Text::new("MQTT user?").with_help_message("Leave empty if the broker takes anonymous clients").prompt()?;
        if !user.is_empty() {
            a.mqtt_user = Some(user);
            a.mqtt_password = Some(Password::new("MQTT password?").without_confirmation().prompt()?);
        }
    }
    a.matter = MatterMode::Off;
    if !thread && !board.matter() {
        println!("  (Matter: not on the {} yet)", board.chip_label);
    } else if battery && !thread {
        println!("  (Matter: not on battery over Wi-Fi yet; Thread devices take it on battery)");
    } else if thread
        || Confirm::new("Also work with Matter (any Matter controller)?")
            .with_default(false)
            .with_help_message(
                "Next to Home Assistant's link. Paired with a code; uses about 2 MB of flash and more RAM.",
            )
            .prompt()?
    {
        let fixed = Select::new(
            "Which Matter pairing code?",
            vec!["One fixed code: shown now, print it as a label", "A new code drawn by each device at its first boot"],
        )
        .with_help_message("A fixed code is the same on every device flashed from this project")
        .prompt()?
        .starts_with("One");
        a.matter = if fixed { MatterMode::Fixed } else { MatterMode::PerDevice };
    }
    if battery {
        ask_battery(&mut a, board, thread)?;
    }
    let package = if a.package.is_empty() { slug(&a.name, '-') } else { a.package.clone() };
    let default_url =
        a.mqtt_host.as_deref().map(|h| format!("http://{h}:8080/{package}/manifest.json")).unwrap_or_default();
    ask_updates(&mut a, &default_url, thread, battery)?;
    if !thread {
        ask_extras(&mut a, board)?;
    }

    // 5. Parts.
    a.status_led = board.rgb_led.is_some()
        && !battery
        && Confirm::new("Keep the board's RGB LED as a light?")
            .with_default(true)
            .with_help_message("A light in your home app; a short press of BOOT toggles it")
            .prompt()?;
    let mut taken: Vec<u8> = a.battery_pin.into_iter().collect();
    // The first is the default: the board's LED when plugged in, nothing on battery.
    let rgb = if a.status_led { "The board's RGB LED (over the light)" } else { "The board's RGB LED" };
    let plain = board.plain_led.map(|(pin, _)| format!("The board's LED (GPIO{pin})"));
    let shows = match (board.rgb_led.is_some(), plain.as_deref(), battery) {
        (true, _, false) => vec![rgb, "A LED on a pin", "Nothing"],
        (true, _, true) => vec!["Nothing", "A LED on a pin", rgb],
        (false, Some(own), false) => vec![own, "A LED on a pin", "Nothing"],
        (false, Some(own), true) => vec!["Nothing", "A LED on a pin", own],
        (false, None, _) => vec!["Nothing", "A LED on a pin"],
    };
    let show = Select::new("What shows its status (pairing, connecting, updates)?", shows)
        .with_help_message("Blinks while it waits to be set up, and while it connects or updates")
        .prompt()?;
    let own = plain.as_deref() == Some(show);
    a.status = match show {
        "Nothing" => Indicator::Off,
        "A LED on a pin" => Indicator::Led,
        _ if own => Indicator::Led,
        _ => Indicator::Rgb,
    };
    if (a.status_led || a.status == Indicator::Rgb)
        && let Some(led) = board.rgb_led
    {
        taken.push(led);
    }
    a.status_pin = None;
    a.status_active = None;
    taken.extend(board.init.iter().map(|i| i.pin));
    if own {
        a.status_pin = board.plain_led.map(|(pin, _)| pin);
        taken.extend(a.status_pin);
    } else if a.status == Indicator::Led {
        a.status_pin =
            ask_pin(board, "Which GPIO for the LED? (to ground through a resistor)", PinUse::Output, &taken, false)?;
        taken.extend(a.status_pin);
    }
    a.button_pin = None;
    if battery
        && a.matter == MatterMode::Off
        && let Some(wake) = board.wake_pins()
        && !wake.contains(&board.boot_pin)
    {
        let question = format!(
            "A wake-up button on GPIO{}-{}? (BOOT, GPIO{}, cannot wake the chip)",
            wake.start(),
            wake.end(),
            board.boot_pin
        );
        a.button_pin = ask_pin(board, &question, PinUse::Input, &taken, true)?;
        taken.extend(a.button_pin);
    }
    println!("\nWhat does it do? Add its parts one by one (sensors, relays, buttons...).");
    a.parts.clear();
    a.i2c = None;
    while let Some(part) = ask_part(board, thread, &mut taken, a.parts.is_empty(), &mut a.i2c)? {
        a.parts.push(part);
    }

    // 6. Tooling.
    a.ai_agents = Confirm::new("Set up for AI coding agents (Claude Code, Codex, Cursor, Copilot…)?")
        .with_default(false)
        .with_help_message("Adds AGENTS.md")
        .prompt()?;

    let project = Project::new(a, iotkit_path)?;
    summary(&project);
    if !Confirm::new("Create it?").with_default(true).prompt()? {
        return Err(InquireError::OperationCanceled.into());
    }
    Ok(project)
}

/// A part: its component, name and pins (`None`: done). `taken` gets its pins; the first I²C
/// part asks for the bus.
pub(crate) fn ask_part(
    board: &'static Board,
    thread: bool,
    taken: &mut Vec<u8>,
    first: bool,
    bus: &mut Option<I2cPins>,
) -> Result<Option<PartAnswer>> {
    let mut options: Vec<String> = vec!["Done".into()];
    let mut choices: Vec<&Kind> = Vec::new();
    for g in [Group::Measures, Group::Detects, Group::Controls, Group::Inputs] {
        // On Thread, the ADC feeds the random number generator.
        for k in kinds().iter().filter(|k| k.group == g && !(thread && k.analog())) {
            options.push(format!("{}: {}", g.label(), k.label));
            choices.push(k);
        }
    }
    let prompt = if first { "Add a part?" } else { "Add another part?" };
    let choice = Select::new(prompt, options.clone())
        .with_page_size(12)
        .with_help_message("Type to filter. Each part becomes an entity and a module in src/parts/.")
        .prompt()?;
    if choice == "Done" {
        return Ok(None);
    }
    let kind = choices[options.iter().position(|o| *o == choice).expect("chosen") - 1];
    let name = Text::new("Its name?")
        .with_default(kind.name)
        .with_help_message("As Home Assistant shows it, e.g. \"Kitchen temperature\"")
        .prompt()?;
    let mut part = ask_pins(board, kind, name, taken)?;
    if let Some(i2c) = &kind.i2c {
        if bus.is_none() {
            *bus = ask_bus(board, taken)?;
        }
        if i2c.addresses.len() > 1 {
            let labels: Vec<String> = i2c.addresses.iter().map(|a| format!("{a:#04x}")).collect();
            let chosen = Select::new("Its I²C address?", labels.clone())
                .with_help_message(&format!("The first is the chip's default; the other: {}", kind.wiring))
                .prompt()?;
            let at = labels.iter().position(|l| *l == chosen).unwrap_or(0);
            part.address = (at > 0).then_some(i2c.addresses[at]);
        }
    }
    Ok(Some(part))
}

/// The I²C bus's pins (`None`: to choose later).
fn ask_bus(board: &'static Board, taken: &mut Vec<u8>) -> Result<Option<I2cPins>> {
    println!("  The I²C bus, shared by every I²C part (most breakout boards have its pull-ups):");
    let Some(sda) = ask_pin(board, "Which GPIO for SDA?", PinUse::Output, taken, true)? else { return Ok(None) };
    taken.push(sda);
    let Some(scl) = ask_pin(board, "Which GPIO for SCL?", PinUse::Output, taken, false)? else { return Ok(None) };
    taken.push(scl);
    Ok(Some(I2cPins { sda, scl }))
}

/// A part's pins (each may be left to choose later).
fn ask_pins(board: &'static Board, kind: &Kind, name: String, taken: &mut Vec<u8>) -> Result<PartAnswer> {
    let mut part =
        PartAnswer { kind: kind.id.into(), name: Some(name), pin: None, pins: BTreeMap::new(), address: None };
    for (i, spec) in kind.pins.iter().enumerate() {
        let name = part.name.as_deref().unwrap_or_default();
        let question = if kind.pins.len() > 1 {
            format!("Which GPIO for {name}'s {}? ({})", spec.role, kind.wiring)
        } else {
            format!("Which GPIO for {name}? ({})", kind.wiring)
        };
        let pin = ask_pin(board, &question, spec.pin_use, taken, true)?;
        taken.extend(pin);
        match pin {
            Some(n) if i > 0 || kind.pins.len() > 1 => {
                part.pins.insert(spec.role.into(), n);
            }
            pin => part.pin = pin,
        }
    }
    Ok(part)
}

/// `HH:MM` on the half hour, or `wake`: iotkit's `IOTKIT_UPDATE_TIME`.
fn update_time_ok(s: &str) -> bool {
    if s == "wake" {
        return true;
    }
    let Some((h, m)) = s.split_once(':') else { return false };
    h.len() == 2 && h.parse::<u8>().is_ok_and(|h| h < 24) && (m == "00" || m == "30")
}

fn non_empty(s: &str) -> Result<Validation, inquire::CustomUserError> {
    Ok(if s.trim().is_empty() { Validation::Invalid("needed".into()) } else { Validation::Valid })
}

/// A GPIO number, checked for the chip (refused pins are asked again, warnings shown).
/// The battery questions: how often a sleeping device reports, and its level's pin.
/// Updates: whether, from where, and whether they install by themselves.
fn ask_updates(a: &mut Answers, default_url: &str, thread: bool, battery: bool) -> Result<()> {
    println!("\nUpdates:");
    a.updates = Confirm::new(if thread { "Update it over Matter?" } else { "Update it over the network?" })
        .with_default(true)
        .with_help_message(if thread {
            "Signed updates from a Matter OTA provider. Without: flashed over USB only, about 20 KB of RAM and 170 KB of flash less"
        } else {
            "Signed updates from Home Assistant and `cargo iotkit ota push`. Without: flashed over USB only, smaller"
        })
        .prompt()?;
    a.updates_url = None;
    a.auto_update = None;
    a.update_time = None;
    if !a.updates {
        return Ok(());
    }
    if !thread {
        let url = Text::new("Where will you publish updates (manifest URL)?")
            .with_default(default_url)
            .with_help_message(
                "Home Assistant then offers new versions; `cargo iotkit ota publish` fills it. Leave empty to skip.",
            )
            .prompt()?;
        a.updates_url = (!url.trim().is_empty()).then(|| url.trim().to_string());
    }
    let esphome = a.esphome != EsphomeMode::Off;
    // The link's default (`Project::auto_update`): on where nobody can click.
    let default = thread || (battery && esphome);
    let auto = Confirm::new("Install updates by themselves?")
        .with_default(default)
        .with_help_message(if thread {
            "Yes: it asks its controllers' providers every day. No: only the updates its controller announces"
        } else {
            "Yes: an *Auto-update* switch in Home Assistant, on. No: a release waits for a click on *Install*"
        })
        .prompt()?;
    a.auto_update = (auto != default).then_some(auto);
    if auto && esphome {
        let time = Text::new("At what time? (HH:MM on the half hour, or `wake`: once found)")
            .with_default("02:00")
            .with_help_message("Home Assistant gives the time; *Update time* in Home Assistant changes it")
            .with_validator(|s: &str| {
                Ok(if update_time_ok(s.trim()) {
                    Validation::Valid
                } else {
                    Validation::Invalid("`HH:MM` on the half hour (02:00, 13:30), or `wake`".into())
                })
            })
            .prompt()?;
        a.update_time = (time.trim() != "02:00").then(|| time.trim().to_string());
    }
    if auto && thread {
        let time = Text::new("At what time? (HH:MM on the half hour, or empty: once offered)")
            .with_help_message(
                "In the controller's time: Home Assistant's Matter Server gives it (`time_sync`). Without it, once offered",
            )
            .with_validator(|s: &str| {
                let s = s.trim();
                Ok(if s.is_empty() || (s != "wake" && update_time_ok(s)) {
                    Validation::Valid
                } else {
                    Validation::Invalid("`HH:MM` on the half hour (02:00, 13:30), or empty".into())
                })
            })
            .prompt()?;
        a.update_time = (!time.trim().is_empty()).then(|| time.trim().to_string());
    }
    Ok(())
}

/// What to leave out of a Wi-Fi device: setup ways, the console.
fn ask_extras(a: &mut Answers, board: &'static Board) -> Result<()> {
    let all = SetupWay::all(board.chip);
    let labels: Vec<&str> = all.iter().map(|w| w.label()).collect();
    let help = if a.wifi_ssid.is_some() {
        "Space toggles. The Wi-Fi network is built in: none is needed"
    } else {
        "Space toggles. At least one, as the Wi-Fi network is not built in"
    };
    let ssid = a.wifi_ssid.is_some();
    let chosen = MultiSelect::new("How can it be given a Wi-Fi network at its first boot?", labels.clone())
        .with_all_selected_by_default()
        .with_help_message(help)
        .with_validator(move |c: &[inquire::list_option::ListOption<&&str>]| {
            Ok(if c.is_empty() && !ssid {
                Validation::Invalid("at least one, or enter the Wi-Fi network now".into())
            } else {
                Validation::Valid
            })
        })
        .prompt()?;
    let setup: Vec<SetupWay> = all.iter().copied().filter(|w| chosen.contains(&w.label())).collect();
    // A device's own ESPHome key, or a broker with no setup page to give it: the console reads or sets it.
    let needs_console = a.esphome == EsphomeMode::PerDevice
        || (a.esphome == EsphomeMode::Off && a.mqtt_host.is_none() && !setup.contains(&SetupWay::AccessPoint));
    a.setup = (setup != all).then_some(setup);
    a.console = needs_console
        || Confirm::new("Add the USB console (`help`)?")
            .with_default(false)
            .with_help_message(
                "Diagnostics and settings by hand over USB, for development. Without: a little smaller, and the app gets the USB port",
            )
            .prompt()?;
    Ok(())
}

fn ask_battery(a: &mut Answers, board: &'static Board, thread: bool) -> Result<()> {
    let every = Select::new(
        "How often should it report?",
        vec!["Every minute", "Every 5 minutes", "Every 15 minutes", "Every hour"],
    )
    .with_starting_cursor(1)
    .with_help_message(if a.matter == MatterMode::Off {
        "It wakes up, reports and sleeps again. More often drains the battery faster"
    } else {
        "How often it reads its sensors (each report wakes its radio); commands still arrive within seconds"
    })
    .prompt()?;
    a.report_every_min = match every {
        "Every minute" => 1,
        "Every 15 minutes" => 15,
        "Every hour" => 60,
        _ => 5,
    };
    if thread {
        // The ADC feeds the random number generator.
        return Ok(());
    }
    if Confirm::new("Report the battery level?")
        .with_default(true)
        .with_help_message("Needs two resistors from the battery to an analog pin")
        .prompt()?
    {
        a.battery_pin = ask_pin(board, "Analog pin for the battery level?", PinUse::Analog, &[], false)?;
        let curve =
            Select::new("Which battery?", vec!["LiPo / Li-ion cell (3.0–4.2 V)", "2 × AA alkaline (2.0–3.2 V)"])
                .prompt()?;
        a.battery_curve = if curve.starts_with('2') { Curve::TwoAa } else { Curve::Lipo };
    }
    Ok(())
}

fn ask_pin(board: &'static Board, question: &str, kind: PinUse, taken: &[u8], optional: bool) -> Result<Option<u8>> {
    let taken = taken.to_vec();
    let help = if optional { "A GPIO number, or empty to decide later" } else { "A GPIO number" };
    let answer = Text::new(question)
        .with_help_message(help)
        .with_validator(move |s: &str| {
            let s = s
                .trim()
                .trim_start_matches(['G', 'g'])
                .trim_start_matches(['P', 'p'])
                .trim_start_matches(['I', 'i'])
                .trim_start_matches(['O', 'o']);
            if s.is_empty() {
                return Ok(if optional { Validation::Valid } else { Validation::Invalid("a GPIO number".into()) });
            }
            let Ok(pin) = s.parse::<u8>() else { return Ok(Validation::Invalid("a GPIO number, e.g. 4".into())) };
            Ok(match check_pin(board, pin, kind, &taken) {
                PinCheck::Refused(why) => Validation::Invalid(why.into()),
                _ => Validation::Valid,
            })
        })
        .prompt()?;
    let digits = answer.trim().trim_start_matches(|c: char| c.is_ascii_alphabetic());
    if digits.is_empty() {
        return Ok(None);
    }
    let pin: u8 = digits.parse()?;
    if let PinCheck::Warn(w) = check_pin(board, pin, kind, &[]) {
        println!("  note: {w}");
    }
    Ok(Some(pin))
}

fn summary(p: &Project) {
    let a = &p.answers;
    println!("\n{} ({}), by {}", a.name, p.package, a.maker);
    println!("  board    {} ({}, {} flash)", p.board.model, p.board.chip_label, p.flash.to_uppercase());
    if p.sleepy() {
        println!("  power    battery, reports every {} min", a.report_every_min);
    } else if p.icd() {
        println!("  power    battery, a Matter ICD (`matter-icd`), reads every {} min", a.report_every_min);
    } else {
        println!("  power    plugged in");
    }
    let mut links = vec![p.ha_link()];
    match a.matter {
        MatterMode::Fixed => links.push("Matter (fixed code)"),
        MatterMode::PerDevice => links.push("Matter (code per device)"),
        MatterMode::Off => {}
    }
    println!("  talks to {}", links.join(" + "));
    println!("  Wi-Fi    {}", if a.wifi_ssid.is_some() { "built in" } else { "set up from a phone at first boot" });
    for part in &p.parts {
        println!(
            "  part     {} ({}), {}",
            part.name,
            part.kind.label,
            part.pin_cell().replace("to choose", "pin to choose")
        );
    }
    if a.ai_agents {
        println!("  agents   AGENTS.md");
    }
    for w in &p.warnings {
        println!("  note     {w}");
    }
    println!();
}

impl Board {
    fn xtensa(&self) -> bool {
        matches!(self.chip, "esp32s3" | "esp32")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(toml: &str) -> Project {
        Project::new(toml::from_str(toml).unwrap(), "/opt/iotkit".into()).unwrap()
    }

    /// Every Rust file of the project, `src/main.rs` first.
    fn code(p: &Project) -> String {
        codegen::sources(p).unwrap().into_iter().map(|(path, text)| format!("// == {path}\n{text}")).collect()
    }

    #[test]
    fn checks() {
        let err = |toml: &str| Project::new(toml::from_str(toml).unwrap(), "/x".into()).err().unwrap().to_string();
        assert!(err("board = \"esp32c3-devkitm-1\"\nmatter = \"fixed\"").contains("ESP32-C6"));
        assert!(err("board = \"esp32-devkitc\"\npower = \"battery\"").contains("deep sleep"));
        assert!(err("[[parts]]\nkind = \"relay\"\npin = 12").contains("USB"));
        assert!(err("status_led = true\n[[parts]]\nkind = \"relay\"\npin = 8").contains("already used"));
        assert!(err("[[parts]]\nkind = \"kettle\"").contains("unknown part"));
        let p =
            project("name = \"Garden sensor\"\n[[parts]]\nkind = \"temperature\"\n[[parts]]\nkind = \"temperature\"");
        assert_eq!(p.package, "garden-sensor");
        assert_eq!(p.parts[1].id, "temperature_2");
        assert_eq!(p.parts[1].entities[0].konst, "TEMPERATURE_2");
    }

    #[test]
    fn plugged_main() {
        let p = project(include_str!("../../answers/c6.toml"));
        let main = code(&p);
        assert!(main.contains("static PUMP: Switch = Switch::new(\"pump\", \"Pump\");"), "{main}");
        // Ready-made parts where there is one, a TODO elsewhere.
        assert!(main.contains("parts::pump::spawn(spawner, p.GPIO4);"), "{main}");
        assert!(main.contains("spawner.spawn(task(Output::new(pin, Level::Low, OutputConfig::default())).unwrap());"));
        assert!(main.contains("components::relay(&PUMP, pin, Active::High).await"), "{main}");
        assert!(main.contains("components::button(&BUTTON, pin, Active::Low"), "{main}");
        assert!(main.contains("components::poll(&SOIL_MOISTURE"), "{main}");
        assert!(main.contains("TODO(temperature):"));
        assert!(main.contains("iotkit::start(spawner, board, &CONFIG).await;"));
        // The RGB LED: a light that shows the status too, one task.
        assert!(main.contains("rgb_status_light(&LED, led, 1, iotkit::status::Show::All, true)"), "{main}");
        assert!(!main.contains("status_task"), "{main}");
        let secrets = codegen::secrets(&p).unwrap();
        assert!(secrets.contains(&format!("MATTER_PASSCODE = \"{}\"", p.matter_setup.unwrap().passcode)));
        let cargo = codegen::cargo_toml(
            &render::render("Cargo.toml", render::FILES[1].1, &p.package, "esp32c6", "/opt/iotkit").unwrap(),
            &p,
        )
        .unwrap();
        assert!(cargo.contains("default     = [\"matter-wifi\"]"), "{cargo}");
        // The default features, and the console on top.
        assert!(
            cargo.contains("{ path = \"/opt/iotkit/crates/iotkit\", features = [\"esp32c6\", \"ha-update\", \"smartled\", \"remember\", \"console\"]"),
            "{cargo}"
        );
        assert!(codegen::readme(&p).contains(&p.matter_setup.unwrap().manual_code_pretty()));
    }

    #[test]
    fn battery_main() {
        let p = project(include_str!("../../answers/c3.toml"));
        let main = code(&p);
        assert!(main.contains("start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP)"), "{main}");
        assert!(main.contains("Duration::from_secs(900)"));
        // The soil probe reads the ADC after the battery level, once per wake-up.
        assert!(main.contains("Battery::new(p.ADC1.reborrow(), p.GPIO3, 2.0).curve(BatteryCurve::ALKALINE_2AA)"));
        assert!(main.contains("parts::soil_moisture::read(p.GPIO2, p.ADC1).await;"), "{main}");
        assert!(main.contains("SOIL_MOISTURE.set(value);") && main.contains("TODO(soil_moisture): calibrate"));
        // An I²C chip on battery: the bus lent to it in `main`, no mutex.
        assert!(main.contains("let mut bus = i2c::bus(p.I2C0, p.GPIO7, p.GPIO10);"), "{main}");
        assert!(main.contains("parts::air::read(&mut bus).await;") && !main.contains("Mutex"), "{main}");
        assert!(main.contains("match components::i2c::sht4x(i2c, ADDR).await {"), "{main}");
        assert!(!main.contains("led_task") && !main.contains("Ticker"));
        let cargo = codegen::cargo_toml(
            &render::render("Cargo.toml", render::FILES[1].1, &p.package, "esp32c3", "/opt/iotkit").unwrap(),
            &p,
        )
        .unwrap();
        assert!(cargo.contains("\"deep-sleep\"") && !cargo.contains("matter-wifi"), "{cargo}");
    }

    /// One `TODO(<id>)` per part, its hints in `main` on a battery device, and every comment
    /// within the project's `max_width`.
    #[test]
    fn todos() {
        for (answers, battery) in
            [(include_str!("../../answers/c6.toml"), false), (include_str!("../../answers/c3.toml"), true)]
        {
            let p = project(answers);
            let main = code(&p);
            // One TODO per part, but none for a ready-made part (the soil probe keeps one: calibrate).
            for (part, ready) in p.parts.iter().zip(codegen::ready(&p)) {
                let want = usize::from(!ready || part.kind.analog());
                assert_eq!(main.matches(&format!("TODO({}):", part.id)).count(), want, "{}", part.id);
            }
            assert!(!main.contains("TODO:"), "{main}");
            for line in main.lines().filter(|l| l.trim_start().starts_with("//")) {
                assert!(line.chars().count() <= codegen::MAX_WIDTH, "{line}");
            }
            if battery {
                // Read once per wake-up, in `main`, with their pins.
                assert!(main.contains("parts::temperature::read(p.GPIO4).await;"), "{main}");
                assert!(main.contains("with_sda(_pin)"), "{main}");
                assert!(
                    main.contains("GATE.set(Input::new(pin, InputConfig::default().with_pull(Pull::Up)).is_high());")
                );
            }
        }
    }

    /// What can be left out (updates, setup ways, the console), auto-update on every link, and the
    /// combinations refused.
    #[test]
    fn slim() {
        let err = |toml: &str| Project::new(toml::from_str(toml).unwrap(), "/x".into()).err().unwrap().to_string();
        assert!(err("updates = false\nupdates_url = \"http://h/m.json\"").contains("updates = false"));
        assert!(err("updates = false\nauto_update = true").contains("updates = false"));
        assert!(err("setup = []").contains("wifi_ssid"));
        assert!(err("setup = [\"serial\"]").contains("mqtt_host"));
        assert!(err("esphome = \"per-device\"").contains("api-key"));
        assert!(err("update_time = \"02:15\"\nesphome = \"fixed\"").contains("half hour"));
        assert!(err("update_time = \"02:00\"").contains("ESPHome API"));
        assert!(err("board = \"esp32-devkitc\"\nsetup = [\"bluetooth\"]").contains("ESP32"));
        assert!(err("transport = \"thread\"\nmatter = \"fixed\"\nconsole = true").contains("Thread"));
        let features = |p: &Project| {
            let rendered = render::render("Cargo.toml", render::FILES[1].1, &p.package, p.board.chip, "/x").unwrap();
            codegen::cargo_toml(&rendered, p).unwrap()
        };

        // Defaults: the line as before, no `[env]` added.
        let p = project("");
        assert!(features(&p).contains("features = [\"esp32c6\", \"ha-update\", \"smartled\", \"remember\"] }"));
        assert!(codegen::update_env(&p).is_empty() && !p.auto_update());

        let p = project(include_str!("../../answers/xiao.toml"));
        let cargo = features(&p);
        assert!(
            cargo.contains("default-features = false, features = [\"esp32c6\", \"ha-update\", \"mqtt\", \"wifi\", \"softap\", \"ota-push\"]"),
            "{cargo}"
        );
        let main = code(&p);
        // No console or Improv over USB: the app keeps the port.
        assert!(!main.contains("bt: p.BT") && !main.contains("usb: p.USB_DEVICE"), "{main}");
        assert!(codegen::update_env(&p).contains("IOTKIT_AUTO_UPDATE = \"on\""));
        assert!(codegen::readme(&p).contains("once found"));

        let p = project(include_str!("../../answers/c3.toml"));
        let cargo = features(&p);
        assert!(!cargo.contains("ha-update") && !cargo.contains("ota-push"), "{cargo}");
        assert!(codegen::readme(&p).contains("Built without updates"));

        // Over Thread: no `matter-ota`; auto-update off leaves the controller's announcements.
        let p = project("transport = \"thread\"\nmatter = \"fixed\"\nupdates = false");
        assert!(features(&p).contains("[\"esp32c6\", \"matter-thread\", \"smartled\", \"remember\"]"));
        let p = project("transport = \"thread\"\nmatter = \"fixed\"\nauto_update = false");
        assert!(codegen::update_env(&p).contains("IOTKIT_AUTO_UPDATE = \"off\"") && !p.auto_update());
        assert!(codegen::readme(&p).contains("announces"));
        // An update time over Matter: the controller's time, never `wake`, not with auto-update off.
        let p = project(include_str!("../../answers/h2.toml"));
        let cargo = features(&p);
        assert!(cargo.contains("[\"esp32h2\", \"matter-thread\", \"matter-ota\""), "{cargo}");
        let env = codegen::update_env(&p);
        assert!(env.contains("IOTKIT_UPDATE_TIME = \"03:00\"") && !env.contains("IOTKIT_AUTO_UPDATE"), "{env}");
        assert!(codegen::readme(&p).contains("03:00 in the controller's time"));
        let thread = "transport = \"thread\"\nmatter = \"fixed\"\n";
        assert!(err(&format!("{thread}update_time = \"wake\"")).contains("deep sleep"));
        assert!(err(&format!("{thread}update_time = \"03:00\"\nauto_update = false")).contains("by themselves"));
        assert!(err(&format!("{thread}update_time = \"03:00\"\nupdates = false")).contains("updates = false"));
    }

    /// The ESPHome API instead of MQTT: its feature, a built-in key, and the combinations refused.
    #[test]
    fn esphome() {
        let err = |toml: &str| Project::new(toml::from_str(toml).unwrap(), "/x".into()).err().unwrap().to_string();
        assert!(err("esphome = \"per-device\"\nmqtt_host = \"10.0.0.1\"").contains("not both"));
        assert!(err("esphome = \"fixed\"\nesphome_key = \"abc=\"").contains("base64"));

        let p = project(include_str!("../../answers/esphome.toml"));
        let key = p.esphome_key.clone().unwrap();
        assert!(valid_key(&key), "{key}");
        assert!(codegen::secrets(&p).unwrap().contains(&format!("ESPHOME_API_KEY = \"{key}\"")));
        assert!(!codegen::secrets(&p).unwrap().contains("MQTT_HOST"));
        let cargo = codegen::cargo_toml(
            &render::render("Cargo.toml", render::FILES[1].1, &p.package, "esp32c6", "/opt/iotkit").unwrap(),
            &p,
        )
        .unwrap();
        // The defaults but MQTT and the console.
        assert!(
            cargo.contains(
                "default-features = false, features = [\"esp32c6\", \"ha-update\", \"smartled\", \"remember\", \"esphome-api\", \"improv-ble\", \"wifi\", \"softap\", \"improv-serial\", \"ota-push\"]"
            ),
            "{cargo}"
        );
        let env = codegen::update_env(&p);
        assert!(env.contains("IOTKIT_AUTO_UPDATE = \"on\"") && env.contains("IOTKIT_UPDATE_TIME = \"03:30\""), "{env}");
        assert!(codegen::readme(&p).contains("at 03:30"));
        let readme = codegen::readme(&p);
        assert!(
            readme.contains("ESPHOME_API_KEY") && !readme.contains(&key) && !readme.contains("mqtt 192"),
            "{readme}"
        );

        let p = project("esphome = \"per-device\"\nmatter = \"fixed\"\nconsole = true");
        assert!(p.esphome_key.is_none() && codegen::readme(&p).contains("`api-key`"));
        assert!(p.warnings.iter().any(|w| w.contains("by its address")));
        // A given key is kept; the firmware's own encoding of 32 zero bytes.
        let zero = format!("{}=", "A".repeat(43));
        assert_eq!(base64(&[0; 32]), zero);
        assert_eq!(project(&format!("esphome = \"fixed\"\nesphome_key = \"{zero}\"")).esphome_key, Some(zero));
        assert!(!valid_key(&format!("{}B=", "A".repeat(42))), "unused bits set");
    }

    /// Matter over Thread: its board and features, Matter updates, and the combinations refused.
    #[test]
    fn thread() {
        let err = |toml: &str| Project::new(toml::from_str(toml).unwrap(), "/x".into()).err().unwrap().to_string();
        let thread = "transport = \"thread\"\nmatter = \"fixed\"\n";
        assert!(err(&format!("{thread}board = \"esp32c3-devkitm-1\"")).contains("802.15.4"));
        assert!(err("transport = \"thread\"").contains("talk Matter"));
        assert!(err(&format!("{thread}power = \"battery\"\nbattery_pin = 2")).contains("ADC"));
        assert!(err(&format!("{thread}mqtt_host = \"10.0.0.1\"")).contains("no Wi-Fi"));
        assert!(err(&format!("{thread}updates_url = \"http://h/m.json\"")).contains("over Matter"));
        assert!(err(&format!("{thread}[[parts]]\nkind = \"soil_moisture\"\npin = 2")).contains("ADC"));
        assert!(err("board = \"other-esp32h2\"\nflash_size = \"8mb\"").contains("no Wi-Fi"));

        let p = project(include_str!("../../answers/thread.toml"));
        let main = code(&p);
        assert!(
            main.contains("radio: p.IEEE802154") && main.contains("adc1: p.ADC1") && !main.contains("WIFI"),
            "{main}"
        );
        assert!(main.contains("Show::All, true)") && main.contains("over Matter"), "{main}");
        // The status LED keeps its state, and is written before `start` keeps the CPU.
        assert!(main.contains("Light::new(\"status_led\", \"Status LED\").remember()"), "{main}");
        assert!(main.find("led::spawn_rgb(") < main.find("iotkit::start("), "{main}");
        let cargo = codegen::cargo_toml(
            &render::render("Cargo.toml", render::FILES[1].1, &p.package, "esp32c6", "/opt/iotkit").unwrap(),
            &p,
        )
        .unwrap();
        assert!(
            cargo.contains(
                "default-features = false, features = [\"esp32c6\", \"matter-thread\", \"matter-ota\", \"smartled\", \"remember\"]"
            ),
            "{cargo}"
        );
        assert!(
            cargo.contains("esp-radio") && cargo.contains("\"ieee802154\"") && !cargo.contains("matter-wifi"),
            "{cargo}"
        );
        let readme = codegen::readme(&p);
        assert!(readme.contains("ota build") && readme.contains("clang") && !readme.contains("ota push"), "{readme}");
        assert!(!readme.contains("Wi-Fi and Home Assistant") && readme.contains("pairing window"), "{readme}");
        assert!(p.matter_setup.is_none() && codegen::secrets(&p).is_none_or(|s| !s.contains("WIFI")));

        let h2 =
            project("board = \"other-esp32h2\"\nflash_size = \"8mb\"\ntransport = \"thread\"\nmatter = \"per-device\"");
        assert!(code(&h2).contains("radio: p.IEEE802154"));
        let part = project(&format!("{thread}[[parts]]\nkind = \"setting\""));
        assert!(part.warnings.iter().any(|w| w.contains("does not show over Thread")), "{:?}", part.warnings);
    }

    fn cargo_toml(p: &Project) -> String {
        let rendered = render::render("Cargo.toml", render::FILES[1].1, &p.package, p.board.chip, "/opt/iotkit");
        codegen::cargo_toml(&rendered.unwrap(), p).unwrap()
    }

    /// A board file's own wiring: the XIAO ESP32-C6's antenna switch and active-low LED.
    #[test]
    fn xiao() {
        let p = project(include_str!("../../answers/xiao.toml"));
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!((p.answers.status, p.answers.status_pin), (Indicator::Led, Some(15)));
        let main = code(&p);
        assert!(main.contains("let _gpio3 = Output::new(p.GPIO3, Level::Low, OutputConfig::default());"), "{main}");
        assert!(main.contains("let _gpio14 = Output::new(p.GPIO14, Level::Low, OutputConfig::default());"), "{main}");
        assert!(main.find("p.GPIO3") < main.find("iotkit::start("), "before the radio: {main}");
        assert!(main.contains("led::spawn_status(spawner, p.GPIO15);"), "{main}");
        assert!(main.contains("Output::new(pin, Level::High,") && main.contains("Active::Low"), "{main}");
        assert!(codegen::readme(&p).contains("| Status LED | GPIO15 | on the board |"));
        let err = |toml: &str| Project::new(toml::from_str(toml).unwrap(), "/x".into()).err().unwrap().to_string();
        assert!(err("board = \"xiao-esp32c6\"\n[[parts]]\nkind = \"relay\"\npin = 3").contains("antenna switch"));
        // Matter over Thread fits its 4 MB, over Wi-Fi not; with no part, nothing to add.
        let thread = project("board = \"xiao-esp32c6\"\ntransport = \"thread\"\nmatter = \"fixed\"");
        assert!(thread.warnings.iter().any(|w| w.contains("nothing a controller can add")), "{:?}", thread.warnings);
        assert!(err("board = \"xiao-esp32c6\"\nmatter = \"fixed\"").contains("Matter over Wi-Fi needs 8 MB"));
    }

    /// On battery, the mode that suits the link: deep sleep over the ESPHome API (or MQTT), a
    /// Matter ICD over Thread; Matter over Wi-Fi refused (esp-radio).
    #[test]
    fn battery_modes() {
        let p = project(include_str!("../../answers/esphome-battery.toml"));
        assert!(p.sleepy() && !p.icd());
        let main = code(&p);
        assert!(main.contains("start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP)"), "{main}");
        assert!(main.contains("Battery::new(p.ADC1, p.GPIO2, 2.0)") && !main.contains("battery_task"), "{main}");
        assert!(
            cargo_toml(&p)
                .contains("features = [\"esp32c6\", \"ha-update\", \"deep-sleep\", \"esphome-api\", \"improv-ble\"")
        );
        assert!(!cargo_toml(&p).contains("\"mqtt\""));
        assert!(codegen::readme(&p).contains("knows it sleeps"));
        // The wake-up button and the status LED.
        assert!(main.contains("boot_button: Some(p.GPIO3.into())") && !p.warnings.iter().any(|w| w.contains("wake")));
        assert!(
            main.contains("components::status_led(led, Active::High, iotkit::status::Show::All)")
                && main.contains("led::spawn_status(spawner, p.GPIO6)")
                && main.contains("Output::new(pin, Level::Low")
        );
        assert!(codegen::readme(&p).contains("The button on GPIO3"));
        let boot = project(&include_str!("../../answers/esphome-battery.toml").replace("button_pin = 3\n", ""));
        assert!(boot.warnings.iter().any(|w| w.contains("cannot wake the chip")), "{:?}", boot.warnings);

        let p = project(include_str!("../../answers/thread-battery.toml"));
        assert!(p.icd() && !p.sleepy());
        let main = code(&p);
        assert!(main.contains("iotkit::start(spawner, board, &CONFIG).await") && !main.contains("SleepConfig"));
        assert!(main.contains("components::button(&BUTTON"), "ready-made parts, as plugged in: {main}");
        // Its sensors read every `report_every_min`: each report wakes the radio.
        let sensor = project(&format!(
            "{}\n[i2c]\nsda = 0\nscl = 1\n[[parts]]\nkind = \"bh1750\"",
            include_str!("../../answers/thread-battery.toml")
        ));
        let main = code(&sensor);
        assert!(main.contains("Ticker::every(Duration::from_secs(300))") && main.contains("every 5 min"), "{main}");
        assert!(codegen::readme(&sensor).contains("reading its sensors every 5 min"));
        let cargo = cargo_toml(&p);
        assert!(
            cargo.contains("[\"esp32c6\", \"matter-thread\", \"matter-ota\", \"matter-icd\", \"smartled\"]"),
            "{cargo}"
        );
        assert!(
            main.contains("components::status_rgb(led, 1, 32, iotkit::status::Show::All)")
                && !main.contains("Light::new(\"status_led\"")
        );
        assert!(codegen::readme(&p).contains("sleepy Thread device"));

        // Matter over Wi-Fi on battery waits for esp-radio.
        let err = Project::new(toml::from_str("power = \"battery\"\nmatter = \"fixed\"").unwrap(), "/x".into());
        assert!(err.err().unwrap().to_string().contains("not supported yet"));
    }

    /// No parts: still the answers' maker, name and LED id (as the README), not the template's.
    #[test]
    fn no_parts_main() {
        let p = project("name = \"C6 devkit\"\nmaker = \"Tanguy\"");
        let main = code(&p);
        assert!(main.contains(".manufacturer(\"Tanguy\")") && main.contains(".product_name(\"C6 devkit\")"), "{main}");
        assert!(main.contains("Light::new(\"status_led\"") && codegen::readme(&p).contains("`status_led`"));
        assert!(!main.contains("TODO(") && main.contains("`cargo iotkit add` adds one"), "{main}");
    }

    /// Every component's ready-made code renders on every chip, its pins accepted there.
    #[test]
    fn components_render() {
        for chip in ["esp32c6", "esp32c3", "esp32c5", "esp32s3", "esp32", "esp32h2"] {
            let board = boards().iter().find(|b| b.chip == chip).unwrap();
            let thread = if board.wifi() { "" } else { "transport = \"thread\"\nmatter = \"per-device\"\n" };
            for k in kinds().iter().filter(|k| k.code.is_some() && !(!thread.is_empty() && k.analog())) {
                let mut taken = vec![board.boot_pin];
                taken.extend(board.rgb_led);
                let pins: Vec<String> = k
                    .pins
                    .iter()
                    .map(|spec| {
                        let n = (0..=48).find(|&n| check_pin(board, n, spec.pin_use, &taken) == PinCheck::Ok).unwrap();
                        taken.push(n);
                        format!("{} = {n}", spec.role)
                    })
                    .collect();
                let answers = format!(
                    "board = \"{}\"\nflash_size = \"8mb\"\n{thread}[[parts]]\nkind = \"{}\"\npins = {{ {} }}",
                    board.id,
                    k.id,
                    pins.join(", ")
                );
                let p = project(&answers);
                assert!(codegen::ready(&p)[0], "{chip} {}", k.id);
                let code = codegen::part(&p, 0).unwrap();
                assert!(
                    !code.file.contains("{{") && !code.file.contains("{%") && !code.file.contains("TODO:"),
                    "{}",
                    code.file
                );
            }
        }
        let err = |toml: &str| Project::new(toml::from_str(toml).unwrap(), "/x".into()).err().unwrap().to_string();
        assert!(err("[[parts]]\nkind = \"relay\"\npins = { sda = 4 }").contains("no pin `sda`"));
        assert!(err("[[parts]]\nkind = \"setting\"\npin = 4").contains("takes no pin"));
        // A keyword is not a module name.
        let p = project("[[parts]]\nkind = \"mode\"\nname = \"Type\"");
        assert_eq!((p.parts[0].id.as_str(), p.parts[0].module.as_str()), ("type", "type_part"));
    }

    /// `iotkit.toml` reads back as the same project, without the secrets.
    #[test]
    fn record_roundtrip() {
        let p = project(&format!(
            "wifi_ssid = \"Home\"\nwifi_password = \"hunter2\"\n{}",
            include_str!("../../answers/c6.toml")
        ));
        let text = record(&p.answers).unwrap();
        assert!(!text.contains("Home") && !text.contains("hunter2") && !text.contains("passcode"), "{text}");
        let back = Project::new(toml::from_str(&text).unwrap(), "/x".into()).unwrap();
        assert_eq!(back.parts.len(), p.parts.len());
        assert_eq!(code(&back), code(&p));
    }

    /// AGENTS.md only when asked for, with the lines that apply to the device.
    #[test]
    fn agents() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gen/xtask-test");
        for (name, answers) in
            [("c6", include_str!("../../answers/c6.toml")), ("c3", include_str!("../../answers/c3.toml"))]
        {
            let p = project(answers);
            let dir = base.join(name);
            let _ = fs::remove_dir_all(&dir);
            write(&p, &dir).unwrap();
            assert_eq!(dir.join("AGENTS.md").exists(), p.answers.ai_agents, "{name}");
            let md = codegen::agents_md(&p);
            assert!(md.contains(&format!("{}. A smart device on an", p.answers.name)), "{md}");
            assert_eq!(md.contains("Matter"), name == "c6");
            assert_eq!(md.contains("start_sleepy"), name == "c3");
            assert!(!md.contains("export-esp"));
            assert!(md.lines().count() <= 60, "{md}");
        }
        assert!(project("ai_agents = true").answers.ai_agents && !project("").answers.ai_agents);
        assert!(codegen::agents_md(&project("board = \"esp32s3-devkitc-1\"")).contains("export-esp"));
    }
}
