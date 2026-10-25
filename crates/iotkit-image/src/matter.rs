//! Matter OTA image files (Matter Core spec 11.21.2), as `chip-ota-provider-app` serves them:
//!
//! ```text
//! magic 0x1BEEF11E (u32 LE) | total size (u64 LE) | header size (u32 LE) | header (TLV) | payload
//! ```
//!
//! The header names the vendor, product and version, and holds the payload's SHA-256. Matter
//! leaves the payload to the vendor, and checks no signature: any administrator of any fabric can
//! point the device at a provider. iotkit's payload therefore starts with its signed manifest
//! ([`bundle`]), followed by the app image; `matter-ota-unsigned` firmware takes the bare app image.

use crate::{Error, Version, app};

/// The file identifier, first in every Matter OTA file.
pub const MAGIC: u32 = 0x1bee_f11e;
/// Magic, total size and header size, before the TLV header.
pub const PREFIX_LEN: usize = 16;
/// Largest TLV header accepted: the fields below at their longest (403 bytes), with room for
/// fields of later spec versions.
pub const MAX_HEADER_LEN: usize = 512;
/// Longest `SoftwareVersionString` (spec: 1 to 64).
pub const MAX_VERSION_TEXT_LEN: usize = 64;
/// Longest `ReleaseNotesURL` (spec: 256).
pub const MAX_RELEASE_NOTES_LEN: usize = 256;
/// `ImageDigestType` for SHA-256 (IANA Named Information Hash Algorithm Registry).
const SHA256: u8 = 1;

/// The Matter `SoftwareVersion` of `v`: `major << 16 | minor << 8 | patch`, so versions compare as
/// numbers. `None` when the minor or patch is above 255.
pub const fn version(v: Version) -> Option<u32> {
    if v.minor > 0xff || v.patch > 0xff {
        return None;
    }
    Some((v.major as u32) << 16 | (v.minor as u32) << 8 | v.patch as u32)
}

/// The [`Version`] of a Matter `SoftwareVersion` written by [`version`].
pub const fn from_version(v: u32) -> Version {
    Version::new((v >> 16) as u16, (v >> 8 & 0xff) as u16, (v & 0xff) as u16)
}

/// The sizes in the fixed prefix of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefix {
    /// The whole file: prefix, header and payload.
    pub total_size: u64,
    /// The TLV header after the prefix.
    pub header_size: u32,
}

impl Prefix {
    /// Reads the first [`PREFIX_LEN`] bytes of a file.
    pub fn parse(b: &[u8; PREFIX_LEN]) -> Result<Self, Error> {
        let magic = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let mut total = [0; 8];
        total.copy_from_slice(&b[4..12]);
        let header_size = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
        if magic != MAGIC || header_size as usize > MAX_HEADER_LEN {
            return Err(Error::Matter);
        }
        Ok(Self { total_size: u64::from_le_bytes(total), header_size })
    }
}

/// The TLV header of a Matter OTA file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header<'a> {
    /// `VendorID`.
    pub vendor_id: u16,
    /// `ProductID`.
    pub product_id: u16,
    /// `SoftwareVersion` ([`version`]).
    pub version: u32,
    /// `SoftwareVersionString`.
    pub version_text: &'a str,
    /// `PayloadSize`.
    pub payload_size: u64,
    /// `MinApplicableSoftwareVersion`.
    pub min_applicable: Option<u32>,
    /// `MaxApplicableSoftwareVersion`.
    pub max_applicable: Option<u32>,
    /// `ReleaseNotesURL` (empty: none).
    pub release_notes_url: &'a str,
    /// SHA-256 of the payload (`ImageDigest`).
    pub digest: [u8; 32],
}

// TLV (Matter Core spec, appendix A): a control byte (tag form << 5 | element type), the tag,
// then the value. The header is an anonymous structure of context-tagged fields.
const ANONYMOUS: u8 = 0x00;
const CONTEXT: u8 = 0x20;
const UINT: u8 = 0x04; // to 0x07: 1, 2, 4, 8 bytes
const UTF8: u8 = 0x0c; // to 0x0f: length on 1, 2, 4, 8 bytes
const BYTES: u8 = 0x10; // to 0x13: the same
const STRUCT: u8 = 0x15;
const END: u8 = 0x18;

