//! The parts a builder can add, from `components/*.toml`: what Home Assistant (and Matter) sees,
//! and the code or the TODO left in the generated project.

use super::boards::PinUse;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Measures,
    Detects,
    Controls,
    Inputs,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Measures => "Measures",
            Group::Detects => "Detects",
            Group::Controls => "Controls",
            Group::Inputs => "Inputs",
        }
    }
}

/// How the generated code drives the part without ready-made code: the entity type's loop.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stub {
    /// A reading (`Sensor::set`), every 30 s.
    Reading,
    /// Something detected (`BinarySensor::set`).
    Detect,
    /// Presses (`Event::trigger`).
    Presses,
    Switch,
    Light,
    Cover,
    Lock,
    Fan,
    Climate,
    Number,
    Select,
}

/// How a part talks to the chip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bus {
    GpioIn,
    GpioOut,
    Adc,
    I2c,
    OneWire,
    Uart,
    Ledc,
    Rmt,
}

impl Bus {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "gpio-in" => Bus::GpioIn,
            "gpio-out" => Bus::GpioOut,
            "adc" => Bus::Adc,
            "i2c" => Bus::I2c,
            "one-wire" => Bus::OneWire,
            "uart" => Bus::Uart,
            "ledc" => Bus::Ledc,
            "rmt" => Bus::Rmt,
            _ => return None,
        })
    }

    /// The esp-hal 1.2 setup, on one line, for `pin`.
    pub fn hint(self, pin: &str) -> String {
        match self {
            Bus::GpioIn => format!("GPIO: `Input::new({pin}, InputConfig::default().with_pull(Pull::Up))`"),
            Bus::GpioOut => {
                format!(
                    "GPIO: `Output::new({pin}, Level::Low, OutputConfig::default())` once, then `set_high()`/`set_low()`"
                )
            }
            Bus::Adc => {
                format!(
                    "ADC: `config.enable_pin({pin}, Attenuation::_11dB)`, `Adc::new(p.ADC1, config).into_async()` (ESP32: blocking)"
                )
            }
            Bus::I2c => {
                format!("I²C: `I2c::new(p.I2C0, Default::default()).unwrap().with_sda({pin}).with_scl(…).into_async()`")
            }
            Bus::OneWire => {
                format!(
                    "1-Wire: `Flex::new({pin})`, `apply_output_config` with `DriveMode::OpenDrain`, input and output enabled"
                )
            }
            Bus::Uart => format!(
                "UART: `Uart::new(p.UART1, cfg).unwrap().with_rx({pin}).into_async()`, `cfg`: `uart::Config::default().with_baudrate(9600)`"
            ),
            Bus::Ledc => format!(
                "LEDC: `Ledc::new(p.LEDC)`, `timer::<LowSpeed>(Timer0)`, `channel(Channel0, {pin})`, `set_duty(percent)`"
            ),
            Bus::Rmt => format!(
                "RMT: `Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap().into_async()` (one per chip), its `channel1` on `{pin}`"
            ),
        }
    }
}

/// One of a part's entities: most parts have one; a sensor chip one per value it measures.
pub struct EntitySpec {
    /// In a part with several, what this one measures (`temperature`): its id is the part's,
    /// then this; its name the part's, then `name`.
    pub key: &'static str,
    pub name: &'static str,
    /// The entity type, its constructor (`new`, or a shortcut such as `temperature`), the
    /// arguments after the id and the name, and the builder calls after it.
    pub ty: &'static str,
    pub ctor: &'static str,
    pub args: &'static str,
    pub builder: &'static str,
    /// The Matter device type it shows as, if any.
    pub matter: Option<&'static str>,
    /// For an I²C chip: its value in the reading `r` (`r.temperature`).
    pub value: &'static str,
}

/// A sensor chip on the project's I²C bus.
pub struct I2cSpec {
    /// Its addresses, the default first.
    pub addresses: Vec<u8>,
    /// Liquid: an `Option` of the reading, from `{{ i2c }}` (a `&mut` I²C bus) and `ADDR`.
    pub read: &'static str,
}

/// A pin a part needs.
pub struct PinSpec {
    /// Its variable in the generated code (`pin`, `sda`).
    pub role: &'static str,
    pub pin_use: PinUse,
}

