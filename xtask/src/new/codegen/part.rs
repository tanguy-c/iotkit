//! A part's module, `src/parts/<id>.rs`: its entity, the `spawn` (or, read once per wake-up, the
//! `read`) that `main` calls with its pins, and its task. `cargo iotkit new` and `add` share it.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use anyhow::{Context, Result};

use super::{comment, group, wrap_comments};
use crate::new::{
    Part, Project,
    parts::{Bus, Stub},
};

/// `use` paths, one item each, written grouped by crate.
#[derive(Default)]
pub struct Uses(BTreeSet<String>);

impl Uses {
    pub fn add(&mut self, path: impl Into<String>) {
        self.0.insert(path.into());
    }

    pub fn extend<'a>(&mut self, paths: impl IntoIterator<Item = &'a str>) {
        self.0.extend(paths.into_iter().map(String::from));
    }

    /// One `use` line per crate.
    pub fn lines(&self) -> String {
        let mut crates: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for path in &self.0 {
            let (head, rest) = path.split_once("::").expect("a path");
            crates.entry(head).or_default().insert(rest);
        }
        crates.iter().map(|(head, rest)| format!("use {head}::{};\n", group(rest))).collect()
    }
}

/// How `main` starts a part.
pub enum Call {
    /// `parts::pump::spawn(spawner, p.GPIO4);`, among the spawns.
    Spawn(String),
    /// `parts::temperature::read(p.GPIO5).await;`, once per wake-up.
    Read(String),
}

pub struct PartCode {
    /// `src/parts/<id>.rs`.
    pub file: String,
    /// In `Config::entities`, one per entity: `&parts::pump::PUMP`.
    pub entities: Vec<String>,
    pub call: Call,
    /// iotkit features its code needs.
    pub features: Vec<&'static str>,
}

/// Whether each part gets its ready-made code: an I²C chip once the bus has its pins; another
/// part with every pin chosen and the peripherals it takes not taken by an earlier part,
/// plugged in (or a Matter ICD) or, on a battery device, a reading with code for it.
pub fn ready(p: &Project) -> Vec<bool> {
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    p.parts
        .iter()
        .map(|part| match (&part.kind.i2c, &part.kind.code) {
            (Some(_), _) => p.answers.i2c.is_some(),
            (None, Some(code))
                if (!p.sleepy() || part.kind.is_reading() && !code.read.is_empty())
                    && part.pins.iter().all(Option::is_some)
                    && code.takes.iter().all(|t| !taken.contains(t)) =>
            {
                taken.extend(code.takes.iter().copied());
                true
            }
            _ => false,
        })
        .collect()
}

/// The iotkit features the parts' ready-made code needs.
pub fn features(p: &Project) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (part, ready) in p.parts.iter().zip(ready(p)) {
        if let (true, Some(code)) = (ready, &part.kind.code) {
            out.extend(code.features.iter().copied().filter(|f| !out.contains(f)).collect::<Vec<_>>());
        }
    }
    out
}