mod tag {
    pub const VENDOR_ID: u8 = 0;
    pub const PRODUCT_ID: u8 = 1;
    pub const VERSION: u8 = 2;
    pub const VERSION_TEXT: u8 = 3;
    pub const PAYLOAD_SIZE: u8 = 4;
    pub const MIN_APPLICABLE: u8 = 5;
    pub const MAX_APPLICABLE: u8 = 6;
    pub const RELEASE_NOTES_URL: u8 = 7;
    pub const DIGEST_TYPE: u8 = 8;
    pub const DIGEST: u8 = 9;
}

/// A TLV value of the kinds the header uses.
enum Value<'a> {
    Uint(u64),
    Bytes(&'a [u8]),
    /// Of another kind, skipped.
    Other,
}

/// Length of the tag of an element whose control byte is `control`.
const fn tag_len(control: u8) -> usize {
    [0, 1, 2, 4, 2, 4, 6, 8][(control >> 5) as usize]
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.0.len() {
            return Err(Error::Matter);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn uint(&mut self, width: u8) -> Result<u64, Error> {
        let bytes = self.take(1 << width)?;
        let mut le = [0; 8];
        le[..bytes.len()].copy_from_slice(bytes);
        Ok(u64::from_le_bytes(le))
    }

    /// The next context-tagged field, or `None` at the end of the structure.
    fn field(&mut self) -> Result<Option<(u8, Value<'a>)>, Error> {
        let control = self.take(1)?[0];
        if control == END {
            return Ok(None);
        }
        if control & 0xe0 != CONTEXT {
            return Err(Error::Matter);
        }
        let tag = self.take(1)?[0];
        let kind = control & 0x1f;
        let value = match kind {
            UINT..=0x07 => Value::Uint(self.uint(kind - UINT)?),
            UTF8..=0x0f | BYTES..=0x13 => {
                let len = self.uint((kind - UTF8) & 0x03)?;
                Value::Bytes(self.take(usize::try_from(len).map_err(|_| Error::Matter)?)?)
            }
            kind => {
                self.skip(kind)?;
                Value::Other
            }
        };
        Ok(Some((tag, value)))
    }

    /// Skips a value of element type `kind`, a container up to its end. A loop, not recursion:
    /// the nesting is the sender's to choose.
    fn skip(&mut self, mut kind: u8) -> Result<(), Error> {
        let mut depth = 0u32;
        loop {
            match kind {
                0x00..=0x07 => {
                    self.take(1 << (kind & 0x03))?;
                }
                0x08 | 0x09 | 0x14 => {}
                0x0a => {
                    self.take(4)?;
                }
                0x0b => {
                    self.take(8)?;
                }
                UTF8..=0x13 => {
                    let len = self.uint((kind - UTF8) & 0x03)?;
                    self.take(usize::try_from(len).map_err(|_| Error::Matter)?)?;
                }
                STRUCT..=0x17 => depth += 1,
                END if depth > 0 => depth -= 1,
                _ => return Err(Error::Matter),
            }
            if depth == 0 {
                return Ok(());
            }
            let control = self.take(1)?[0];
            self.take(tag_len(control))?;
            kind = control & 0x1f;
        }
    }
}

impl<'a> Header<'a> {
    /// Reads the TLV header (the [`Prefix::header_size`] bytes after the prefix).
    pub fn parse(tlv: &'a [u8]) -> Result<Self, Error> {
        let mut r = Reader(tlv);
        if r.take(1)?[0] != (ANONYMOUS | STRUCT) {
            return Err(Error::Matter);
        }
        let (mut vendor_id, mut product_id, mut version, mut payload_size) = (None, None, None, None);
        let (mut version_text, mut digest_type, mut digest) = (None, None, None);
        let mut h = Header {
            vendor_id: 0,
            product_id: 0,
            version: 0,
            version_text: "",
            payload_size: 0,
            min_applicable: None,
            max_applicable: None,
            release_notes_url: "",
            digest: [0; 32],
        };
        let text = |b: &'a [u8], max: usize| match core::str::from_utf8(b) {
            Ok(s) if s.len() <= max => Ok(s),
            _ => Err(Error::Matter),
        };
        while let Some((tag, value)) = r.field()? {
            match (tag, value) {
                (tag::VENDOR_ID, Value::Uint(v)) => vendor_id = u16::try_from(v).ok(),
                (tag::PRODUCT_ID, Value::Uint(v)) => product_id = u16::try_from(v).ok(),
                (tag::VERSION, Value::Uint(v)) => version = u32::try_from(v).ok(),
                (tag::VERSION_TEXT, Value::Bytes(b)) => version_text = Some(text(b, MAX_VERSION_TEXT_LEN)?),
                (tag::PAYLOAD_SIZE, Value::Uint(v)) => payload_size = Some(v),
                (tag::MIN_APPLICABLE, Value::Uint(v)) => {
                    h.min_applicable = Some(u32::try_from(v).map_err(|_| Error::Matter)?)
                }
                (tag::MAX_APPLICABLE, Value::Uint(v)) => {
                    h.max_applicable = Some(u32::try_from(v).map_err(|_| Error::Matter)?)
                }
                (tag::RELEASE_NOTES_URL, Value::Bytes(b)) => h.release_notes_url = text(b, MAX_RELEASE_NOTES_LEN)?,
                (tag::DIGEST_TYPE, Value::Uint(v)) => digest_type = Some(v),
                (tag::DIGEST, Value::Bytes(b)) => digest = <[u8; 32]>::try_from(b).ok(),
                // Fields of later spec versions.
                _ => {}
            }
        }
        h.vendor_id = vendor_id.ok_or(Error::Matter)?;
        h.product_id = product_id.ok_or(Error::Matter)?;
        h.version = version.ok_or(Error::Matter)?;
        h.version_text = version_text.filter(|t| !t.is_empty()).ok_or(Error::Matter)?;
        h.payload_size = payload_size.ok_or(Error::Matter)?;
        if digest_type != Some(SHA256.into()) {
            return Err(Error::Matter);
        }
        h.digest = digest.ok_or(Error::Matter)?;
        Ok(h)
    }

    /// Checks that the sizes in `prefix` match this header.
    pub fn check_prefix(&self, prefix: Prefix) -> Result<(), Error> {
        let expected = (PREFIX_LEN as u64 + u64::from(prefix.header_size)).checked_add(self.payload_size);
        if expected == Some(prefix.total_size) { Ok(()) } else { Err(Error::Size) }
    }

    /// Checks that a device of `vendor_id`/`product_id` running Matter version `running` may
    /// install this file (no downgrades, within the applicable range).
    pub fn check_update(&self, vendor_id: u16, product_id: u16, running: u32) -> Result<(), Error> {
        if (self.vendor_id, self.product_id) != (vendor_id, product_id) {
            Err(Error::WrongProduct)
        } else if self.version <= running {
            Err(Error::NotNewer)
        } else if self.min_applicable.is_some_and(|min| running < min)
            || self.max_applicable.is_some_and(|max| running > max)
        {
            Err(Error::TooOld)
        } else {
            Ok(())
        }
    }

    /// Checks the start of a bare app image (`matter-ota-unsigned`) against this header: an app
    /// for `chip_id` named `product`, whose version is this header's.
    pub fn check_app(&self, head: &[u8], chip_id: u16, product: &str) -> Result<(), Error> {
        let app = app::AppInfo::parse(head)?;
        if app.chip_id != chip_id {
            return Err(Error::WrongChip);
        }
        let app_version = Version::parse(app.version).ok().and_then(version);
        if app.project != product || app_version != Some(self.version) {
            return Err(Error::Mismatch);
        }
        Ok(())
    }

    /// Writes the prefix and the TLV header into `out`; returns the length. The payload follows.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if self.version_text.is_empty()
            || self.version_text.len() > MAX_VERSION_TEXT_LEN
            || self.release_notes_url.len() > MAX_RELEASE_NOTES_LEN
        {
            return Err(Error::Matter);
        }
        let mut w = Writer { out, at: PREFIX_LEN };
        w.put(&[ANONYMOUS | STRUCT])?;
        w.uint(tag::VENDOR_ID, self.vendor_id.into())?;
        w.uint(tag::PRODUCT_ID, self.product_id.into())?;
        w.uint(tag::VERSION, self.version.into())?;
        w.bytes(UTF8, tag::VERSION_TEXT, self.version_text.as_bytes())?;
        w.uint(tag::PAYLOAD_SIZE, self.payload_size)?;
        if let Some(min) = self.min_applicable {
            w.uint(tag::MIN_APPLICABLE, min.into())?;
        }
        if let Some(max) = self.max_applicable {
            w.uint(tag::MAX_APPLICABLE, max.into())?;
        }
        if !self.release_notes_url.is_empty() {
            w.bytes(UTF8, tag::RELEASE_NOTES_URL, self.release_notes_url.as_bytes())?;
        }
        w.uint(tag::DIGEST_TYPE, SHA256.into())?;
        w.bytes(BYTES, tag::DIGEST, &self.digest)?;
        w.put(&[END])?;
        let len = w.at;
        let header_size = (len - PREFIX_LEN) as u32;
        let total = len as u64 + self.payload_size;
        w.out[..4].copy_from_slice(&MAGIC.to_le_bytes());
        w.out[4..12].copy_from_slice(&total.to_le_bytes());
        w.out[12..16].copy_from_slice(&header_size.to_le_bytes());
        Ok(len)
    }
}