/// Ready-made code (Liquid templates): the part works as generated.
pub struct Code {
    /// `use` paths, one item each.
    pub uses: Vec<&'static str>,
    /// Peripherals only one part can have (`ADC1`), passed from `main`.
    pub takes: Vec<&'static str>,
    /// iotkit features it needs.
    pub features: Vec<&'static str>,
    /// Lines in `spawn` before the task starts, the task's parameters, what `spawn` passes it,
    /// and its body.
    pub setup: &'static str,
    pub param: &'static str,
    pub arg: &'static str,
    pub body: &'static str,
    /// On a battery device, a reading's code once per wake-up (the body of `read`, which gets
    /// the same parameters as `spawn`) and its `use` paths; empty: a TODO there.
    pub read: &'static str,
    pub read_uses: Vec<&'static str>,
}

pub struct Kind {
    /// The file's name in `components/`: `kind = "temperature"` in answers files.
    pub id: &'static str,
    pub group: Group,
    order: u32,
    /// In the menu.
    pub label: &'static str,
    /// The default entity name.
    pub name: &'static str,
    /// Its entities, one at least.
    pub entities: Vec<EntitySpec>,
    /// What to connect (README's wiring table).
    pub wiring: &'static str,
    pub pins: Vec<PinSpec>,
    /// The TODO in the generated code: what to do, in the part's terms.
    pub todo: &'static str,
    /// How it can be wired (alternatives): one setup hint each under the TODO.
    pub buses: Vec<Bus>,
    /// Driver crates (no_std, embedded-hal 1.0), checked on crates.io.
    pub drivers: &'static str,
    /// The ready-made code, if there is one.
    pub code: Option<Code>,
    /// A chip on the I²C bus (ready-made once the bus has its pins).
    pub i2c: Option<I2cSpec>,
}

impl Kind {
    /// Its (first) entity's type.
    pub fn ty(&self) -> &'static str {
        self.entities[0].ty
    }

    /// Comes with code that works as generated (given its pins, or the I²C bus).
    pub fn ready_made(&self) -> bool {
        self.code.is_some() || self.i2c.is_some()
    }

    /// What its first pin does (`PinUse::Input` for a part with none).
    pub fn pin_use(&self) -> PinUse {
        self.pins.first().map_or(PinUse::Input, |p| p.pin_use)
    }

    /// Reads a pin with the ADC.
    pub fn analog(&self) -> bool {
        self.pins.iter().any(|p| p.pin_use == PinUse::Analog)
    }

    /// The task's loop around the TODO, from the entity type.
    pub fn stub(&self) -> Stub {
        stub(self.ty()).expect("checked when loaded")
    }

    /// Read once per wake-up on a battery device (rather than driven by commands).
    pub fn is_reading(&self) -> bool {
        matches!(self.stub(), Stub::Reading | Stub::Detect)
    }
}

fn stub(ty: &str) -> Option<Stub> {
    Some(match ty {
        "Sensor" => Stub::Reading,
        "BinarySensor" => Stub::Detect,
        "Event" => Stub::Presses,
        "Switch" => Stub::Switch,
        "Light" => Stub::Light,
        "Cover" | "Valve" => Stub::Cover,
        "Lock" => Stub::Lock,
        "Fan" => Stub::Fan,
        "Climate" => Stub::Climate,
        "Number" => Stub::Number,
        "Select" => Stub::Select,
        _ => return None,
    })
}

mod files {
    include!(concat!(env!("OUT_DIR"), "/components.rs"));
}

/// A component file (`components/<id>.toml`; its README has the format).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    label: String,
    group: String,
    order: u32,
    name: String,
    wiring: String,
    matter: Option<String>,
    entity: Option<EntityFile>,
    /// A chip measuring several values: one each.
    #[serde(default)]
    entities: Vec<EntityFile>,
    #[serde(default)]
    pins: Vec<PinFile>,
    todo: TodoFile,
    code: Option<CodeFile>,
    i2c: Option<I2cFile>,
    /// Where its facts come from (a sensor's bus and address: ESPHome's component, Zephyr's
    /// binding, the datasheet).
    #[serde(default)]
    #[allow(dead_code)]
    sources: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityFile {
    /// In `[[entities]]`: what it measures, and its name after the part's.
    key: Option<String>,
    name: Option<String>,
    matter: Option<String>,
    #[serde(rename = "type", default = "sensor")]
    ty: String,
    #[serde(default = "new")]
    ctor: String,
    #[serde(default)]
    args: String,
    #[serde(default)]
    builder: String,
    #[serde(default = "reading")]
    value: String,
}

fn new() -> String {
    "new".into()
}

fn sensor() -> String {
    "Sensor".into()
}

