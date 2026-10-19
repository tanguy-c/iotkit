//! Firmware update manifest, shared by the device and the host tooling.
//!
//! `cargo xtask build` writes a `manifest.json` next to each app image:
//!
//! ```json
//! {"product":"devkit","version":"1.2.0","min_version":"1.0.0","size":812304,
//!  "sha256":"<64 hex>","url":"devkit-1.2.0.bin","signature":"<128 hex>"}
//! ```
//!
//! The Ed25519 signature covers everything except `url` (see [`Manifest::signed_message`]), so a
//! device checks the manifest before downloading, then the image's size and SHA-256 after.

#![no_std]

use core::{fmt, str};

use serde::{Deserialize, Serialize};
pub use sha2::{Digest, Sha256};

/// Prefix of every signed message, so a key is never tricked into signing something else.
const DOMAIN: &[u8; 16] = b"iotkit-image-v1\0";

/// Longest product name.
pub const MAX_PRODUCT_LEN: usize = 32;

/// Largest signed message: domain, product length and name, two versions, size, SHA-256.
const MAX_MESSAGE_LEN: usize = DOMAIN.len() + 1 + MAX_PRODUCT_LEN + 6 + 6 + 4 + 32;

/// Ed25519 public key that verifies manifests (32 bytes).
pub type PublicKey = [u8; 32];

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a manifest, or a buffer too small to write one.
    Json,
    /// A field is not valid hex of the expected length.
    Hex,
    /// A version is not `major.minor.patch`.
    Version,
    /// The product name is empty or longer than [`MAX_PRODUCT_LEN`].
    Product,
    /// The signature does not match the public key.
    Signature,
    /// The manifest is for another product.
    WrongProduct,
    /// The update is not newer than the running firmware.
    NotNewer,
    /// The running firmware is older than the update's `min_version`.
    TooOld,
    /// The downloaded image has another size than the manifest says.
    Size,
    /// The downloaded image has another SHA-256 than the manifest says.
    Hash,
    /// Not an ESP app image, or its app descriptor is missing.
    NotAnImage,
    /// An app image for another chip.
    WrongChip,
    /// The app descriptor's project or version differs from the manifest.
    Mismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::Json => "invalid manifest JSON",
            Error::Hex => "invalid hex field",
            Error::Version => "version is not major.minor.patch",
            Error::Product => "product name empty or too long",
            Error::Signature => "bad signature",
            Error::WrongProduct => "manifest is for another product",
            Error::NotNewer => "update is not newer than the running firmware",
            Error::TooOld => "running firmware is older than min_version",
            Error::Size => "image size mismatch",
            Error::Hash => "image SHA-256 mismatch",
            Error::NotAnImage => "not an ESP app image",
            Error::WrongChip => "app image for another chip",
            Error::Mismatch => "app image project or version differs from the manifest",
        })
    }
}

impl core::error::Error for Error {}

/// A `major.minor.patch` version. Pre-release suffixes are not supported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self { major, minor, patch }
    }

    /// Parses `"1.2.3"`.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let mut parts = s.split('.').map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::Version);
            }
            p.parse::<u16>().map_err(|_| Error::Version)
        });
        let mut next = || parts.next().ok_or(Error::Version)?;
        let version = Self::new(next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(Error::Version);
        }
        Ok(version)
    }

    fn to_le_bytes(self) -> [u8; 6] {
        let [a, b] = self.major.to_le_bytes();
        let [c, d] = self.minor.to_le_bytes();
        let [e, f] = self.patch.to_le_bytes();
        [a, b, c, d, e, f]
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A parsed firmware manifest. String fields borrow from the JSON it was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest<'a> {
    /// Which firmware this is (the app's package name).
    pub product: &'a str,
    pub version: Version,
    /// Oldest running version allowed to install this update (e.g. after a config format change).
    pub min_version: Version,
    /// Image size in bytes.
    pub size: u32,
    pub sha256: [u8; 32],
    /// Where to download the image, relative to the manifest or absolute.
    pub url: &'a str,
    pub signature: [u8; 64],
}