struct Writer<'b> {
    out: &'b mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn put(&mut self, b: &[u8]) -> Result<(), Error> {
        let end = self.at + b.len();
        self.out.get_mut(self.at..end).ok_or(Error::Matter)?.copy_from_slice(b);
        self.at = end;
        Ok(())
    }

    /// The smallest encoding, as CHIP's `TLVWriter` does.
    fn uint(&mut self, tag: u8, v: u64) -> Result<(), Error> {
        let width: u8 = match v {
            0..=0xff => 0,
            0x100..=0xffff => 1,
            0x1_0000..=0xffff_ffff => 2,
            _ => 3,
        };
        self.put(&[CONTEXT | (UINT + width), tag])?;
        self.put(&v.to_le_bytes()[..1 << width])
    }

    fn bytes(&mut self, kind: u8, tag: u8, b: &[u8]) -> Result<(), Error> {
        // Header strings are short: a 1- or 2-byte length.
        if let Ok(len) = u8::try_from(b.len()) {
            self.put(&[CONTEXT | kind, tag, len])?;
        } else {
            let len = u16::try_from(b.len()).map_err(|_| Error::Matter)?;
            self.put(&[CONTEXT | (kind + 1), tag])?;
            self.put(&len.to_le_bytes())?;
        }
        self.put(b)
    }
}

