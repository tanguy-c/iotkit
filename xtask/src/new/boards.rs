//! The boards `cargo iotkit new` knows, and which pins a part may use on each chip.

use std::ops::RangeInclusive;

pub struct Board {
    pub id: &'static str,
    /// As the menu shows it.
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
}

pub const BOARDS: &[Board] = &[
    Board {
        id: "esp32c6-devkitc-1",
        label: "ESP32-C6-DevKitC-1 (Wi-Fi 6, Matter; the board iotkit is tested on)",
        chip: "esp32c6",
        model: "ESP32-C6-DevKitC-1",
        chip_label: "ESP32-C6",
        flash: Some("8mb"),
        boot_pin: 9,
        rgb_led: Some(8),
    },
    Board {
        id: "esp32c3-devkitm-1",
        label: "ESP32-C3-DevKitM-1 (Wi-Fi, small and cheap; no Matter)",
        chip: "esp32c3",
        model: "ESP32-C3-DevKitM-1",
        chip_label: "ESP32-C3",
        flash: Some("4mb"),
        boot_pin: 9,
        rgb_led: Some(8),
    },
    Board {
        id: "esp32c5-devkitc-1",
        label: "ESP32-C5-DevKitC-1 (dual-band Wi-Fi; no Matter yet)",
        chip: "esp32c5",
        model: "ESP32-C5-DevKitC-1",
        chip_label: "ESP32-C5",
        flash: Some("8mb"),
        boot_pin: 28,
        rgb_led: Some(27),
    },
    Board {
        id: "esp32s3-devkitc-1",
        label: "ESP32-S3-DevKitC-1 (more pins and CPU; no Matter yet)",
        chip: "esp32s3",
        model: "ESP32-S3-DevKitC-1",
        chip_label: "ESP32-S3",
        flash: Some("8mb"),
        boot_pin: 0,
        rgb_led: Some(38),
    },
    Board {
        id: "other-esp32c6",
        label: "Another board with an ESP32-C6",
        chip: "esp32c6",
        model: "ESP32-C6 board",
        chip_label: "ESP32-C6",
        flash: None,
        boot_pin: 9,
        rgb_led: None,
    },
    Board {
        id: "other-esp32c3",
        label: "Another board with an ESP32-C3",
        chip: "esp32c3",
        model: "ESP32-C3 board",
        chip_label: "ESP32-C3",
        flash: None,
        boot_pin: 9,
        rgb_led: None,
    },
    Board {
        id: "other-esp32c5",
        label: "Another board with an ESP32-C5",
        chip: "esp32c5",
        model: "ESP32-C5 board",
        chip_label: "ESP32-C5",
        flash: None,
        boot_pin: 28,
        rgb_led: None,
    },
    Board {
        id: "other-esp32s3",
        label: "Another board with an ESP32-S3",
        chip: "esp32s3",
        model: "ESP32-S3 board",
        chip_label: "ESP32-S3",
        flash: None,
        boot_pin: 0,
        rgb_led: None,
    },
];

pub fn board(id: &str) -> Option<&'static Board> {
    BOARDS.iter().find(|b| b.id == id)
}

impl Board {
    /// Matter over Wi-Fi fits and builds (see `iotkit`'s compile errors).
    pub fn matter(&self) -> bool {
        self.chip == "esp32c6"
    }
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
}

fn pins(chip: &str) -> Pins {
    match chip {
        "esp32c3" => Pins {
            max: 21,
            reserved: &[(11..=17, "used by the flash"), (18..=19, "the USB port (console, flashing)")],
            warn: &[(2..=2, "a strapping pin"), (8..=9, "a strapping pin (BOOT, the LED)")],
            adc1: 0..=4,
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
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PinCheck {
    Ok,
    Warn(String),
    Refused(String),
}

/// Whether `pin` suits a part (`analog`: read with the ADC) next to the pins already `taken`.
pub fn check_pin(board: &Board, pin: u8, analog: bool, taken: &[u8]) -> PinCheck {
    let p = pins(board.chip);
    if pin > p.max {
        return PinCheck::Refused(format!("the {} has no GPIO{pin} (up to GPIO{})", board.chip_label, p.max));
    }
    if let Some((_, why)) = p.reserved.iter().find(|(r, _)| r.contains(&pin)) {
        return PinCheck::Refused(format!("GPIO{pin} is {why}"));
    }
    if taken.contains(&pin) {
        return PinCheck::Refused(format!("GPIO{pin} is already used by another part"));
    }
    if analog && !p.adc1.contains(&pin) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c6_pins() {
        let c6 = board("esp32c6-devkitc-1").unwrap();
        assert_eq!(check_pin(c6, 4, false, &[]), PinCheck::Warn(check_pin_text(c6, 4)));
        assert_eq!(check_pin(c6, 2, true, &[]), PinCheck::Ok);
        assert!(matches!(check_pin(c6, 12, false, &[]), PinCheck::Refused(_)), "USB");
        assert!(matches!(check_pin(c6, 26, false, &[]), PinCheck::Refused(_)), "flash");
        assert!(matches!(check_pin(c6, 31, false, &[]), PinCheck::Refused(_)));
        assert!(matches!(check_pin(c6, 10, true, &[]), PinCheck::Refused(_)), "not ADC1");
        assert!(matches!(check_pin(c6, 2, false, &[2]), PinCheck::Refused(_)), "taken");
        assert!(matches!(check_pin(c6, 8, false, &[]), PinCheck::Warn(_)), "LED");
        assert_eq!(check_pin(c6, 18, false, &[]), PinCheck::Ok);
    }

    fn check_pin_text(board: &Board, pin: u8) -> String {
        match check_pin(board, pin, false, &[]) {
            PinCheck::Warn(w) => w,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_chips() {
        let c3 = board("esp32c3-devkitm-1").unwrap();
        assert!(matches!(check_pin(c3, 18, false, &[]), PinCheck::Refused(_)));
        assert_eq!(check_pin(c3, 3, true, &[]), PinCheck::Ok);
        let s3 = board("esp32s3-devkitc-1").unwrap();
        assert!(matches!(check_pin(s3, 23, false, &[]), PinCheck::Refused(_)));
        assert_eq!(check_pin(s3, 40, false, &[]), PinCheck::Ok);
        assert!(matches!(check_pin(s3, 38, false, &[]), PinCheck::Warn(_)), "LED");
        for b in BOARDS {
            assert!(crate::CHIPS.iter().any(|c| c.name == b.chip), "{}", b.id);
        }
    }
}
