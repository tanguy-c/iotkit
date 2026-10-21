//! The protobuf subset the ESPHome API uses: varints, `fixed32`, floats and strings, no nested
//! messages. Fields at their default value are left out, as proto3 does.

/// Writes a message into a buffer; [`finish`](Writer::finish) says whether it fit.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    len: usize,
    overflow: bool,
}

const VARINT: u8 = 0;
const LEN: u8 = 2;
const FIXED32: u8 = 5;

impl<'a> Writer<'a> {
    /// An empty message in `buf`.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0, overflow: false }
    }

    /// The message's length; `None` if it did not fit.
    pub fn finish(self) -> Option<usize> {
        (!self.overflow).then_some(self.len)
    }

    fn put(&mut self, bytes: &[u8]) {
        match self.buf.get_mut(self.len..self.len + bytes.len()) {
            Some(to) => {
                to.copy_from_slice(bytes);
                self.len += bytes.len();
            }
            None => self.overflow = true,
        }
    }

    fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.put(&[v as u8 | 0x80]);
            v >>= 7;
        }
        self.put(&[v as u8]);
    }

    fn tag(&mut self, field: u32, wire: u8) {
        self.varint(u64::from(field << 3 | u32::from(wire)));
    }

    /// A `uint32` or an enum.
    pub fn uint(&mut self, field: u32, v: u32) {
        if v != 0 {
            self.item(field, v);
        }
    }

    /// One value of a `repeated` enum: written even when 0.
    pub fn item(&mut self, field: u32, v: u32) {
        self.tag(field, VARINT);
        self.varint(u64::from(v));
    }

    /// An `int32` (negative values take 10 bytes, as protobuf has it).
    pub fn int(&mut self, field: u32, v: i32) {
        if v != 0 {
            self.tag(field, VARINT);
            self.varint(i64::from(v) as u64);
        }
    }

    /// A `bool`.
    pub fn bool(&mut self, field: u32, v: bool) {
        self.uint(field, v.into());
    }

    /// A `fixed32`: entity keys, written even when 0.
    pub fn fixed32(&mut self, field: u32, v: u32) {
        self.tag(field, FIXED32);
        self.put(&v.to_le_bytes());
    }

    /// A `float`.
    pub fn float(&mut self, field: u32, v: f32) {
        if v != 0.0 {
            self.fixed32(field, v.to_bits());
        }
    }

    /// A `string` (one value of a `repeated` one is written even when empty).
    pub fn str(&mut self, field: u32, s: &str) {
        if !s.is_empty() {
            self.text(field, s);
        }
    }

    /// One value of a `repeated string`.
    pub fn text(&mut self, field: u32, s: &str) {
        self.tag(field, LEN);
        self.varint(s.len() as u64);
        self.put(s.as_bytes());
    }
}

/// A field's value, by wire type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    /// `uint32`, `int32`, `bool`, enums.
    Varint(u64),
    /// `fixed32`, `float`.
    Fixed32(u32),
    /// `fixed64`, `double`: not used by the messages read, skipped.
    Fixed64(u64),
    /// `string`, `bytes`, nested messages.
    Bytes(&'a [u8]),
}

impl<'a> Value<'a> {
    /// As a `uint32`, `bool` or enum (`0` for another wire type).
    pub fn uint(self) -> u32 {
        match self {
            Value::Varint(v) => v as u32,
            _ => 0,
        }
    }

    /// As a `bool`.
    pub fn bool(self) -> bool {
        self.uint() != 0
    }

    /// As a `float` or `fixed32` bits.
    pub fn fixed32(self) -> u32 {
        match self {
            Value::Fixed32(v) => v,
            _ => 0,
        }
    }

    /// As a `float`.
    pub fn float(self) -> f32 {
        f32::from_bits(self.fixed32())
    }

    /// As a `string` (empty if not UTF-8).
    pub fn str(self) -> &'a str {
        match self {
            Value::Bytes(b) => core::str::from_utf8(b).unwrap_or(""),
            _ => "",
        }
    }
}

/// The fields of a message, in order. A malformed field ends it.
pub struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    /// The fields of `message`.
    pub fn new(message: &'a [u8]) -> Self {
        Self(message)
    }

    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = self.0.split_first()?;
            self.0 = rest;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(taken)
    }

    fn field(&mut self) -> Option<(u32, Value<'a>)> {
        let tag = self.varint()?;
        let field = u32::try_from(tag >> 3).ok()?;
        let value = match tag & 7 {
            0 => Value::Varint(self.varint()?),
            1 => Value::Fixed64(u64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            2 => {
                let n = usize::try_from(self.varint()?).ok()?;
                Value::Bytes(self.take(n)?)
            }
            5 => Value::Fixed32(u32::from_le_bytes(self.take(4)?.try_into().ok()?)),
            _ => return None,
        };
        Some((field, value))
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u32, Value<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let field = self.field();
        if field.is_none() {
            self.0 = &[];
        }
        field
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut buf = [0; 64];
        let mut w = Writer::new(&mut buf);
        w.fixed32(1, 0xdead_beef);
        w.bool(2, true);
        w.uint(3, 0); // left out
        w.uint(4, 300);
        w.int(5, -1);
        w.float(6, 0.5);
        w.str(7, "hé");
        w.item(8, 0);
        let n = w.finish().unwrap();
        let fields: heapless::Vec<_, 8> = Fields::new(&buf[..n]).collect();
        assert_eq!(
            fields.as_slice(),
            [
                (1, Value::Fixed32(0xdead_beef)),
                (2, Value::Varint(1)),
                (4, Value::Varint(300)),
                (5, Value::Varint(u64::MAX)),
                (6, Value::Fixed32(0.5f32.to_bits())),
                (7, Value::Bytes("hé".as_bytes())),
                (8, Value::Varint(0)),
            ]
        );
        assert_eq!(&buf[..3], [0x0d, 0xef, 0xbe], "tag 1 fixed32, little-endian");
    }

    #[test]
    fn overflow_and_malformed() {
        let mut buf = [0; 4];
        let mut w = Writer::new(&mut buf);
        w.str(1, "too long");
        assert_eq!(w.finish(), None);
        // A length past the end, an unknown wire type, an endless varint: nothing more read.
        for bad in [&[0x0a, 5, b'a'][..], &[0x0b], &[0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]]
        {
            assert_eq!(Fields::new(bad).count(), 0, "{bad:?}");
        }
    }
}