/// The start of iotkit's payload: a signed manifest before the app image.
///
/// ```text
/// "IOTK" | manifest length (u16 LE) | manifest JSON | app image
/// ```
///
/// The manifest comes first so the device checks it before erasing anything. Its `size` and
/// `sha256` are the app image's.
pub mod bundle {
    use crate::Error;

    /// First bytes of the payload.
    pub const MAGIC: &[u8; 4] = b"IOTK";
    /// Magic and manifest length.
    pub const PREFIX_LEN: usize = 6;
    /// Largest manifest (as the HTTP path accepts).
    pub const MAX_MANIFEST_LEN: usize = 1024;

    /// The manifest length after the [`PREFIX_LEN`] first bytes of a payload.
    pub fn manifest_len(prefix: &[u8; PREFIX_LEN]) -> Result<usize, Error> {
        if &prefix[..4] != MAGIC {
            return Err(Error::Matter);
        }
        let len = usize::from(u16::from_le_bytes([prefix[4], prefix[5]]));
        if len == 0 || len > MAX_MANIFEST_LEN {
            return Err(Error::Json);
        }
        Ok(len)
    }

    /// The prefix for a manifest of `len` bytes.
    pub fn prefix(len: usize) -> Result<[u8; PREFIX_LEN], Error> {
        if len == 0 || len > MAX_MANIFEST_LEN {
            return Err(Error::Json);
        }
        let [a, b] = (len as u16).to_le_bytes();
        Ok([MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], a, b])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header<'static> {
        Header {
            vendor_id: 0xfff1,
            product_id: 0x8001,
            version: 0x0001_0200,
            version_text: "1.2.0",
            payload_size: 1234,
            min_applicable: None,
            max_applicable: None,
            release_notes_url: "",
            digest: [0xab; 32],
        }
    }

    fn parse(file: &[u8]) -> Result<(Prefix, Header<'_>), Error> {
        let prefix = Prefix::parse(file[..PREFIX_LEN].try_into().unwrap())?;
        let end = PREFIX_LEN + prefix.header_size as usize;
        let h = Header::parse(file.get(PREFIX_LEN..end).ok_or(Error::Matter)?)?;
        h.check_prefix(prefix)?;
        Ok((prefix, h))
    }

    #[test]
    fn versions() {
        assert_eq!(version(Version::new(1, 2, 3)), Some(0x0001_0203));
        assert_eq!(version(Version::new(0, 1, 0)), Some(256));
        assert_eq!(version(Version::new(1, 256, 0)), None);
        assert!(version(Version::new(1, 10, 0)) > version(Version::new(1, 9, 255)));
        assert_eq!(from_version(0x0001_0203), Version::new(1, 2, 3));
    }

    #[test]
    fn round_trip() {
        let mut buf = [0; PREFIX_LEN + MAX_HEADER_LEN];
        for h in [
            header(),
            Header {
                min_applicable: Some(0x100),
                max_applicable: Some(0x1_0100),
                release_notes_url: "https://example.com/1.2.0",
                payload_size: 5_000_000_000,
                ..header()
            },
        ] {
            let n = h.encode(&mut buf).unwrap();
            let (prefix, parsed) = parse(&buf[..n]).unwrap();
            assert_eq!(parsed, h);
            assert_eq!(prefix.total_size, n as u64 + h.payload_size);
        }
    }

    /// The header of `ota_image_tool.py create -v 0xFFF1 -p 0x8001 -vn 2 -vs 2.0 -da sha256` for
    /// the payload `fw!!`, as CHIP's `chip.tlv.TLVWriter` encodes it.
    const CHIP_HEADER: &str = "152500f1ff250101802402022c0303322e302404042408013009204d94009828c8267b381ec706e8c70773e2a697db4a4cd08c1fe8046b601f1a1818";

    #[test]
    fn reads_chip_encoding() {
        let mut tlv = [0; 60];
        for (b, hex) in tlv.iter_mut().zip(CHIP_HEADER.as_bytes().chunks(2)) {
            *b = u8::from_str_radix(core::str::from_utf8(hex).unwrap(), 16).unwrap();
        }
        let h = Header::parse(&tlv).unwrap();
        assert_eq!((h.vendor_id, h.product_id, h.version, h.version_text), (0xfff1, 0x8001, 2, "2.0"));
        assert_eq!((h.payload_size, h.digest), (4, crate::sha256(b"fw!!")));
        let mut buf = [0; PREFIX_LEN + MAX_HEADER_LEN];
        let n = h.encode(&mut buf).unwrap();
        assert_eq!(&buf[PREFIX_LEN..n], &tlv, "encoded as CHIP does");
    }

    /// Every field at its longest still fits.
    #[test]
    fn longest_header() {
        let notes = [b'u'; MAX_RELEASE_NOTES_LEN];
        let text = [b'v'; MAX_VERSION_TEXT_LEN];
        let h = Header {
            version: u32::MAX,
            version_text: core::str::from_utf8(&text).unwrap(),
            payload_size: u64::MAX / 2,
            min_applicable: Some(u32::MAX),
            max_applicable: Some(u32::MAX),
            release_notes_url: core::str::from_utf8(&notes).unwrap(),
            ..header()
        };
        let mut buf = [0; PREFIX_LEN + MAX_HEADER_LEN];
        let n = h.encode(&mut buf).unwrap();
        assert_eq!(n - PREFIX_LEN, 403);
        assert_eq!(parse(&buf[..n]).unwrap().1, h);
    }

    /// Fields of later spec versions are skipped, whatever their kind.
    #[test]
    fn skips_unknown_fields() {
        let mut buf = [0; PREFIX_LEN + MAX_HEADER_LEN];
        let n = header().encode(&mut buf).unwrap();
        // Before the closing END: a signed int, a bool, a null, a float, and a structure holding
        // an array and a fully qualified tag.
        let extra: &[u8] = &[
            0x20, 20, 0xff, //
            0x29, 21, //
            0x34, 22, //
            0x2a, 23, 0, 0, 0x80, 0x3f, //
            0x35, 24, 0x36, 0, 0x04, 1, 0x18, 0xc8, 1, 2, 3, 4, 5, 6, 0x00, 7, 0x18,
        ];
        let len = n + extra.len();
        buf[n - 1..len - 1].copy_from_slice(extra);
        buf[len - 1] = END;
        buf[12..16].copy_from_slice(&((len - PREFIX_LEN) as u32).to_le_bytes());
        buf[4..12].copy_from_slice(&(len as u64 + header().payload_size).to_le_bytes());
        assert_eq!(parse(&buf[..len]).unwrap().1, header());
        // An unterminated structure.
        assert_eq!(Header::parse(&buf[PREFIX_LEN..len - 2]), Err(Error::Matter));
    }

    #[test]
    fn rejects_bad_files() {
        let mut buf = [0; PREFIX_LEN + MAX_HEADER_LEN];
        let n = header().encode(&mut buf).unwrap();
        let mut bad = buf;
        bad[0] ^= 1;
        assert_eq!(parse(&bad[..n]), Err(Error::Matter), "magic");
        let mut bad = buf;
        bad[4] ^= 1;
        assert_eq!(parse(&bad[..n]), Err(Error::Size), "total size");
        for cut in PREFIX_LEN + 1..n - 1 {
            let mut short = buf;
            short[12..16].copy_from_slice(&((cut - PREFIX_LEN) as u32).to_le_bytes());
            assert!(parse(&short[..cut]).is_err(), "truncated at {cut}");
        }
        assert_eq!(Header { version_text: "", ..header() }.encode(&mut buf), Err(Error::Matter));
    }

    #[test]
    fn update_policy() {
        let h = header(); // 1.2.0
        assert_eq!(h.check_update(0xfff1, 0x8001, 0x0001_0105), Ok(()));
        assert_eq!(h.check_update(0xfff1, 0x8002, 0x0001_0105), Err(Error::WrongProduct));
        assert_eq!(h.check_update(0xfff1, 0x8001, 0x0001_0200), Err(Error::NotNewer));
        let h = Header { min_applicable: Some(0x0001_0000), ..h };
        assert_eq!(h.check_update(0xfff1, 0x8001, 0x0000_0900), Err(Error::TooOld));
    }

    #[test]
    fn bare_app() {
        let mut head = [0; app::HEAD_LEN];
        head[0] = 0xe9;
        head[12..14].copy_from_slice(&app::ESP32C6.to_le_bytes());
        head[32..36].copy_from_slice(&0xabcd_5432u32.to_le_bytes());
        head[48..53].copy_from_slice(b"1.2.0");
        head[80..86].copy_from_slice(b"thread");
        let h = header();
        assert_eq!(h.check_app(&head, app::ESP32C6, "thread"), Ok(()));
        assert_eq!(h.check_app(&head, app::ESP32H2, "thread"), Err(Error::WrongChip));
        assert_eq!(h.check_app(&head, app::ESP32C6, "devkit"), Err(Error::Mismatch));
        assert_eq!(Header { version: 0x0001_0201, ..h }.check_app(&head, app::ESP32C6, "thread"), Err(Error::Mismatch));
    }

    #[test]
    fn bundle_prefix() {
        let p = bundle::prefix(300).unwrap();
        assert_eq!(bundle::manifest_len(&p), Ok(300));
        assert_eq!(bundle::manifest_len(b"\xe9\0\0\0\x01\x00"), Err(Error::Matter), "a bare image");
        assert_eq!(bundle::prefix(0), Err(Error::Json));
        assert_eq!(bundle::prefix(bundle::MAX_MANIFEST_LEN + 1), Err(Error::Json));
    }
}