/// The JSON form.
#[derive(Serialize, Deserialize)]
struct Json<'a> {
    product: &'a str,
    version: &'a str,
    min_version: &'a str,
    size: u32,
    sha256: &'a str,
    url: &'a str,
    signature: &'a str,
}

impl<'a> Manifest<'a> {
    /// Reads a manifest. Does not check the signature: call [`verify`](Self::verify).
    pub fn parse(json: &'a [u8]) -> Result<Self, Error> {
        let (j, _) = serde_json_core::from_slice::<Json<'a>>(json).map_err(|_| Error::Json)?;
        if j.product.is_empty() || j.product.len() > MAX_PRODUCT_LEN {
            return Err(Error::Product);
        }
        Ok(Self {
            product: j.product,
            version: Version::parse(j.version)?,
            min_version: Version::parse(j.min_version)?,
            size: j.size,
            sha256: hex_decode(j.sha256)?,
            url: j.url,
            signature: hex_decode(j.signature)?,
        })
    }

    /// Writes the manifest as JSON into `buf`; returns the length.
    pub fn to_json(&self, buf: &mut [u8]) -> Result<usize, Error> {
        let (mut version, mut min_version) = ([0; 17], [0; 17]);
        let (mut sha256, mut signature) = ([0; 64], [0; 128]);
        let j = Json {
            product: self.product,
            version: write_version(self.version, &mut version),
            min_version: write_version(self.min_version, &mut min_version),
            size: self.size,
            sha256: hex_encode(&self.sha256, &mut sha256),
            url: self.url,
            signature: hex_encode(&self.signature, &mut signature),
        };
        serde_json_core::to_slice(&j, buf).map_err(|_| Error::Json)
    }

    /// The bytes the signature covers: every field but `url` and `signature`.
    pub fn signed_message(&self, buf: &mut [u8; MAX_MESSAGE_LEN]) -> Result<usize, Error> {
        let product = self.product.as_bytes();
        if product.is_empty() || product.len() > MAX_PRODUCT_LEN {
            return Err(Error::Product);
        }
        let mut n = 0;
        let mut put = |bytes: &[u8]| {
            buf[n..n + bytes.len()].copy_from_slice(bytes);
            n += bytes.len();
        };
        put(DOMAIN);
        put(&[product.len() as u8]);
        put(product);
        put(&self.version.to_le_bytes());
        put(&self.min_version.to_le_bytes());
        put(&self.size.to_le_bytes());
        put(&self.sha256);
        Ok(n)
    }

    /// Checks the signature against `key`.
    pub fn verify(&self, key: &PublicKey) -> Result<(), Error> {
        let mut buf = [0; MAX_MESSAGE_LEN];
        let n = self.signed_message(&mut buf)?;
        let key = ed25519_compact::PublicKey::new(*key);
        let signature = ed25519_compact::Signature::new(self.signature);
        key.verify(&buf[..n], &signature).map_err(|_| Error::Signature)
    }

    /// Checks that a device running `product` at `running` may install this update (no downgrades).
    /// Call after [`verify`](Self::verify).
    pub fn check_update(&self, product: &str, running: Version) -> Result<(), Error> {
        if self.product != product {
            Err(Error::WrongProduct)
        } else if running < self.min_version {
            Err(Error::TooOld)
        } else if self.version <= running {
            Err(Error::NotNewer)
        } else {
            Ok(())
        }
    }

    /// Checks a downloaded image's size and SHA-256 (see [`Sha256`]).
    pub fn check_image(&self, size: u32, sha256: &[u8; 32]) -> Result<(), Error> {
        if size != self.size {
            Err(Error::Size)
        } else if sha256 != &self.sha256 {
            Err(Error::Hash)
        } else {
            Ok(())
        }
    }

