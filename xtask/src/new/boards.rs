//! The boards `cargo iotkit new` knows, and which pins a part may use on each chip.

use std::ops::RangeInclusive;

pub struct Board {
    /// The file's name in `boards/`.
    pub id: &'static str,
    /// As the menu shows it (searched as you type).
    pub label: &'static str,
    /// Cargo feature and `espflash --chip`.
    pub chip: &'static str,
    /// Home Assistant's "model" and "chip" lines.
    pub model: &'static str,
    pub chip_label: &'static str,
    /// `None`: asked (another board).
    pub flash: Option<&'static str>,
    pub boot_pin: u8,
    /// The onboard WS2812 LED.
    pub rgb_led: Option<u8>,
    /// A single onboard LED: its pin, and whether a low level lights it.
    pub plain_led: Option<(u8, bool)>,
    /// Pins `main` sets before the radio starts (an antenna switch).
    pub init: Vec<Init>,
    order: u32,
}

/// A pin set at start, and why.
pub struct Init {
    pub pin: u8,
    pub high: bool,
    pub why: &'static str,
}

mod files {
    include!(concat!(env!("OUT_DIR"), "/boards.rs"));
}

/// A board file (`boards/<id>.toml`).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    label: String,
    chip: String,
    model: String,
    flash: Option<String>,
    boot_pin: u8,
    order: u32,
    led: Option<LedFile>,
    #[serde(default)]
    init: Vec<InitFile>,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LedFile {
    kind: String,
    pin: u8,
    active: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InitFile {
    pin: u8,
    level: String,
    why: String,
}

/// The board files, checked: an error names the file.
fn load() -> Result<Vec<Board>, String> {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let mut boards = Vec::new();
    for (id, text) in files::FILES {
        let f: File = toml::from_str(text).map_err(|e| format!("boards/{id}.toml: {e}"))?;
        let chip_label = match f.chip.as_str() {
            "esp32c6" => "ESP32-C6",
            "esp32c3" => "ESP32-C3",
            "esp32c5" => "ESP32-C5",
            "esp32s3" => "ESP32-S3",
            "esp32h2" => "ESP32-H2",
            "esp32" => "ESP32",
            other => return Err(format!("boards/{id}.toml: unknown chip `{other}`")),
        };
        if f.flash.as_deref().is_some_and(|f| f != "4mb" && f != "8mb") {
            return Err(format!("boards/{id}.toml: flash is `4mb` or `8mb`"));
        }
        let level = |l: Option<&str>| match l {
            None | Some("high") => Ok(false),
            Some("low") => Ok(true),
            Some(other) => Err(format!("boards/{id}.toml: level `{other}`: `high` or `low`")),
        };
        let (rgb_led, plain_led) = match f.led {
            None => (None, None),
            Some(LedFile { kind, pin, active }) if kind == "rgb" && active.is_none() => (Some(pin), None),
            Some(LedFile { kind, pin, active }) if kind == "plain" => (None, Some((pin, level(active.as_deref())?))),
            Some(_) => return Err(format!("boards/{id}.toml: led: `kind = \"rgb\"` or `\"plain\"` (with `active`)")),
        };
        let init = f
            .init
            .into_iter()
            .map(|i| Ok(Init { pin: i.pin, high: !level(Some(&i.level))?, why: leak(i.why) }))
            .collect::<Result<_, String>>()?;
        if f.sources.is_empty() && !id.starts_with("other-") {
            return Err(format!("boards/{id}.toml: `sources`: where its facts come from"));
        }
        boards.push(Board {
            id,
            label: leak(f.label),
            chip: leak(f.chip),
            model: leak(f.model),
            chip_label,
            flash: f.flash.map(leak),
            boot_pin: f.boot_pin,
            rgb_led,
            plain_led,
            init,
            order: f.order,
        });
    }
    boards.sort_by_key(|b| (b.order, b.id));
    Ok(boards)
}

/// Every board, in menu order.
pub fn boards() -> &'static [Board] {
    static BOARDS: std::sync::OnceLock<Vec<Board>> = std::sync::OnceLock::new();
    BOARDS.get_or_init(|| load().unwrap_or_else(|e| panic!("{e}")))
}

