//! Matter pairing codes on the host: what an iotkit device built with `MATTER_PASSCODE` and
//! `MATTER_DISCRIMINATOR` answers to (Matter core specification 5.1.3 and 5.1.4).

use anyhow::{Result, bail};

/// The test vendor and product rs-matter's `TEST_DEV_DET` (and so iotkit) uses.
const VENDOR_ID: u16 = 0xfff1;
const PRODUCT_ID: u16 = 0x8001;
/// Discovery capabilities: on the IP network (iotkit commissions over Wi-Fi, not Bluetooth).
const ON_NETWORK: u8 = 0b100;
const BASE38: &[u8; 38] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-.";

/// Passcodes the Matter specification forbids (5.1.7.1); as `iotkit-link-matter`'s.
const INVALID_PASSCODES: [u32; 12] =
    [0, 11111111, 22222222, 33333333, 44444444, 55555555, 66666666, 77777777, 88888888, 99999999, 12345678, 87654321];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub passcode: u32,
    pub discriminator: u16,
}

impl Setup {
    /// A new random code.
    pub fn random() -> Result<Self> {
        loop {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
            let a = u32::from_le_bytes(bytes[..4].try_into()?);
            let b = u32::from_le_bytes(bytes[4..].try_into()?);
            let setup = Self { passcode: a % 99_999_998 + 1, discriminator: (b & 0xfff) as u16 };
            if setup.valid() {
                return Ok(setup);
            }
        }
    }

    pub fn valid(&self) -> bool {
        (1..=99_999_998).contains(&self.passcode)
            && !INVALID_PASSCODES.contains(&self.passcode)
            && self.discriminator < 4096
    }

    /// The 11-digit code typed in a controller's app.
    pub fn manual_code(&self) -> String {
        let short = u32::from(self.discriminator >> 8);
        let digits =
            format!("{}{:05}{:04}", short >> 2, ((short & 0b11) << 14) | (self.passcode & 0x3fff), self.passcode >> 14);
        let check = verhoeff(&digits);
        format!("{digits}{check}")
    }

    /// The same, grouped as apps show it: `1234-567-8901`.
    pub fn manual_code_pretty(&self) -> String {
        let c = self.manual_code();
        format!("{}-{}-{}", &c[..4], &c[4..7], &c[7..])
    }

    /// The text of the QR code (`MT:...`), without optional data.
    pub fn qr_text(&self) -> String {
        let mut bits = Bits::default();
        bits.push(0, 3); // version
        bits.push(u64::from(VENDOR_ID), 16);
        bits.push(u64::from(PRODUCT_ID), 16);
        bits.push(0, 2); // standard commissioning flow
        bits.push(u64::from(ON_NETWORK), 8);
        bits.push(u64::from(self.discriminator), 12);
        bits.push(u64::from(self.passcode), 27);
        bits.push(0, 4); // padding
        format!("MT:{}", base38_encode(&bits.bytes))
    }

    /// The QR code as an SVG image (for a label).
    pub fn qr_svg(&self) -> Result<String> {
        let code = qrcode::QrCode::new(self.qr_text())?;
        Ok(code.render::<qrcode::render::svg::Color>().min_dimensions(240, 240).quiet_zone(true).build())
    }

    /// The QR code drawn with block characters, for the terminal.
    pub fn qr_terminal(&self) -> Result<String> {
        let code = qrcode::QrCode::new(self.qr_text())?;
        Ok(code
            .render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .build())
    }
}

/// What a QR code holds (its fixed part); to check codes printed by a device.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub struct QrFields {
    pub vendor_id: u16,
    pub product_id: u16,
    pub discovery: u8,
    pub setup: Setup,
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_qr(text: &str) -> Result<QrFields> {
    let Some(data) = text.strip_prefix("MT:") else { bail!("not a Matter QR code: {text}") };
    let bytes = base38_decode(data)?;
    if bytes.len() < 11 {
        bail!("QR code too short");
    }
    let mut bits = BitReader { bytes: &bytes, at: 0 };
    let version = bits.take(3);
    if version != 0 {
        bail!("QR code version {version}");
    }
    let vendor_id = bits.take(16) as u16;
    let product_id = bits.take(16) as u16;
    let _flow = bits.take(2);
    let discovery = bits.take(8) as u8;
    let discriminator = bits.take(12) as u16;
    let passcode = bits.take(27) as u32;
    Ok(QrFields { vendor_id, product_id, discovery, setup: Setup { passcode, discriminator } })
}

/// The passcode and the discriminator's top 4 bits from an 11-digit manual code.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_manual(code: &str) -> Result<(u32, u16)> {
    let digits: String = code.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 11 || verhoeff(&digits[..10]) != digits[10..].parse::<u8>()? {
        bail!("not an 11-digit manual code: {code}");
    }
    let first: u32 = digits[..1].parse()?;
    let middle: u32 = digits[1..6].parse()?;
    let last: u32 = digits[6..10].parse()?;
    let short = ((first & 0b11) << 2) | (middle >> 14);
    Ok(((last << 14) | (middle & 0x3fff), short as u16))
}

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    len: usize,
}