    /// Checks the start of a downloaded image (at least [`app::HEAD_LEN`] bytes): an app for
    /// `chip_id` whose descriptor names this manifest's product and version.
    pub fn check_app(&self, head: &[u8], chip_id: u16) -> Result<(), Error> {
        let app = app::AppInfo::parse(head)?;
        if app.chip_id != chip_id {
            return Err(Error::WrongChip);
        }
        let mut version = [0; 17];
        if app.project != self.product || app.version != write_version(self.version, &mut version) {
            return Err(Error::Mismatch);
        }
        Ok(())
    }

    /// Signs the manifest with a 64-byte Ed25519 secret key (seed followed by public key).
    #[cfg(feature = "sign")]
    pub fn sign(&mut self, secret_key: &[u8; 64]) -> Result<(), Error> {
        let mut buf = [0; MAX_MESSAGE_LEN];
        let n = self.signed_message(&mut buf)?;
        let key = ed25519_compact::SecretKey::new(*secret_key);
        // Deterministic signatures: no randomness needed on the host either.
        self.signature = *key.sign(&buf[..n], None);
        Ok(())
    }
}

/// The header of an ESP-IDF app image (`espflash save-image` output).
pub mod app {
    use super::Error;

    /// Bytes needed by [`AppInfo::parse`]: image header, first segment header, and the app
    /// descriptor up to its project name.
    pub const HEAD_LEN: usize = 112;
    /// `ESP_IMAGE_HEADER_MAGIC`.
    const IMAGE_MAGIC: u8 = 0xe9;
    /// `ESP_APP_DESC_MAGIC_WORD`, first in the first segment (`esp_app_desc!`).
    const DESC_MAGIC: u32 = 0xabcd_5432;
    /// Chip ids in the image header (espflash's `Chip::id`).
    pub const ESP32C3: u16 = 5;
    pub const ESP32S3: u16 = 9;
    pub const ESP32C6: u16 = 13;
    pub const ESP32H2: u16 = 16;
    pub const ESP32C5: u16 = 23;

    /// What an app image says about itself.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct AppInfo<'a> {
        pub chip_id: u16,
        pub version: &'a str,
        pub project: &'a str,
    }

    impl<'a> AppInfo<'a> {
        pub fn parse(head: &'a [u8]) -> Result<Self, Error> {
            let head = head.get(..HEAD_LEN).ok_or(Error::NotAnImage)?;
            let word = |at: usize| u32::from_le_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
            if head[0] != IMAGE_MAGIC || word(32) != DESC_MAGIC {
                return Err(Error::NotAnImage);
            }
            let text = |field: &'a [u8]| {
                let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
                core::str::from_utf8(&field[..end]).map_err(|_| Error::NotAnImage)
            };
            Ok(Self {
                chip_id: u16::from_le_bytes([head[12], head[13]]),
                version: text(&head[48..80])?,
                project: text(&head[80..112])?,
            })
        }
    }
}

/// Parses a hex public key at compile time, for
/// `static KEY: PublicKey = public_key(include_str!("../keys/ota.pub"));`. Surrounding
/// whitespace is ignored; anything else that is not 64 hex digits fails the build.
pub const fn public_key(hex: &str) -> PublicKey {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("public key: not a hex digit"),
        }
    }
    let s = hex.trim_ascii().as_bytes();
    assert!(s.len() == 64, "public key: expected 64 hex digits");
    let mut key = [0; 32];
    let mut i = 0;
    while i < 32 {
        key[i] = nibble(s[2 * i]) << 4 | nibble(s[2 * i + 1]);
        i += 1;
    }
    key
}

/// SHA-256 of a whole image.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn write_version(v: Version, buf: &mut [u8; 17]) -> &str {
    struct W<'b>(&'b mut [u8], usize);
    impl fmt::Write for W<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let end = self.1 + s.len();
            self.0.get_mut(self.1..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
            self.1 = end;
            Ok(())
        }
    }
    let mut w = W(buf, 0);
    // "65535.65535.65535" is 17 bytes: always fits.
    let _ = fmt::write(&mut w, format_args!("{v}"));
    let n = w.1;
    str::from_utf8(&buf[..n]).unwrap_or_default()
}