fn reading() -> String {
    "r".into()
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct I2cFile {
    addresses: Vec<u8>,
    read: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PinFile {
    #[serde(default = "pin")]
    role: String,
    #[serde(rename = "use")]
    pin_use: String,
}

fn pin() -> String {
    "pin".into()
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TodoFile {
    text: String,
    #[serde(default)]
    buses: Vec<String>,
    #[serde(default)]
    drivers: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeFile {
    #[serde(default)]
    uses: Vec<String>,
    #[serde(default)]
    takes: Vec<String>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    setup: String,
    param: String,
    arg: String,
    body: String,
    #[serde(default)]
    read: String,
    #[serde(default)]
    read_uses: Vec<String>,
}

/// A lowercase Rust identifier, not a keyword (a variable or a module of the generated code).
pub fn ident(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !KEYWORDS.contains(&s)
}

const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate", "do", "dyn", "else",
    "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "macro", "match", "mod",
    "move", "mut", "override", "priv", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
    "try", "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// The component files, checked: an error names the file.
fn load() -> Result<Vec<Kind>, String> {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let mut kinds = Vec::new();
    for (id, text) in files::FILES {
        let err = |e: &dyn std::fmt::Display| format!("components/{id}.toml: {e}");
        let f: File = toml::from_str(text).map_err(|e| err(&e))?;
        let group = match f.group.as_str() {
            "measures" => Group::Measures,
            "detects" => Group::Detects,
            "controls" => Group::Controls,
            "inputs" => Group::Inputs,
            other => return Err(err(&format!("group `{other}`: measures, detects, controls or inputs"))),
        };
        let several = !f.entities.is_empty();
        let files = match (f.entity, several) {
            (Some(e), false) => vec![e],
            (None, true) => f.entities,
            _ => return Err(err(&"`[entity]`, or `[[entities]]` for a chip measuring several values")),
        };
        let mut entities: Vec<EntitySpec> = Vec::new();
        for e in files {
            if stub(&e.ty).is_none() {
                return Err(err(&format!("entity type `{}`: not one the wizard has a loop for", e.ty)));
            }
            let (key, name, matter) = match (several, e.key, e.name) {
                (false, None, None) => (String::new(), f.name.clone(), f.matter.clone().or(e.matter)),
                (true, Some(key), Some(name)) if ident(&key) && !entities.iter().any(|x| x.key == key) => {
                    (key, name, e.matter)
                }
                (false, ..) => return Err(err(&"`[entity]`: no `key` or `name` (the part's name is its name)")),
                (true, ..) => return Err(err(&"`[[entities]]`: each a distinct `key` (a Rust name) and a `name`")),
            };
            entities.push(EntitySpec {
                key: leak(key),
                name: leak(name),
                ty: leak(e.ty),
                ctor: leak(e.ctor),
                args: leak(e.args),
                builder: leak(e.builder),
                matter: matter.map(leak),
                value: leak(e.value),
            });
        }
        if several && (f.i2c.is_none() || f.matter.is_some()) {
            return Err(err(&"`[[entities]]`: an I²C chip's (`[i2c]`), each with its own `matter`"));
        }
        let mut pins: Vec<PinSpec> = Vec::new();
        for p in f.pins {
            let pin_use = match p.pin_use.as_str() {
                "input" => PinUse::Input,
                "output" => PinUse::Output,
                "analog" => PinUse::Analog,
                other => return Err(err(&format!("pin use `{other}`: input, output or analog"))),
            };
            if !ident(&p.role) || pins.iter().any(|q| q.role == p.role) {
                return Err(err(&format!("pin role `{}`: a distinct lowercase Rust name", p.role)));
            }
            pins.push(PinSpec { role: leak(p.role), pin_use });
        }
        let buses = f
            .todo
            .buses
            .iter()
            .map(|b| Bus::parse(b).ok_or_else(|| err(&format!("bus `{b}`"))))
            .collect::<Result<_, _>>()?;
        let i2c = match f.i2c {
            None => None,
            Some(_) if f.code.is_some() || !pins.is_empty() => {
                return Err(err(&"`[i2c]`: its code is `read`, on the bus (no `[code]` or `[[pins]]`)"));
            }
            Some(_) if entities.iter().any(|e| stub(e.ty) != Some(Stub::Reading)) => {
                return Err(err(&"`[i2c]`: sensors only (read every 30 s, or once per wake-up)"));
            }
            Some(i) if i.addresses.is_empty() || i.addresses.iter().any(|a| !(0x08..0x78).contains(a)) => {
                return Err(err(&"`[i2c]`: `addresses`, 7-bit (0x08 to 0x77), the default first"));
            }
            Some(i) => Some(I2cSpec { addresses: i.addresses, read: leak(i.read) }),
        };
        let code = match f.code {
            None => None,
            Some(_) if pins.is_empty() => return Err(err(&"`[code]` drives pins: add `[[pins]]`")),
            Some(c) => {
                if let Some(u) = c
                    .uses
                    .iter()
                    .chain(&c.read_uses)
                    .find(|u| !u.contains("::") || u.contains(['{', ','].as_slice()) && !u.contains("{%"))
                {
                    return Err(err(&format!("uses `{u}`: one path each (`esp_hal::gpio::Output`)")));
                }
                if let Some(t) = c.takes.iter().find(|t| !ident(&t.to_lowercase())) {
                    return Err(err(&format!("takes `{t}`: a peripheral of `p` (`ADC1`)")));
                }
                let all = |v: Vec<String>| v.into_iter().map(leak).collect();
                Some(Code {
                    uses: all(c.uses),
                    takes: all(c.takes),
                    features: all(c.features),
                    setup: leak(c.setup),
                    param: leak(c.param),
                    arg: leak(c.arg),
                    body: leak(c.body),
                    read: leak(c.read),
                    read_uses: all(c.read_uses),
                })
            }
        };
        kinds.push(Kind {
            id,
            group,
            order: f.order,
            label: leak(f.label),
            name: leak(f.name),
            entities,
            wiring: leak(f.wiring),
            pins,
            todo: leak(f.todo.text),
            buses,
            drivers: leak(f.todo.drivers),
            code,
            i2c,
        });
    }
    kinds.sort_by_key(|k| (k.group as u8, k.order, k.id));
    Ok(kinds)
}

/// Every part, in menu order (by group).
pub fn kinds() -> &'static [Kind] {
    static KINDS: std::sync::OnceLock<Vec<Kind>> = std::sync::OnceLock::new();
    KINDS.get_or_init(|| load().unwrap_or_else(|e| panic!("{e}")))
}

pub fn kind(id: &str) -> Option<&'static Kind> {
    kinds().iter().find(|k| k.id == id)
}

/// `"Living room temperature"` → `"living_room_temperature"` (entity id, file names).
pub fn slug(name: &str, sep: char) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c.to_ascii_lowercase(),
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with(sep) {
            out.push(sep);
        }
    }
    while out.ends_with(sep) {
        out.pop();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, if sep == '_' { "e_" } else { "d-" });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Living room Température!", '_'), "living_room_temperature");
        assert_eq!(slug("  Garden sensor ", '-'), "garden-sensor");
        assert_eq!(slug("2nd floor", '_'), "e_2nd_floor");
        assert_eq!(slug("CO2", '_'), "co2");
    }

    /// Every file in `components/` loads, in its group's order.
    #[test]
    fn component_files() {
        let all = load().unwrap();
        assert!(all.len() >= 20);
        assert_eq!(all[0].id, "sht4x", "the menu's first");
        for g in [Group::Measures, Group::Detects, Group::Controls, Group::Inputs] {
            assert!(all.iter().any(|k| k.group == g), "{}", g.label());
        }
        let relay = kind("relay").unwrap();
        assert!(relay.code.is_some() && relay.pin_use() == PinUse::Output && relay.entities[0].ctor == "new");
        let bme = kind("bme280").unwrap();
        assert_eq!(bme.entities.iter().map(|e| e.key).collect::<Vec<_>>(), ["temperature", "humidity", "pressure"]);
        assert!(bme.i2c.is_some() && bme.pins.is_empty() && bme.ready_made());
        assert!(kind("soil_moisture").unwrap().analog() && kind("setting").unwrap().pins.is_empty());
        assert!(!ident("mod") && !ident("Pin") && ident("sda") && ident("living_room"));
    }

    /// Every wired part gets a hint per bus, on the pin it is given.
    #[test]
    fn hints() {
        for k in kinds() {
            let wired = !k.wiring.starts_with("(none") && k.id != "number";
            assert_eq!(!k.buses.is_empty(), wired, "{}", k.id);
            assert_eq!(k.pins.is_empty() && k.i2c.is_none(), k.wiring.starts_with("(none"), "{}", k.id);
            for bus in &k.buses {
                assert!(bus.hint("p.GPIO23").contains("p.GPIO23"), "{} {bus:?}", k.id);
            }
            assert_eq!(k.analog(), k.buses == [Bus::Adc], "{}", k.id);
        }
    }
}
