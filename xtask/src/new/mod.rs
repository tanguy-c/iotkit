//! `cargo iotkit new`: a new device project from a few questions (or an answers file).

mod boards;
mod codegen;
pub mod matter;
mod parts;
mod render;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use boards::{BOARDS, Board, PinCheck, check_pin};
use inquire::{Confirm, InquireError, Password, PasswordDisplayMode, Select, Text, validator::Validation};
use parts::{Group, KINDS, Kind, slug};
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
    pub power: Power,
    /// Battery: how often it wakes up.
    pub report_every_min: u32,
    /// Battery: the ADC1 pin behind a divider, to report the charge.
    pub battery_pin: Option<u8>,
    pub battery_curve: Curve,
    /// The board's RGB LED as a light (plugged-in devkits).
    pub status_led: bool,
    pub wifi_ssid: Option<String>,
    pub wifi_password: Option<String>,
    pub mqtt_host: Option<String>,
    pub mqtt_user: Option<String>,
    pub mqtt_password: Option<String>,
    pub matter: MatterMode,
    /// A fixed code to reuse (else drawn); with `matter = "fixed"`.
    pub matter_passcode: Option<u32>,
    pub matter_discriminator: Option<u16>,
    /// The manifest Home Assistant's updates come from.
    pub updates_url: Option<String>,
    pub parts: Vec<PartAnswer>,
}

impl Default for Answers {
    fn default() -> Self {
        Self {
            name: "My device".into(),
            package: String::new(),
            maker: "Me".into(),
            board: "esp32c6-devkitc-1".into(),
            flash_size: None,
            power: Power::Plugged,
            report_every_min: 5,
            battery_pin: None,
            battery_curve: Curve::Lipo,
            status_led: true,
            wifi_ssid: None,
            wifi_password: None,
            mqtt_host: None,
            mqtt_user: None,
            mqtt_password: None,
            matter: MatterMode::Off,
            matter_passcode: None,
            matter_discriminator: None,
            updates_url: None,
            parts: Vec::new(),
        }
    }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartAnswer {
    pub kind: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub pin: Option<u8>,
}

/// The answers, checked and completed.
pub struct Project {
    pub answers: Answers,
    pub board: &'static Board,
    pub package: String,
    pub flash: String,
    pub parts: Vec<Part>,
    pub matter_setup: Option<matter::Setup>,
    pub iotkit_path: String,
    pub warnings: Vec<String>,
}

pub struct Part {
    pub kind: &'static Kind,
    pub name: String,
    /// Entity id (`living_room_temperature`).
    pub id: String,
    /// Its `static` (`LIVING_ROOM_TEMPERATURE`).
    pub konst: String,
    pub pin: Option<u8>,
}

impl Project {
    pub fn battery(&self) -> bool {
        self.answers.power == Power::Battery
    }

    pub fn status_led(&self) -> bool {
        self.answers.status_led && !self.battery() && self.board.rgb_led.is_some()
    }