pub fn board(id: &str) -> Option<&'static Board> {
    boards().iter().find(|b| b.id == id)
}

impl Board {
    /// Matter over Wi-Fi fits and builds (see `iotkit`'s compile errors).
    pub fn matter(&self) -> bool {
        self.chip == "esp32c6"
    }

    /// An 802.15.4 radio: Matter over Thread.
    pub fn thread(&self) -> bool {
        matches!(self.chip, "esp32c6" | "esp32h2")
    }

    /// Wi-Fi: no Thread-only chip.
    pub fn wifi(&self) -> bool {
        self.chip != "esp32h2"
    }
}

/// What a part does with its pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinUse {
    /// Reads a level (a contact, a button).
    Input,
    /// Reads a voltage with the ADC.
    Analog,
    /// Drives it (a relay, LEDs, PWM).
    Output,
}

/// Pins of a chip a part must not or should not use.
struct Pins {
    /// Highest GPIO number.
    max: u8,
    /// Refused: the flash, USB.
    reserved: &'static [(RangeInclusive<u8>, &'static str)],
    /// Accepted with a warning: strapping pins (their level at reset picks the boot mode).
    warn: &'static [(RangeInclusive<u8>, &'static str)],
    /// ADC1 (analog readings; ADC2 is taken by Wi-Fi).
    adc1: RangeInclusive<u8>,
    /// Pins that cannot drive anything.
    input_only: Option<RangeInclusive<u8>>,
}

