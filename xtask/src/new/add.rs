//! `cargo iotkit add`: a part in a project made by `cargo iotkit new`, from `iotkit.toml`. It
//! writes the part's module and adds it to `src/main.rs`'s lists (at their marks), to
//! `src/parts/mod.rs`, the README's wiring table and `iotkit.toml`; it never changes code
//! written before.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use super::{Answers, Group, PartAnswer, Project, ask_part, codegen, is_cancel, kinds, parts, put, record, rustfmt};

/// The answers `cargo iotkit new` was given, in the project.
pub const RECORD: &str = "iotkit.toml";

pub struct AddArgs {
    /// A component's id; `None`: asked.
    pub component: Option<String>,
    pub name: Option<String>,
    /// `4` (its first pin) or `role=4`; `sda=6` and `scl=7` for the I²C bus.
    pub pins: Vec<String>,
    /// An I²C chip's address, if not its default.
    pub address: Option<u8>,
    pub dir: PathBuf,
}

pub fn run(args: AddArgs) -> Result<()> {
    let dir = &args.dir;
    let path = dir.join(RECORD);
    let text = fs::read_to_string(&path).with_context(|| {
        format!("{}: not found; `cargo iotkit add` works in projects made by `cargo iotkit new`", path.display())
    })?;
    let mut answers: Answers = toml::from_str(&text).with_context(|| path.display().to_string())?;
    restore_secrets(&mut answers, dir);
    let before = Project::new(answers.clone(), String::new())?;

    let part = match args.component {
        Some(id) => {
            let kind = parts::kind(&id)
                .with_context(|| format!("unknown component `{id}` (`cargo iotkit components` lists them)"))?;
            let mut part =
                PartAnswer { kind: id, name: args.name, pin: None, pins: BTreeMap::new(), address: args.address };
            let (mut sda, mut scl) = (None, None);
            for arg in &args.pins {
                let (role, pin) = match arg.split_once('=') {
                    Some((role, pin)) => (Some(role.trim()), pin),
                    None => (None, arg.as_str()),
                };
                let digits = pin.trim().trim_start_matches(|c: char| c.is_ascii_alphabetic());
                let n: u8 = digits.parse().with_context(|| format!("--pin {arg}: a GPIO number, or ROLE=GPIO"))?;
                match role {
                    Some("sda") if kind.i2c.is_some() => sda = Some(n),
                    Some("scl") if kind.i2c.is_some() => scl = Some(n),
                    Some(r) if kind.pins.len() > 1 || kind.pins.first().is_some_and(|s| s.role != r) => {
                        part.pins.insert(r.into(), n);
                    }
                    _ if part.pin.is_some() => bail!("--pin {arg}: name the pin (`ROLE=GPIO`), the first is given"),
                    _ => part.pin = Some(n),
                }
            }
            match (sda, scl, answers.i2c) {
                (None, None, _) => {}
                (Some(sda), Some(scl), None) => answers.i2c = Some(super::I2cPins { sda, scl }),
                (_, _, Some(bus)) => bail!(
                    "the I²C bus is already on GPIO{} (SDA) and GPIO{} (SCL): leave out `--pin sda=` and `scl=`",
                    bus.sda,
                    bus.scl
                ),
                _ => bail!("the I²C bus: both `--pin sda=GPIO` and `--pin scl=GPIO`"),
            }
            part
        }
        None => {
            let mut taken = taken(&before);
            match ask_part(before.board, before.thread(), &mut taken, true, &mut answers.i2c) {
                Ok(Some(part)) => part,
                Ok(None) => return Ok(()),
                Err(e) if is_cancel(&e) => {
                    println!("Cancelled: nothing written.");
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
    };
    answers.parts.push(part);
    let p = Project::new(answers, String::new())?;
    let index = p.parts.len() - 1;
    let new = &p.parts[index];
    let code = codegen::part(&p, index)?;

    // Everything is checked before anything is written.
    let module = format!("src/parts/{}.rs", new.module);
    ensure!(!dir.join(&module).exists(), "{module} already exists: give the part another name (`--name`)");
    let main_path = dir.join("src/main.rs");
    let mut main = fs::read_to_string(&main_path).with_context(|| main_path.display().to_string())?;
    let manual = |mark: &str, line: &str| {
        format!(
            "src/main.rs has no `{mark}` (made before `cargo iotkit add`, or the comment was removed): put it back above\n  \
             the list it marks, or add `{line}` by hand"
        )
    };
    for entity in &code.entities {
        main = insert(&main, codegen::ENTITIES_MARK, "&parts::", &format!("{entity},"))
            .with_context(|| manual(codegen::ENTITIES_MARK, entity))?;
    }
    let (mark, line) = match &code.call {
        codegen::Call::Spawn(line) => (codegen::SPAWNS_MARK, line),
        codegen::Call::Read(line) => (codegen::READS_MARK, line),
    };
    main = insert(&main, mark, "parts::", line).with_context(|| manual(mark, line))?;
    // The first I²C part: the bus, created above the parts that take it.
    let new_bus = p.i2c().is_some() && before.i2c().is_none();
    if new_bus {
        let bus = codegen::bus_line(&p).expect("a bus");
        main = above(&main, mark, &format!("{bus}\n")).with_context(|| manual(mark, &bus))?;
        // In rustfmt's order: `i2c` sorts before `led` and `parts`.
        let first = main.lines().find(|l| l.starts_with("mod ")).unwrap_or("mod parts;").to_string();
        main = above(&main, &first, "mod i2c;").context("src/main.rs has no `mod parts;`")?;
    }
    let mods_path = dir.join("src/parts/mod.rs");
    let mut mods = fs::read_to_string(&mods_path).unwrap_or_else(|_| codegen::PARTS_MOD.into());
    if !mods.ends_with('\n') {
        mods.push('\n');
    }
    if !mods.contains("pub mod ") {
        mods.push('\n');
    }
    mods += &format!("pub mod {};\n", new.module);
    let cargo_path = dir.join("Cargo.toml");
    let mut cargo = fs::read_to_string(&cargo_path).with_context(|| cargo_path.display().to_string())?;
    for f in &code.features {
        cargo = feature(&cargo, f).with_context(|| format!("Cargo.toml: add `{f}` to iotkit's features by hand"))?;
    }
    if new_bus {
        for line in codegen::bus_dependencies(&p) {
            cargo = dependency(&cargo, line).with_context(|| format!("Cargo.toml: add `{line}` by hand"))?;
        }
    }
    let readme_path = dir.join("README.md");
    let mut readme = fs::read_to_string(&readme_path).ok().and_then(|r| readme_row(&r, &codegen::readme_row(&p, new)));
    if let (true, Some(row)) = (new_bus, codegen::readme_bus_row(&p)) {
        readme = readme.and_then(|r| readme_row(&r, &row));
    }

    fs::write(dir.join("src/main.rs"), main).context("src/main.rs")?;
    put(&dir.join(&module), &code.file)?;
    rustfmt(dir, &module);
    fs::write(&mods_path, mods).context("src/parts/mod.rs")?;
    if new_bus && let Some(i2c) = codegen::i2c_rs(&p) {
        put(&dir.join("src/i2c.rs"), &i2c)?;
    }
    fs::write(&cargo_path, cargo).context("Cargo.toml")?;
    if let Some(readme) = readme {
        fs::write(&readme_path, readme).context("README.md")?;
    }
    put(&dir.join(RECORD), &record(&p.answers)?)?;

    let pins = if new.kind.pins.is_empty() && new.address.is_none() { "no pin".into() } else { new.pin_cell() };
    println!("Added {} ({}), {pins}: {module}", new.name, new.kind.label);
    if !codegen::ready(&p)[index] {
        let why = match &new.kind.code {
            None if new.kind.i2c.is_some() => " (ready-made on an I²C bus: give it `--pin sda=GPIO --pin scl=GPIO`)",
            None => "",
            Some(_) if new.pins.iter().any(Option::is_none) => " (ready-made once its pins are chosen)",
            Some(_) if p.sleepy() => " (on battery: no ready-made code for it yet)",
            Some(code) => &format!(" (its ready-made code takes {}, which an earlier part has)", code.takes.join(", ")),
        };
        println!("  Its code left to write is `TODO({})` there{why}.", new.id);
    }
    for w in &p.warnings {
        if !before.warnings.contains(w) {
            println!("  note: {w}");
        }
    }
    Ok(())
}

/// `cargo iotkit components`: the parts, by group.
pub fn list() {
    for g in [Group::Measures, Group::Detects, Group::Controls, Group::Inputs] {
        println!("{}", g.label());
        for k in kinds().iter().filter(|k| k.group == g) {
            let pins = match k.pins.as_slice() {
                [] if k.i2c.is_some() => "I²C bus".to_string(),
                [] => "no pin".to_string(),
                [one] => format!("{:?} pin", one.pin_use).to_lowercase(),
                many => many
                    .iter()
                    .map(|s| format!("{} ({:?})", s.role, s.pin_use).to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let ready = if k.ready_made() { "ready-made" } else { "TODO" };
            println!("  {:<14} {:<46} {:<14} {ready}", k.id, k.label, pins);
        }
    }
    println!(
        "\nAdd one: cargo iotkit add <id> [--name NAME] [--pin GPIO]. The files: components/ in the iotkit repository."
    );
}

/// The pins the project uses: its board's, its LEDs', its buttons' and its parts'.
fn taken(p: &Project) -> Vec<u8> {
    let a = &p.answers;
    let mut taken: Vec<u8> = p.board.init.iter().map(|i| i.pin).collect();
    if p.status_led() || a.status == super::Indicator::Rgb {
        taken.extend(p.board.rgb_led);
    }
    taken.extend(a.status_pin);
    taken.extend(a.button_pin);
    taken.extend(p.battery().then_some(a.battery_pin).flatten());
    taken.extend(p.parts.iter().flat_map(|part| part.pins.iter().flatten()));
    taken.extend(a.i2c.iter().flat_map(|bus| [bus.sda, bus.scl]));
    taken
}

/// What `iotkit.toml` leaves out, from `.cargo/secrets.toml` (git-ignored: it may be missing).
fn restore_secrets(a: &mut Answers, dir: &Path) {
    let Ok(text) = fs::read_to_string(dir.join(".cargo/secrets.toml")) else { return };
    let Ok(secrets) = text.parse::<toml::Table>() else { return };
    let Some(env) = secrets.get("env").and_then(|e| e.as_table()) else { return };
    let get = |key: &str| env.get(key).and_then(|v| v.as_str()).map(String::from);
    a.wifi_ssid = a.wifi_ssid.take().or(get("WIFI_SSID"));
    a.wifi_password = a.wifi_password.take().or(get("WIFI_PASS"));
    a.mqtt_user = a.mqtt_user.take().or(get("MQTT_USER"));
    a.mqtt_password = a.mqtt_password.take().or(get("MQTT_PASS"));
    a.esphome_key = a.esphome_key.take().or(get("ESPHOME_API_KEY"));
    a.matter_passcode = a.matter_passcode.or(get("MATTER_PASSCODE").and_then(|v| v.parse().ok()));
    a.matter_discriminator = a.matter_discriminator.or(get("MATTER_DISCRIMINATOR").and_then(|v| v.parse().ok()));
}

/// `text` with `line` after the lines starting with `prefix` that follow `mark` (at its indent).
fn insert(text: &str, mark: &str, prefix: &str, line: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().position(|l| l.trim() == mark)?;
    let indent = &lines[at][..lines[at].len() - lines[at].trim_start().len()];
    let mut end = at + 1;
    while end < lines.len() && lines[end].trim_start().starts_with(prefix) {
        end += 1;
    }
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(end, format!("{indent}{line}"));
    Some(out.join("\n") + "\n")
}

/// `text` with `line` above `mark`'s line (at its indent), unless already there.
fn above(text: &str, mark: &str, line: &str) -> Option<String> {
    if text.lines().any(|l| l.trim() == line) {
        return Some(text.into());
    }
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let at = lines.iter().position(|l| l.trim() == mark)?;
    let indent = lines[at][..lines[at].len() - lines[at].trim_start().len()].to_string();
    lines.insert(at, format!("{indent}{line}"));
    Some(lines.join("\n") + "\n")
}

/// `Cargo.toml` with the dependency `line` after `embassy-time`'s, unless it has that crate.
fn dependency(cargo: &str, line: &str) -> Option<String> {
    let name = line.split_whitespace().next()?;
    if cargo.lines().any(|l| l.split_whitespace().next() == Some(name)) {
        return Some(cargo.into());
    }
    let mut lines: Vec<String> = cargo.lines().map(String::from).collect();
    let at = lines.iter().position(|l| l.starts_with("embassy-time"))?;
    lines.insert(at + 1, line.into());
    Some(lines.join("\n") + "\n")
}

/// `Cargo.toml` with `feature` in iotkit's features.
fn feature(cargo: &str, feature: &str) -> Option<String> {
    let mut out = String::new();
    let mut done = false;
    for l in cargo.lines() {
        if !done && (l.starts_with("iotkit ") || l.starts_with("iotkit=")) {
            if l.contains(&format!("{feature:?}")) {
                done = true;
            } else if let Some(at) = l.find("features = [") {
                let at = at + "features = [".len();
                out.push_str(&format!("{}{feature:?}, {}\n", &l[..at], &l[at..]));
                done = true;
                continue;
            }
        }
        out.push_str(l);
        out.push('\n');
    }
    done.then_some(out)
}

/// The README with `row` at the end of its wiring table.
fn readme_row(readme: &str, row: &str) -> Option<String> {
    let lines: Vec<&str> = readme.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("## Parts and wiring"))?;
    let head = start + lines[start..].iter().position(|l| l.starts_with('|'))?;
    let end = head + lines[head..].iter().position(|l| !l.starts_with('|')).unwrap_or(lines.len() - head);
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(end, row.into());
    Some(out.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a written project: the module, its lines at the marks, and the record; a refused pin
    /// writes nothing.
    #[test]
    fn adds() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gen/xtask-test/add");
        let _ = fs::remove_dir_all(&dir);
        let p = Project::new(toml::from_str(include_str!("../../answers/c3.toml")).unwrap(), "/x".into()).unwrap();
        super::super::write(&p, &dir).unwrap();
        let add = |component: &str, pins: &[&str]| {
            run(AddArgs {
                component: Some(component.into()),
                name: Some("Garden pump".into()),
                pins: pins.iter().map(|s| s.to_string()).collect(),
                address: None,
                dir: dir.clone(),
            })
        };
        add("relay", &["0"]).unwrap();
        let main = fs::read_to_string(dir.join("src/main.rs")).unwrap();
        assert!(main.contains("        &parts::garden_pump::GARDEN_PUMP,\n"), "{main}");
        assert!(main.contains("    parts::garden_pump::spawn(spawner, p.GPIO0);\n"), "{main}");
        assert!(fs::read_to_string(dir.join("src/parts/mod.rs")).unwrap().ends_with("pub mod garden_pump;\n"));
        let module = fs::read_to_string(dir.join("src/parts/garden_pump.rs")).unwrap();
        // On battery: a command part, no ready-made code (the device sleeps).
        assert!(module.contains("TODO(garden_pump)") && module.contains("pub fn spawn"), "{module}");
        assert!(fs::read_to_string(dir.join("README.md")).unwrap().contains("| Garden pump | GPIO0 |"));
        assert!(fs::read_to_string(dir.join(RECORD)).unwrap().contains("name = \"Garden pump\"\npin = 0"));
        // A reading: read once per wake-up, at its mark.
        add("temperature", &["pin=1"]).unwrap();
        let main = fs::read_to_string(dir.join("src/main.rs")).unwrap();
        let reads = main.find(codegen::READS_MARK).unwrap();
        assert!(main[reads..].contains("parts::garden_pump_2::read(p.GPIO1).await;"), "{main}");
        // Refused: nothing changes.
        let before = fs::read_to_string(dir.join("src/main.rs")).unwrap();
        assert!(add("relay", &["0"]).unwrap_err().to_string().contains("already used"));
        assert!(add("relay", &["7"]).unwrap_err().to_string().contains("already used"), "the I²C bus's SDA");
        assert!(add("relay", &["sda=8"]).unwrap_err().to_string().contains("no pin `sda`"));
        assert_eq!(fs::read_to_string(dir.join("src/main.rs")).unwrap(), before);
    }

    /// The first I²C part brings the bus (`src/i2c.rs`, its line in `main`, its dependencies);
    /// the next ones share it, each at its own address.
    #[test]
    fn adds_i2c() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gen/xtask-test/add-i2c");
        let _ = fs::remove_dir_all(&dir);
        let p = Project::new(toml::from_str("name = \"Lab\"").unwrap(), "/x".into()).unwrap();
        super::super::write(&p, &dir).unwrap();
        let add = |component: &str, pins: &[&str], address: Option<u8>| {
            run(AddArgs {
                component: Some(component.into()),
                name: None,
                pins: pins.iter().map(|s| s.to_string()).collect(),
                address,
                dir: dir.clone(),
            })
        };
        assert!(add("bh1750", &["sda=4"], None).unwrap_err().to_string().contains("both"));
        add("bh1750", &["sda=4", "scl=5"], None).unwrap();
        add("sht4x", &[], None).unwrap();
        let main = fs::read_to_string(dir.join("src/main.rs")).unwrap();
        assert!(main.contains("mod i2c;\nmod led;\nmod parts;"), "{main}");
        let bus = main.find("    let bus = i2c::bus(p.I2C0, p.GPIO4, p.GPIO5);\n\n").unwrap();
        let mark = main.find(codegen::SPAWNS_MARK).unwrap();
        assert!(bus < mark && main.matches("i2c::bus(").count() == 1, "{main}");
        assert!(
            main[mark..].contains("parts::illuminance::spawn(spawner, bus);\n    parts::air::spawn(spawner, bus);")
        );
        assert!(main.contains("&parts::air::AIR_TEMPERATURE,\n        &parts::air::AIR_HUMIDITY,"), "{main}");
        assert!(fs::read_to_string(dir.join("src/i2c.rs")).unwrap().contains("StaticCell<Bus>"));
        let cargo = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert_eq!(cargo.matches("static_cell ").count(), 1, "{cargo}");
        let readme = fs::read_to_string(dir.join("README.md")).unwrap();
        assert!(readme.contains("| Air | I²C 0x44 |") && readme.contains("| I²C bus | GPIO4 (SDA), GPIO5 (SCL) |"));
        // A second SHT4x: another address.
        assert!(add("sht4x", &[], None).unwrap_err().to_string().contains("both at 0x44"));
        assert!(add("sht4x", &[], Some(0x47)).unwrap_err().to_string().contains("0x44 or 0x45 or 0x46"));
        add("sht4x", &[], Some(0x45)).unwrap();
        let record = fs::read_to_string(dir.join(RECORD)).unwrap();
        assert!(record.contains("[i2c]\nsda = 4\nscl = 5") && record.contains("address = 69"), "{record}");
    }

    #[test]
    fn edits() {
        let main = "    .entities(&[\n        // mark\n        &parts::a::A,\n        &led::LED,\n    ]);\n";
        assert_eq!(
            insert(main, "// mark", "&parts::", "&parts::b::B,").unwrap(),
            "    .entities(&[\n        // mark\n        &parts::a::A,\n        &parts::b::B,\n        &led::LED,\n    ]);\n"
        );
        assert!(insert(main, "// other", "&parts::", "x").is_none());
        let cargo = "[dependencies]\niotkit = { git = \"x\", features = [\"esp32c6\"] }\n";
        assert!(feature(cargo, "smartled").unwrap().contains("features = [\"smartled\", \"esp32c6\"]"));
        assert_eq!(feature(&feature(cargo, "smartled").unwrap(), "smartled"), feature(cargo, "smartled"));
        let readme = "# X\n\n## Parts and wiring\n\n| Part | Pin |\n| --- | --- |\n| A | GPIO1 |\n\nText\n";
        assert!(readme_row(readme, "| B | GPIO2 |").unwrap().contains("| A | GPIO1 |\n| B | GPIO2 |\n\nText"));
    }
}