/// Lower-case hex of `bytes` into `out` (twice as long).
pub fn hex_encode<'b>(bytes: &[u8], out: &'b mut [u8]) -> &'b str {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for (i, b) in bytes.iter().enumerate() {
        out[2 * i] = DIGITS[usize::from(b >> 4)];
        out[2 * i + 1] = DIGITS[usize::from(b & 0xf)];
    }
    str::from_utf8(&out[..2 * bytes.len()]).unwrap_or_default()
}

/// Decodes exactly `N` bytes of hex (either case).
pub fn hex_decode<const N: usize>(s: &str) -> Result<[u8; N], Error> {
    fn nibble(c: u8) -> Result<u8, Error> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(Error::Hex),
        }
    }
    let s = s.as_bytes();
    if s.len() != 2 * N {
        return Err(Error::Hex);
    }
    let mut out = [0; N];
    for (o, [hi, lo]) in out.iter_mut().zip(s.as_chunks::<2>().0) {
        *o = nibble(*hi)? << 4 | nibble(*lo)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_compact::{KeyPair, Seed};

    fn keys() -> KeyPair {
        KeyPair::from_seed(Seed::new([7; 32]))
    }

    fn signed() -> Manifest<'static> {
        let mut m = Manifest {
            product: "devkit",
            version: Version::new(1, 2, 0),
            min_version: Version::new(1, 0, 0),
            size: 4,
            sha256: sha256(b"fw!!"),
            url: "devkit-1.2.0.bin",
            signature: [0; 64],
        };
        let n = {
            let mut buf = [0; MAX_MESSAGE_LEN];
            let n = m.signed_message(&mut buf).unwrap();
            m.signature = *keys().sk.sign(&buf[..n], None);
            n
        };
        assert!(n <= MAX_MESSAGE_LEN);
        m
    }

    #[test]
    fn versions() {
        assert_eq!(Version::parse("1.20.3"), Ok(Version::new(1, 20, 3)));
        for bad in ["", "1", "1.2", "1.2.3.4", "1.2.x", "1.2.3-rc1", "1..3", "70000.0.0", "+1.2.3"] {
            assert_eq!(Version::parse(bad), Err(Error::Version), "{bad}");
        }
        assert!(Version::new(1, 10, 0) > Version::new(1, 9, 9));
        let mut buf = [0; 17];
        assert_eq!(write_version(Version::new(65535, 65535, 65535), &mut buf), "65535.65535.65535");
    }

    #[test]
    fn const_public_key() {
        const KEY: PublicKey = public_key("  000102030405060708091011121314151617181920212223242526272829300a\n");
        let _ = KEY;
        let parsed = public_key("000102030405060708090a0b0c0d0e0f101112131415161718191A1B1C1D1E1F\n");
        assert_eq!(parsed, core::array::from_fn(|i| i as u8));
    }

    #[test]
    fn hex_round_trip() {
        let mut out = [0; 8];
        assert_eq!(hex_encode(&[0x01, 0xab, 0xff, 0x10], &mut out), "01abff10");
        assert_eq!(hex_decode::<4>("01ABff10"), Ok([0x01, 0xab, 0xff, 0x10]));
        assert_eq!(hex_decode::<4>("01abff1"), Err(Error::Hex));
        assert_eq!(hex_decode::<2>("zz00"), Err(Error::Hex));
    }

    #[test]
    fn json_round_trip_and_verify() {
        let m = signed();
        let mut buf = [0; 512];
        let n = m.to_json(&mut buf).unwrap();
        let parsed = Manifest::parse(&buf[..n]).unwrap();
        assert_eq!(parsed, m);
        assert_eq!(parsed.verify(&keys().pk), Ok(()));
    }

    #[cfg(feature = "sign")]
    #[test]
    fn sign_matches_manual_signature() {
        let expected = signed();
        let mut m = Manifest { signature: [0; 64], ..expected };
        m.sign(&keys().sk).unwrap();
        assert_eq!(m.signature, expected.signature);
    }

    #[test]
    fn tampering_breaks_the_signature() {
        let key = keys().pk;
        let m = signed();
        let tampered = [
            Manifest { version: Version::new(9, 0, 0), ..m },
            Manifest { min_version: Version::new(0, 0, 0), ..m },
            Manifest { size: 5, ..m },
            Manifest { sha256: [0; 32], ..m },
            Manifest { product: "other", ..m },
        ];
        for t in tampered {
            assert_eq!(t.verify(&key), Err(Error::Signature));
        }
        // The URL is not signed: the image hash protects the content.
        assert_eq!(Manifest { url: "https://mirror/x.bin", ..m }.verify(&key), Ok(()));
        let other = KeyPair::from_seed(Seed::new([8; 32])).pk;
        assert_eq!(m.verify(&other), Err(Error::Signature));
    }

    #[test]
    fn update_policy() {
        let m = signed(); // 1.2.0, min 1.0.0
        assert_eq!(m.check_update("devkit", Version::new(1, 1, 5)), Ok(()));
        assert_eq!(m.check_update("devkit", Version::new(1, 2, 0)), Err(Error::NotNewer));
        assert_eq!(m.check_update("devkit", Version::new(2, 0, 0)), Err(Error::NotNewer));
        assert_eq!(m.check_update("devkit", Version::new(0, 9, 0)), Err(Error::TooOld));
        assert_eq!(m.check_update("sensor", Version::new(1, 1, 0)), Err(Error::WrongProduct));
    }

    #[test]
    fn image_check() {
        let m = signed();
        assert_eq!(m.check_image(4, &sha256(b"fw!!")), Ok(()));
        assert_eq!(m.check_image(5, &sha256(b"fw!!")), Err(Error::Size));
        assert_eq!(m.check_image(4, &sha256(b"fw!?")), Err(Error::Hash));
        // Streaming gives the same digest.
        let mut h = Sha256::new();
        h.update(b"fw");
        h.update(b"!!");
        assert_eq!(<[u8; 32]>::from(h.finalize()), m.sha256);
    }

    /// The first bytes of an `espflash save-image` output for `devkit` 1.2.0 on the ESP32-C6.
    fn image_head(chip: u16, version: &str, project: &str) -> [u8; app::HEAD_LEN] {
        let mut h = [0; app::HEAD_LEN];
        h[0] = 0xe9;
        h[12..14].copy_from_slice(&chip.to_le_bytes());
        h[32..36].copy_from_slice(&0xabcd_5432u32.to_le_bytes());
        h[48..48 + version.len()].copy_from_slice(version.as_bytes());
        h[80..80 + project.len()].copy_from_slice(project.as_bytes());
        h
    }

    #[test]
    fn app_header_check() {
        let m = signed(); // devkit 1.2.0
        let c6 = app::ESP32C6;
        assert_eq!(m.check_app(&image_head(c6, "1.2.0", "devkit"), c6), Ok(()));
        assert_eq!(m.check_app(&image_head(5, "1.2.0", "devkit"), c6), Err(Error::WrongChip));
        assert_eq!(m.check_app(&image_head(c6, "1.2.1", "devkit"), c6), Err(Error::Mismatch));
        assert_eq!(m.check_app(&image_head(c6, "1.2.0", "other"), c6), Err(Error::Mismatch));
        let mut bad = image_head(c6, "1.2.0", "devkit");
        bad[0] = 0;
        assert_eq!(m.check_app(&bad, c6), Err(Error::NotAnImage));
        assert_eq!(m.check_app(&bad[..50], c6), Err(Error::NotAnImage));
    }

    #[test]
    fn rejects_bad_manifests() {
        assert_eq!(Manifest::parse(b"{}"), Err(Error::Json));
        let long = br#"{"product":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","version":"1.0.0","min_version":"0.0.0","size":1,"sha256":"","url":"","signature":""}"#;
        assert_eq!(Manifest::parse(long), Err(Error::Product));
    }
}