/// The module of `p.parts[index]`.
pub fn part(p: &Project, index: usize) -> Result<PartCode> {
    let part = &p.parts[index];
    let k = part.kind;
    let ready = ready(p)[index];
    let c = &part.entities[0].konst;
    let mut uses = Uses::default();
    for e in &part.entities {
        uses.add(format!("iotkit::{}", e.spec.ty));
        for ty in ["DeviceClass", "Unit", "HvacMode"] {
            if e.spec.builder.contains(&format!("{ty}::")) || e.spec.args.contains(&format!("{ty}::")) {
                uses.add(format!("iotkit::{ty}"));
            }
        }
    }
    // The pins it was given, `(role, GPIO number)`.
    let pins: Vec<(&str, u8)> =
        k.pins.iter().zip(&part.pins).filter_map(|(spec, pin)| pin.map(|n| (spec.role, n))).collect();
    let mut params: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for &(role, n) in &pins {
        uses.add(format!("esp_hal::peripherals::GPIO{n}"));
        let var = if ready { role.to_string() } else { format!("_{role}") };
        params.push(format!("{var}: GPIO{n}<'static>"));
        args.push(format!("p.GPIO{n}"));
    }
    let on_pin = match (part.address, pins.as_slice()) {
        (Some(address), _) => format!(", at {address:#04x} on the I²C bus"),
        (None, []) => String::new(),
        (None, [(_, n)]) => format!(", on GPIO{n}"),
        (None, many) => {
            format!(", on {}", many.iter().map(|(r, n)| format!("GPIO{n} ({r})")).collect::<Vec<_>>().join(", "))
        }
    };

    let mut s = String::new();
    match k.wiring.strip_prefix("(none: ").and_then(|w| w.strip_suffix(')')) {
        Some(what) => _ = writeln!(s, "//! {}: {what}.", part.name),
        None => _ = writeln!(s, "//! {}{on_pin}: {}.", part.name, k.wiring),
    }
    let module = format!("parts::{}", part.module);
    let mut body = String::new();
    let mut features = Vec::new();
    let ctx = || format!("components/{}.toml", k.id);
    // The ready-made code's templates, for this part.
    let render = |tpl: &str, i2c: &str| {
        let globals = liquid::object!({
            "entity": c,
            "id": part.id,
            "chip": p.board.chip,
            "i2c": i2c,
            "pins": pins.iter().map(|(r, n)| ((*r).into(), liquid::model::Value::scalar(i64::from(*n)))).collect::<liquid::Object>(),
        });
        let parser = liquid::ParserBuilder::with_stdlib().build()?;
        let out = parser.parse(tpl)?.render(&globals)?;
        anyhow::Ok(out.trim().to_string())
    };
    let doc_extra = match (p.sleepy(), p.esphome()) {
        (false, _) => "",
        (true, false) => " Commands sent while the device sleeps arrive at its next wake-up.",
        (true, true) => " Commands reach it only while it is awake.",
    };
    let call = match (&k.i2c, part.address) {
        (Some(i2c), Some(address)) if ready => {
            // One reading, reported: the same in a task (every 30 s; a Matter ICD less often) and
            // on battery (once per wake-up).
            let read = render(i2c.read, if p.sleepy() { "i2c" } else { "&mut i2c" }).with_context(ctx)?;
            if read.contains("components::") {
                uses.add("iotkit::components");
            }
            uses.add("log::warn");
            let others: Vec<String> =
                i2c.addresses.iter().filter(|a| **a != address).map(|a| format!("{a:#04x}")).collect();
            let others = if others.is_empty() { String::new() } else { format!(" (or {})", others.join(", ")) };
            let _ = write!(body, "\n/// Its I²C address{others}.\nconst ADDR: u8 = {address:#04x};\n");
            let sets: String = match part.entities.as_slice() {
                [one] if one.spec.value == "r" => format!(" {}.set(r),", one.konst),
                many => format!(
                    " {{\n{}    }}",
                    many.iter().map(|e| format!("        {}.set({});\n", e.konst, e.spec.value)).collect::<String>()
                ),
            };
            let report = format!(
                "match {read} {{\n    Some(r) =>{sets}\n    None => warn!(\"{}: no reading from the {} at {{ADDR:#04x}}\"),\n}}",
                part.id, k.id
            );
            let indent = |text: &str, n: usize| -> String { text.lines().map(|l| format!("{:n$}{l}\n", "")).collect() };
            if p.sleepy() {
                let _ = write!(
                    body,
                    "\n/// Read once per wake-up, from `main`.\npub async fn read(i2c: &mut crate::i2c::Bus) {{\n{}}}\n",
                    indent(&report, 4)
                );
                Call::Read(format!("{module}::read(&mut bus).await;"))
            } else {
                uses.extend(["embassy_executor::Spawner", "embassy_time::Duration", "embassy_time::Ticker"]);
                let _ = write!(
                    body,
                    "
/// Starts it (from `main`), on the I²C bus.
pub fn spawn(spawner: Spawner, bus: &'static crate::i2c::Bus) {{
    spawner.spawn(task(crate::i2c::Device::new(bus)).unwrap());
}}

/// {}, every {}.
#[embassy_executor::task]
async fn task(mut i2c: crate::i2c::Device) {{
    let mut ticker = Ticker::every(Duration::from_secs({}));
    loop {{
{}        ticker.next().await;
    }}
}}
",
                    part.name,
                    every(p),
                    p.read_every_secs(),
                    indent(&report, 8)
                );
                Call::Spawn(format!("{module}::spawn(spawner, bus);"))
            }
        }
        _ if p.sleepy() && k.is_reading() => {
            let code = match &k.code {
                Some(code) if ready => {
                    for t in &code.takes {
                        uses.add(format!("esp_hal::peripherals::{t}"));
                        params.push(format!("{}: {t}<'static>", t.to_lowercase()));
                        args.push(format!("p.{t}"));
                    }
                    let read = render(code.read, "").with_context(ctx)?;
                    for u in &code.read_uses {
                        uses.add(render(u, "").with_context(ctx)?);
                    }
                    format!("{read}\n").lines().map(|l| format!("    {l}\n")).collect()
                }
                _ => todo(4, part, &format!("{}, then {}", k.todo, sets(part))),
            };
            let _ = write!(
                body,
                "
/// Read once per wake-up, from `main`.
pub async fn read({}) {{
{code}}}
",
                params.join(", "),
            );
            Call::Read(format!("{module}::read({}).await;", args.join(", ")))
        }
        _ => {
            uses.add("embassy_executor::Spawner");
            let (setup, param, arg, task_body) = match (&k.code, ready) {
                (Some(code), true) => {
                    for t in &code.takes {
                        uses.add(format!("esp_hal::peripherals::{t}"));
                        params.push(format!("{}: {t}<'static>", t.to_lowercase()));
                        args.push(format!("p.{t}"));
                    }
                    features.extend(code.features.iter().copied());
                    for u in &code.uses {
                        uses.add(render(u, "").with_context(ctx)?);
                    }
                    (
                        render(code.setup, "").with_context(ctx)?,
                        render(code.param, "").with_context(ctx)?,
                        render(code.arg, "").with_context(ctx)?,
                        render(code.body, "").with_context(ctx)?,
                    )
                }
                _ => {
                    let names: Vec<String> = pins.iter().map(|(r, _)| r.to_string()).collect();
                    (String::new(), params.join(", "), names.join(", "), stub(part, p.read_every_secs(), &mut uses))
                }
            };
            let indent = |text: &str| -> String {
                text.lines().map(|l| if l.is_empty() { "\n".to_string() } else { format!("    {l}\n") }).collect()
            };
            let spawn_params: Vec<String> = std::iter::once("spawner: Spawner".to_string())
                .chain(pins.iter().map(|(r, n)| format!("{r}: GPIO{n}<'static>")))
                .chain(params.iter().skip(pins.len()).cloned())
                .collect();
            let _ = write!(
                body,
                "
/// Starts it (from `main`).
pub fn spawn({}) {{
{}    spawner.spawn(task({arg}).unwrap());
}}

/// {}{on_pin}.{doc_extra}
#[embassy_executor::task]
async fn task({param}) {{
{}}}
",
                spawn_params.join(", "),
                indent(&setup),
                part.name,
                indent(&task_body)
            );
            Call::Spawn(format!(
                "{module}::spawn({});",
                std::iter::once("spawner".to_string()).chain(args).collect::<Vec<_>>().join(", ")
            ))
        }
    };
    s.push('\n');
    s.push_str(&uses.lines());
    s.push('\n');
    for e in &part.entities {
        let spec = e.spec;
        let ty = spec.ty;
        let _ = writeln!(
            s,
            "pub static {}: {ty} = {ty}::{}({:?}, {:?}{}){};",
            e.konst, spec.ctor, e.id, e.name, spec.args, spec.builder
        );
    }
    s.push_str(&body);
    Ok(PartCode {
        file: wrap_comments(&s),
        entities: part.entities.iter().map(|e| format!("&{module}::{}", e.konst)).collect(),
        call,
        features,
    })
}