impl Bits {
    /// Appends the `n` low bits of `value`, least significant first.
    fn push(&mut self, value: u64, n: usize) {
        for i in 0..n {
            if self.len.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if value >> i & 1 == 1 {
                *self.bytes.last_mut().expect("pushed") |= 1 << (self.len % 8);
            }
            self.len += 1;
        }
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl BitReader<'_> {
    fn take(&mut self, n: usize) -> u64 {
        let mut v = 0;
        for i in 0..n {
            let bit = self.bytes[self.at / 8] >> (self.at % 8) & 1;
            v |= u64::from(bit) << i;
            self.at += 1;
        }
        v
    }
}

/// 3 bytes → 5 characters, then 2 → 4 or 1 → 2; least significant first.
fn base38_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
        let chars = [2, 4, 5][chunk.len() - 1];
        let mut v = value;
        for _ in 0..chars {
            out.push(BASE38[(v % 38) as usize] as char);
            v /= 38;
        }
    }
    out
}

fn base38_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in text.as_bytes().chunks(5) {
        let bytes = match chunk.len() {
            5 => 3,
            4 => 2,
            2 => 1,
            _ => bail!("bad base38 length"),
        };
        let mut v = 0u32;
        for &c in chunk.iter().rev() {
            let Some(d) = BASE38.iter().position(|&b| b == c) else { bail!("bad base38 character {}", c as char) };
            v = v * 38 + d as u32;
        }
        out.extend_from_slice(&v.to_le_bytes()[..bytes]);
    }
    Ok(out)
}

/// The Verhoeff check digit of `digits`.
fn verhoeff(digits: &str) -> u8 {
    const D: [[u8; 10]; 10] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
        [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
        [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
        [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
        [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
        [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
        [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
        [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
        [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
    ];
    const P: [[u8; 10]; 8] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
        [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
        [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
        [9, 4, 5, 3, 1, 2, 7, 6, 8, 0],
        [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
        [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
        [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
    ];
    const INV: [u8; 10] = [0, 4, 3, 2, 1, 5, 6, 7, 8, 9];
    let c = digits
        .bytes()
        .rev()
        .enumerate()
        .fold(0u8, |c, (i, b)| D[c as usize][P[(i + 1) % 8][(b - b'0') as usize] as usize]);
    INV[c as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Matter SDK's test code.
    #[test]
    fn sdk_test_code() {
        let s = Setup { passcode: 20202021, discriminator: 3840 };
        assert_eq!(s.manual_code(), "34970112332");
        assert_eq!(s.manual_code_pretty(), "3497-011-2332");
        assert_eq!(parse_manual("34970112332").unwrap(), (20202021, 15));
    }

    /// Codes an iotkit device printed (rs-matter's encoders): this module reads them back, and
    /// writes the same manual code.
    #[test]
    fn device_codes() {
        let qr = parse_qr("MT:-24J0SO527FA.D0NG4P04A5D0VF2P3HD1P3H4373UW4T0").unwrap();
        assert_eq!((qr.vendor_id, qr.product_id, qr.discovery), (VENDOR_ID, PRODUCT_ID, ON_NETWORK));
        let (passcode, short) = parse_manual("1178-823-9021").unwrap();
        assert_eq!(qr.setup.passcode, passcode);
        assert_eq!(qr.setup.discriminator >> 8, short);
        assert_eq!(qr.setup.manual_code_pretty(), "1178-823-9021");
        // Ours has no optional data: the same fixed part.
        let ours = qr.setup.qr_text();
        assert_eq!(parse_qr(&ours).unwrap(), qr);
        assert!(ours.len() < 30, "{ours}");
    }

    /// A device built with `MATTER_PASSCODE = 79771663`, `MATTER_DISCRIMINATOR = 985` printed
    /// these: the wizard's codes for the same setup match.
    #[test]
    fn built_in_code() {
        let s = Setup { passcode: 79771663, discriminator: 985 };
        assert_eq!(s.manual_code_pretty(), "0635-034-8682");
        let device = parse_qr("MT:-24J0IRV013KYJ3CT4P04A5D0VF2P3HD1P3H4373UW4T0").unwrap();
        assert_eq!(device, parse_qr(&s.qr_text()).unwrap());
    }

    #[test]
    fn random_codes_round_trip() {
        for _ in 0..200 {
            let s = Setup::random().unwrap();
            assert!(s.valid());
            assert_eq!(parse_qr(&s.qr_text()).unwrap().setup, s);
            let (passcode, short) = parse_manual(&s.manual_code()).unwrap();
            assert_eq!((passcode, short), (s.passcode, s.discriminator >> 8));
        }
        assert!(!Setup { passcode: 12345678, discriminator: 1 }.valid());
        assert!(Setup::random().unwrap().qr_svg().unwrap().starts_with("<?xml"));
    }
}