fn pins(chip: &str) -> Pins {
    match chip {
        "esp32c3" => Pins {
            max: 21,
            reserved: &[(11..=17, "used by the flash"), (18..=19, "the USB port (console, flashing)")],
            warn: &[(2..=2, "a strapping pin"), (8..=9, "a strapping pin (BOOT, the LED)")],
            adc1: 0..=4,
            input_only: None,
        },
        "esp32c5" => Pins {
            max: 28,
            reserved: &[(13..=14, "the USB port (console, flashing)")],
            warn: &[
                (2..=2, "a strapping pin"),
                (7..=7, "a strapping pin"),
                (16..=22, "used by the flash on most modules: check your board"),
                (25..=28, "a strapping pin (BOOT, the LED)"),
            ],
            adc1: 1..=6,
            input_only: None,
        },
        "esp32s3" => Pins {
            max: 48,
            reserved: &[
                (19..=20, "the USB port (console, flashing)"),
                (22..=25, "not on the chip"),
                (26..=32, "used by the flash"),
            ],
            warn: &[
                (0..=0, "a strapping pin (BOOT)"),
                (3..=3, "a strapping pin"),
                (33..=37, "used by the PSRAM on octal-PSRAM modules"),
                (45..=46, "a strapping pin"),
            ],
            adc1: 1..=10,
            input_only: None,
        },
        "esp32" => Pins {
            max: 39,
            reserved: &[
                (1..=1, "the console's TX (UART0)"),
                (3..=3, "the console's RX (UART0)"),
                (6..=11, "used by the flash"),
                (20..=20, "not on the chip"),
                (24..=24, "not on the chip"),
                (28..=31, "not on the chip"),
            ],
            warn: &[
                (0..=0, "a strapping pin (BOOT)"),
                (2..=2, "a strapping pin"),
                (5..=5, "a strapping pin"),
                (12..=12, "a strapping pin (the flash voltage: keep it low at reset)"),
                (15..=15, "a strapping pin"),
            ],
            adc1: 32..=39,
            input_only: Some(34..=39),
        },
        "esp32h2" => Pins {
            max: 27,
            reserved: &[(15..=21, "used by the flash"), (26..=27, "the USB port (console, flashing)")],
            warn: &[
                (2..=3, "a strapping pin"),
                (8..=9, "a strapping pin (BOOT, the LED)"),
                (25..=25, "a strapping pin"),
            ],
            adc1: 1..=5,
            input_only: None,
        },
        // esp32c6
        _ => Pins {
            max: 30,
            reserved: &[(12..=13, "the USB port (console, flashing)"), (24..=30, "used by the flash")],
            warn: &[
                (4..=5, "a strapping pin"),
                (8..=9, "a strapping pin (BOOT, the LED)"),
                (15..=15, "a strapping pin"),
            ],
            adc1: 0..=6,
            input_only: None,
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PinCheck {
    Ok,
    Warn(String),
    Refused(String),
}

/// Whether `pin` suits a part using it as `kind`, next to the pins already `taken`.
pub fn check_pin(board: &Board, pin: u8, kind: PinUse, taken: &[u8]) -> PinCheck {
    let p = pins(board.chip);
    if pin > p.max {
        return PinCheck::Refused(format!("the {} has no GPIO{pin} (up to GPIO{})", board.chip_label, p.max));
    }
    if let Some((_, why)) = p.reserved.iter().find(|(r, _)| r.contains(&pin)) {
        return PinCheck::Refused(format!("GPIO{pin} is {why}"));
    }
    if let Some(init) = board.init.iter().find(|i| i.pin == pin) {
        return PinCheck::Refused(format!("GPIO{pin} is used by the board: it {}", init.why));
    }
    if taken.contains(&pin) {
        return PinCheck::Refused(format!("GPIO{pin} is already used by another part"));
    }
    if kind == PinUse::Output && p.input_only.as_ref().is_some_and(|r| r.contains(&pin)) {
        return PinCheck::Refused(format!("GPIO{pin} is input only: a sensor or a button can use it"));
    }
    if kind == PinUse::Analog && !p.adc1.contains(&pin) {
        return PinCheck::Refused(format!(
            "analog readings need an ADC1 pin: GPIO{} to GPIO{} on the {}",
            p.adc1.start(),
            p.adc1.end(),
            board.chip_label
        ));
    }
    if Some(pin) == board.rgb_led {
        return PinCheck::Warn(format!("GPIO{pin} drives the board's RGB LED"));
    }
    if board.plain_led.is_some_and(|(led, _)| led == pin) {
        return PinCheck::Warn(format!("GPIO{pin} drives the board's LED"));
    }
    if pin == board.boot_pin {
        return PinCheck::Warn(format!("GPIO{pin} is the BOOT button"));
    }
    match p.warn.iter().find(|(r, _)| r.contains(&pin)) {
        Some((_, why)) => PinCheck::Warn(format!(
            "GPIO{pin} is {why}: fine for an output or a sensor, but do not pull it at power-on"
        )),
        None => PinCheck::Ok,
    }
}

impl Board {
    /// The pins that wake the chip from deep sleep (its low-power ones).
    pub fn wake_pins(&self) -> Option<RangeInclusive<u8>> {
        match self.chip {
            "esp32c6" => Some(0..=7),
            "esp32c3" => Some(0..=5),
            "esp32c5" => Some(0..=6),
            "esp32s3" => Some(0..=21),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c6_pins() {
        let c6 = board("esp32c6-devkitc-1").unwrap();
        assert_eq!(check_pin(c6, 4, PinUse::Input, &[]), PinCheck::Warn(check_pin_text(c6, 4)));
        assert_eq!(check_pin(c6, 2, PinUse::Analog, &[]), PinCheck::Ok);
        assert!(matches!(check_pin(c6, 12, PinUse::Input, &[]), PinCheck::Refused(_)), "USB");
        assert!(matches!(check_pin(c6, 26, PinUse::Input, &[]), PinCheck::Refused(_)), "flash");
        assert!(matches!(check_pin(c6, 31, PinUse::Input, &[]), PinCheck::Refused(_)));
        assert!(matches!(check_pin(c6, 10, PinUse::Analog, &[]), PinCheck::Refused(_)), "not ADC1");
        assert!(matches!(check_pin(c6, 2, PinUse::Input, &[2]), PinCheck::Refused(_)), "taken");
        assert!(matches!(check_pin(c6, 8, PinUse::Input, &[]), PinCheck::Warn(_)), "LED");
        assert_eq!(check_pin(c6, 18, PinUse::Input, &[]), PinCheck::Ok);
    }

    fn check_pin_text(board: &Board, pin: u8) -> String {
        match check_pin(board, pin, PinUse::Input, &[]) {
            PinCheck::Warn(w) => w,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_chips() {
        let c3 = board("esp32c3-devkitm-1").unwrap();
        assert!(matches!(check_pin(c3, 18, PinUse::Input, &[]), PinCheck::Refused(_)));
        assert_eq!(check_pin(c3, 3, PinUse::Analog, &[]), PinCheck::Ok);
        let s3 = board("esp32s3-devkitc-1").unwrap();
        assert!(matches!(check_pin(s3, 23, PinUse::Input, &[]), PinCheck::Refused(_)));
        assert_eq!(check_pin(s3, 40, PinUse::Input, &[]), PinCheck::Ok);
        assert!(matches!(check_pin(s3, 38, PinUse::Input, &[]), PinCheck::Warn(_)), "LED");
        let esp32 = board("esp32-devkitc").unwrap();
        assert!(matches!(check_pin(esp32, 7, PinUse::Input, &[]), PinCheck::Refused(_)), "flash");
        assert!(matches!(check_pin(esp32, 3, PinUse::Input, &[]), PinCheck::Refused(_)), "console");
        assert!(matches!(check_pin(esp32, 35, PinUse::Output, &[]), PinCheck::Refused(_)), "input only");
        assert_eq!(check_pin(esp32, 35, PinUse::Analog, &[]), PinCheck::Ok);
        assert!(matches!(check_pin(esp32, 25, PinUse::Analog, &[]), PinCheck::Refused(_)), "ADC2");
        let h2 = board("other-esp32h2").unwrap();
        assert!(matches!(check_pin(h2, 16, PinUse::Input, &[]), PinCheck::Refused(_)), "flash");
        assert_eq!(check_pin(h2, 10, PinUse::Output, &[]), PinCheck::Ok);
        for b in boards() {
            assert!(crate::CHIPS.iter().any(|c| c.name == b.chip), "{}", b.id);
        }
    }

    /// Every file in `boards/` loads, and its pins exist and are free on its chip.
    #[test]
    fn board_files() {
        let all = load().unwrap();
        assert!(all.len() >= 15);
        assert_eq!(all[0].id, "esp32c6-devkitc-1", "the tested board first");
        for b in &all {
            let own = [Some(b.boot_pin), b.rgb_led, b.plain_led.map(|(p, _)| p)];
            for pin in own.into_iter().flatten().chain(b.init.iter().map(|i| i.pin)) {
                let p = pins(b.chip);
                assert!(pin <= p.max && !p.reserved.iter().any(|(r, _)| r.contains(&pin)), "{}: GPIO{pin}", b.id);
            }
        }
        let xiao = board("xiao-esp32c6").unwrap();
        assert_eq!((xiao.flash, xiao.plain_led, xiao.rgb_led), (Some("4mb"), Some((15, true)), None));
        assert_eq!(xiao.init.iter().map(|i| (i.pin, i.high)).collect::<Vec<_>>(), [(3, false), (14, false)]);
        assert!(matches!(check_pin(xiao, 3, PinUse::Input, &[]), PinCheck::Refused(_)), "the antenna switch");
    }
}