/// How often a sensor's task reads it, in words: `30 s`, `5 min`.
fn every(p: &Project) -> String {
    match p.read_every_secs() {
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

/// `A.set(..)`, or with several entities `A.set(..)`, `B.set(..)`, in code quotes.
fn sets(part: &Part) -> String {
    part.entities.iter().map(|e| format!("`{}.set(..)`", e.konst)).collect::<Vec<_>>().join(", ")
}

/// The entity type's loop around the part's TODO.
fn stub(part: &Part, every_secs: u64, uses: &mut Uses) -> String {
    let k = part.kind;
    let c = &part.entities[0].konst;
    let set = format!("{}, then {}", k.todo, sets(part));
    // Commands: the TODO sits between the command and the state it reports.
    let t = todo(4, part, k.todo);
    match k.stub() {
        Stub::Reading => {
            uses.extend(["embassy_time::Duration", "embassy_time::Ticker"]);
            format!(
                "let mut ticker = Ticker::every(Duration::from_secs({}));
loop {{
{}    ticker.next().await;
}}",
                every_secs,
                todo(4, part, &set)
            )
        }
        Stub::Detect | Stub::Presses => {
            uses.add("embassy_time::Timer");
            let head = match k.stub() {
                Stub::Detect => format!("{set} at each change (`wait_for_any_edge().await`, not the timer)"),
                _ => k.todo.to_string(),
            };
            format!(
                "loop {{
{}    Timer::after_secs(1).await;
}}",
                todo(4, part, &head)
            )
        }
        Stub::Switch => format!(
            "{c}.set(false);
loop {{
    let on = {c}.wait_command().await;
{t}    {c}.set(on);
}}"
        ),
        Stub::Light => {
            uses.add("iotkit::LightState");
            format!(
                "let mut state = LightState {{ on: false, brightness: 255, rgb: [255, 255, 255] }};
loop {{
{t}    {c}.set(state);
    state = {c}.wait_command().await;
}}"
            )
        }
        Stub::Cover => {
            uses.extend(["iotkit::CoverCommand", "iotkit::CoverState"]);
            format!(
                "{c}.set(Some(CoverState::Closed), Some(0));
loop {{
    let position = match {c}.wait_command().await {{
        CoverCommand::Open => 100,
        CoverCommand::Close => 0,
        CoverCommand::Position(p) => p,
        CoverCommand::Stop => continue,
    }};
{t}    {c}.set(Some(if position == 0 {{ CoverState::Closed }} else {{ CoverState::Open }}), Some(position));
}}"
            )
        }
        Stub::Lock => {
            uses.extend(["iotkit::LockCommand", "iotkit::LockState"]);
            format!(
                "{c}.set(LockState::Locked);
loop {{
    let state = match {c}.wait_command().await {{
        LockCommand::Lock => LockState::Locked,
        LockCommand::Unlock | LockCommand::Open => LockState::Unlocked,
    }};
{t}    {c}.set(state);
}}"
            )
        }
        Stub::Fan => {
            uses.add("iotkit::FanState");
            format!(
                "{c}.set(FanState::OFF);
loop {{
    let wanted = {c}.wait_command().await;
{t}    {c}.set(wanted);
}}"
            )
        }
        Stub::Climate => {
            uses.extend(["iotkit::ClimateState", "iotkit::HvacMode"]);
            format!(
                "let mut state = ClimateState {{ mode: HvacMode::Heat, target: 20.0, current: None, action: None }};
loop {{
{t}    {c}.set(state);
    let wanted = {c}.wait_command().await;
    state.mode = wanted.mode;
    state.target = wanted.target;
}}"
            )
        }
        Stub::Number => format!(
            "{c}.set(50.0);
loop {{
    let value = {c}.wait_command().await;
{t}    {c}.set(value);
}}"
        ),
        Stub::Select => format!(
            "{c}.set(0);
loop {{
    let index = {c}.wait_command().await;
{t}    {c}.set(index);
}}"
        ),
    }
}

/// A part's `TODO(<id>)`: `head`, then a setup hint per bus (on its first pin's variable) and the
/// driver crates.
fn todo(indent: usize, part: &Part, head: &str) -> String {
    let k = part.kind;
    let unpinned = if k.i2c.is_some() {
        " (the I²C bus has no pins yet: create it in `main`)"
    } else if part.pins.iter().any(Option::is_none) && !k.buses.is_empty() {
        " (no pin chosen yet: give it one in `main`)"
    } else {
        ""
    };
    let mut s = comment(indent, &format!("TODO({}): {head}{unpinned}.", part.id));
    let pin = match (k.pins.first(), part.pins.first()) {
        (Some(spec), Some(Some(_))) => format!("_{}", spec.role),
        _ => "<pin>".into(),
    };
    for &bus in &k.buses {
        s += &comment(indent, &format!("  {}", Bus::hint(bus, &pin)));
    }
    if !k.drivers.is_empty() {
        s += &comment(indent, &format!("  drivers: {}", k.drivers));
    }
    s
}