    /// Checks the answers: board, pins, what the chip can do.
    pub fn new(answers: Answers, iotkit_path: String) -> Result<Self> {
        let Some(board) = boards::board(&answers.board) else {
            let ids: Vec<_> = BOARDS.iter().map(|b| b.id).collect();
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
        if answers.matter != MatterMode::Off {
            ensure!(
                board.matter(),
                "Matter needs an ESP32-C6 (the {} has not the RAM or the drivers yet)",
                board.chip_label
            );
            ensure!(!battery, "Matter devices stay awake: choose `power = \"plugged\"` or no Matter");
            ensure!(flash == "8mb", "Matter needs 8 MB of flash");
        }
        ensure!((1..=24 * 60).contains(&answers.report_every_min), "report_every_min: 1 to 1440");

        let mut warnings = Vec::new();
        let mut taken: Vec<u8> = Vec::new();
        let led = answers.status_led && !battery;
        if let (true, Some(pin)) = (led, board.rgb_led) {
            taken.push(pin);
        }
        let take = |pin: u8, analog: bool, what: &str, taken: &mut Vec<u8>, warnings: &mut Vec<String>| -> Result<()> {
            match check_pin(board, pin, analog, taken) {
                PinCheck::Ok => {}
                PinCheck::Warn(w) => warnings.push(format!("{what}: {w}")),
                PinCheck::Refused(why) => bail!("{what}: {why}"),
            }
            taken.push(pin);
            Ok(())
        };
        if battery && let Some(pin) = answers.battery_pin {
            take(pin, true, "Battery level", &mut taken, &mut warnings)?;
        }
        let mut ids: Vec<String> = vec!["firmware".into(), "status_led".into(), "battery".into()];
        let mut parts = Vec::new();
        for a in &answers.parts {
            let Some(kind) = parts::kind(&a.kind) else {
                let kinds: Vec<_> = KINDS.iter().map(|k| k.id).collect();
                bail!("unknown part `{}` (one of {kinds:?})", a.kind);
            };
            let name = a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| kind.name.to_string());
            let base = slug(&name, '_');
            let base = if base.is_empty() { kind.id.to_string() } else { base };
            let mut id = base.clone();
            let mut n = 2;
            while ids.contains(&id) {
                id = format!("{base}_{n}");
                n += 1;
            }
            ids.push(id.clone());
            if let Some(pin) = a.pin {
                take(pin, kind.analog, &name, &mut taken, &mut warnings)?;
            }
            if battery && !kind.is_reading() {
                warnings.push(format!("{name}: on a battery device, commands apply at the next wake-up"));
            }
            parts.push(Part { kind, konst: id.to_uppercase(), name, id, pin: a.pin });
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
        Ok(Self { answers, board, package, flash, parts, matter_setup, iotkit_path, warnings })
    }
}

/// The checkout this CLI was built from (it has the crates the project uses).
fn default_iotkit_path() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent().unwrap_or(here).to_path_buf()
}

pub struct NewArgs {
    pub dir: Option<PathBuf>,
    pub answers: Option<PathBuf>,
    pub defaults: bool,
    pub iotkit_path: Option<PathBuf>,
}

pub fn run(args: NewArgs) -> Result<()> {
    let iotkit = args.iotkit_path.unwrap_or_else(default_iotkit_path);
    ensure!(
        iotkit.join("crates/iotkit-app").is_dir(),
        "no iotkit checkout at {} (`--iotkit-path DIR`: the folder with crates/)",
        iotkit.display()
    );
    let iotkit = iotkit.canonicalize()?.to_string_lossy().into_owned();
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
            "src/main.rs" if !p.parts.is_empty() || p.battery() || !p.status_led() => text = codegen::main_rs(p),
            ".cargo/config.toml" if p.flash != default_flash(p.board.chip) => {
                text = text.replace(
                    &format!(
                        "--flash-size {} --partition-table {}",
                        default_flash(p.board.chip),
                        table(default_flash(p.board.chip))
                    ),
                    &format!("--flash-size {} --partition-table {}", p.flash, table(&p.flash)),
                );
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
    // The generated code, in the project's style (best effort: rustfmt may be missing).
    let _ = std::process::Command::new("rustfmt").args(["--edition", "2024", "src/main.rs"]).current_dir(dir).status();
    put(&dir.join("README.md"), &codegen::readme(p))?;
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
    if chip == "esp32c3" { "4mb" } else { "8mb" }
}

fn table(flash: &str) -> &'static str {
    if flash == "4mb" { "partitions-4mb.csv" } else { "partitions.csv" }
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
    let flash = if p.board.xtensa() { "  (ESP32-S3: run `. ~/export-esp.sh` first)" } else { "" };
    println!(
        "\nNext:\n  cd {}\n  cargo iotkit flash --release{flash}\n\nWiring and setup: README.md. Each part's code is a TODO in src/main.rs.",
        dir.display()
    );
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
    let labels: Vec<&str> = BOARDS.iter().map(|b| b.label).collect();
    let start = BOARDS.iter().position(|b| b.id == a.board).unwrap_or(0);
    let label = Select::new("Which board?", labels.clone())
        .with_starting_cursor(start)
        .with_help_message("The chip decides what is possible: Matter needs the C6. Only the C6 is tested on hardware.")
        .prompt()?;
    let board = &BOARDS[labels.iter().position(|l| *l == label).unwrap_or(0)];
    a.board = board.id.into();
    if board.flash.is_none() {
        let f = Select::new("How much flash memory does it have?", vec!["4 MB", "8 MB or more"])
            .with_help_message("On the module's label or in the board's datasheet (N4 = 4 MB, N8 = 8 MB)")
            .prompt()?;
        a.flash_size = Some(if f.starts_with('4') { "4mb" } else { "8mb" }.into());
    }

    // 3. Power.
    let battery = Select::new(
        "How is it powered?",
        vec!["Plugged in (USB or mains): always on", "Battery: wakes up, reports, sleeps"],
    )
    .with_help_message("A battery device sleeps between reports to last months, and answers commands only when awake")
    .prompt()?
    .starts_with("Battery");
    a.power = if battery { Power::Battery } else { Power::Plugged };
    if battery {
        let every = Select::new(
            "How often should it report?",
            vec!["Every minute", "Every 5 minutes", "Every 15 minutes", "Every hour"],
        )
        .with_starting_cursor(1)
        .with_help_message("More often drains the battery faster")
        .prompt()?;
        a.report_every_min = match every {
            "Every minute" => 1,
            "Every 15 minutes" => 15,
            "Every hour" => 60,
            _ => 5,
        };
        if Confirm::new("Report the battery level?")
            .with_default(true)
            .with_help_message("Needs two resistors from the battery to an analog pin")
            .prompt()?
        {
            a.battery_pin = ask_pin(board, "Analog pin for the battery level?", true, &[], false)?;
            let curve =
                Select::new("Which battery?", vec!["LiPo / Li-ion cell (3.0–4.2 V)", "2 × AA alkaline (2.0–3.2 V)"])
                    .prompt()?;
            a.battery_curve = if curve.starts_with('2') { Curve::TwoAa } else { Curve::Lipo };
        }
    }

    // 4. Connectivity.
    println!("\nHow it talks to your home:");
    if Confirm::new("Enter your Wi-Fi now?")
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
    let broker = Text::new("Home Assistant's MQTT broker (IP or host name)?")
        .with_help_message(
            "The Mosquitto add-on: Home Assistant's address. Leave empty to set it later on the console (`mqtt HOST`).",
        )
        .prompt()?;
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
    if !board.matter() {
        println!("  (Matter: not on the {} yet, Home Assistant over MQTT only)", board.chip_label);
    } else if battery {
        println!("  (Matter: battery devices use MQTT only, as Matter devices stay awake)");
    } else if Confirm::new("Also work with Matter (Apple Home, Google Home, Alexa, Home Assistant)?")
        .with_default(false)
        .with_help_message("Next to MQTT. Paired with a code; uses about 2 MB of flash and more RAM.")
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
    let package = if a.package.is_empty() { slug(&a.name, '-') } else { a.package.clone() };
    let default_url =
        a.mqtt_host.as_deref().map(|h| format!("http://{h}:8080/{package}/manifest.json")).unwrap_or_default();
    let url = Text::new("Where will you publish updates (manifest URL)?")
        .with_default(&default_url)
        .with_help_message(
            "Home Assistant then offers new versions; `cargo iotkit ota publish` fills it. Leave empty to skip.",
        )
        .prompt()?;
    a.updates_url = (!url.trim().is_empty()).then(|| url.trim().to_string());

    // 5. Parts.
    a.status_led = board.rgb_led.is_some()
        && !battery
        && Confirm::new("Keep the board's RGB LED as a status light?")
            .with_default(true)
            .with_help_message("A light in Home Assistant; a short press of BOOT toggles it")
            .prompt()?;
    println!("\nWhat does it do? Add its parts one by one (sensors, relays, buttons...).");
    let mut taken: Vec<u8> = a.battery_pin.into_iter().collect();
    if a.status_led
        && let Some(led) = board.rgb_led
    {
        taken.push(led);
    }
    a.parts.clear();
    loop {
        let mut options: Vec<String> = vec!["Done".into()];
        for g in [Group::Measures, Group::Detects, Group::Controls, Group::Inputs] {
            options.extend(KINDS.iter().filter(|k| k.group == g).map(|k| format!("{}: {}", g.label(), k.label)));
        }
        let prompt = if a.parts.is_empty() { "Add a part?" } else { "Add another part?" };
        let choice = Select::new(prompt, options.clone())
            .with_page_size(12)
            .with_help_message("Type to filter. Each part becomes an entity, with a TODO in the code for its driver.")
            .prompt()?;
        if choice == "Done" {
            break;
        }
        let kind = &KINDS[options.iter().position(|o| *o == choice).expect("chosen") - 1];
        let name = Text::new("Its name?")
            .with_default(kind.name)
            .with_help_message("As Home Assistant shows it, e.g. \"Kitchen temperature\"")
            .prompt()?;
        let pin = if kind.wiring.starts_with("(none") {
            None
        } else {
            ask_pin(board, &format!("Which GPIO for {name}? ({})", kind.wiring), kind.analog, &taken, true)?
        };
        taken.extend(pin);
        a.parts.push(PartAnswer { kind: kind.id.into(), name: Some(name), pin });
    }

    let project = Project::new(a, iotkit_path)?;
    summary(&project);
    if !Confirm::new("Create it?").with_default(true).prompt()? {
        return Err(InquireError::OperationCanceled.into());
    }
    Ok(project)
}

fn non_empty(s: &str) -> Result<Validation, inquire::CustomUserError> {
    Ok(if s.trim().is_empty() { Validation::Invalid("needed".into()) } else { Validation::Valid })
}

/// A GPIO number, checked for the chip (refused pins are asked again, warnings shown).
fn ask_pin(board: &'static Board, question: &str, analog: bool, taken: &[u8], optional: bool) -> Result<Option<u8>> {
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
            Ok(match check_pin(board, pin, analog, &taken) {
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
    if let PinCheck::Warn(w) = check_pin(board, pin, analog, &[]) {
        println!("  note: {w}");
    }
    Ok(Some(pin))
}

fn summary(p: &Project) {
    let a = &p.answers;
    println!("\n{} ({}), by {}", a.name, p.package, a.maker);
    println!("  board    {} ({}, {} flash)", p.board.model, p.board.chip_label, p.flash.to_uppercase());
    if p.battery() {
        println!("  power    battery, reports every {} min", a.report_every_min);
    } else {
        println!("  power    plugged in");
    }
    let mut links = vec!["Home Assistant (MQTT)"];
    match a.matter {
        MatterMode::Fixed => links.push("Matter (fixed code)"),
        MatterMode::PerDevice => links.push("Matter (code per device)"),
        MatterMode::Off => {}
    }
    println!("  talks to {}", links.join(" + "));
    println!("  Wi-Fi    {}", if a.wifi_ssid.is_some() { "built in" } else { "set up from a phone at first boot" });
    for part in &p.parts {
        let pin = part.pin.map_or("pin to choose".into(), |n| format!("GPIO{n}"));
        println!("  part     {} ({}), {pin}", part.name, part.kind.label);
    }
    for w in &p.warnings {
        println!("  note     {w}");
    }
    println!();
}

impl Board {
    fn xtensa(&self) -> bool {
        self.chip == "esp32s3"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(toml: &str) -> Project {
        Project::new(toml::from_str(toml).unwrap(), "/opt/iotkit".into()).unwrap()
    }

    #[test]
    fn checks() {
        let err = |toml: &str| Project::new(toml::from_str(toml).unwrap(), "/x".into()).err().unwrap().to_string();
        assert!(err("board = \"esp32c3-devkitm-1\"\nmatter = \"fixed\"").contains("ESP32-C6"));
        assert!(err("power = \"battery\"\nmatter = \"fixed\"").contains("awake"));
        assert!(err("[[parts]]\nkind = \"relay\"\npin = 12").contains("USB"));
        assert!(err("status_led = true\n[[parts]]\nkind = \"relay\"\npin = 8").contains("already used"));
        assert!(err("[[parts]]\nkind = \"kettle\"").contains("unknown part"));
        let p =
            project("name = \"Garden sensor\"\n[[parts]]\nkind = \"temperature\"\n[[parts]]\nkind = \"temperature\"");
        assert_eq!(p.package, "garden-sensor");
        assert_eq!(p.parts[1].id, "temperature_2");
        assert_eq!(p.parts[1].konst, "TEMPERATURE_2");
    }

    #[test]
    fn plugged_main() {
        let p = project(include_str!("../../answers/c6.toml"));
        let main = codegen::main_rs(&p);
        assert!(main.contains("static PUMP: Switch = Switch::new(\"pump\", \"Pump\");"), "{main}");
        assert!(main.contains("spawner.spawn(pump_task(p.GPIO4.into()).unwrap());"));
        assert!(main.contains("async fn pump_task(_pin: AnyPin<'static>)"));
        assert!(main.contains("iotkit_app::start(spawner, board, &CONFIG).await;"));
        assert!(main.contains("led_task"));
        let secrets = codegen::secrets(&p).unwrap();
        assert!(secrets.contains(&format!("MATTER_PASSCODE = \"{}\"", p.matter_setup.unwrap().passcode)));
        let cargo = codegen::cargo_toml(
            &render::render("Cargo.toml", render::FILES[1].1, &p.package, "esp32c6", "/opt/iotkit").unwrap(),
            &p,
        )
        .unwrap();
        assert!(cargo.contains("default     = [\"matter-wifi\"]"), "{cargo}");
        assert!(cargo.contains("features = [\"esp32c6\", \"ha-update\"]"), "{cargo}");
        assert!(codegen::readme(&p).contains(&p.matter_setup.unwrap().manual_code_pretty()));
    }

    #[test]
    fn battery_main() {
        let p = project(include_str!("../../answers/c3.toml"));
        let main = codegen::main_rs(&p);
        assert!(main.contains("start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP)"), "{main}");
        assert!(main.contains("Duration::from_secs(900)"));
        assert!(main.contains("Battery::new(p.ADC1, p.GPIO3, 2.0).curve(BatteryCurve::ALKALINE_2AA)"));
        assert!(!main.contains("led_task") && !main.contains("Ticker"));
        let cargo = codegen::cargo_toml(
            &render::render("Cargo.toml", render::FILES[1].1, &p.package, "esp32c3", "/opt/iotkit").unwrap(),
            &p,
        )
        .unwrap();
        assert!(cargo.contains("\"deep-sleep\"") && !cargo.contains("matter-wifi"), "{cargo}");
    }
}
