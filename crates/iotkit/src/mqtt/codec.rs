//! MQTT 3.1.1, the subset a sensor needs: CONNECT (will, credentials), QoS 0 PUBLISH, SUBSCRIBE,
//! PING, DISCONNECT, and decoding what the broker sends.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Why a packet could not be written or read.
pub enum Error {
    /// The output buffer is too small.
    Full,
    /// A field longer than MQTT allows, or a packet larger than the buffer.
    TooLong,
    /// Not a valid packet.
    Malformed,
}

/// A ping, to keep the connection alive.
pub const PINGREQ: [u8; 2] = [0xc0, 0];
/// A clean disconnect (the broker drops the will).
pub const DISCONNECT: [u8; 2] = [0xe0, 0];

struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, b: &[u8]) -> Result<(), Error> {
        self.out.get_mut(self.len..self.len + b.len()).ok_or(Error::Full)?.copy_from_slice(b);
        self.len += b.len();
        Ok(())
    }
    fn u16(&mut self, v: u16) -> Result<(), Error> {
        self.bytes(&v.to_be_bytes())
    }
    fn string(&mut self, s: &[u8]) -> Result<(), Error> {
        self.u16(u16::try_from(s.len()).map_err(|_| Error::TooLong)?)?;
        self.bytes(s)
    }
}

/// Writes the fixed header and moves the `body_len` bytes already written at `out[5..]` right
/// after it. Returns the packet length.
fn finish(out: &mut [u8], first: u8, body_len: usize) -> Result<usize, Error> {
    if body_len > 268_435_455 {
        return Err(Error::TooLong);
    }
    let mut header = [first, 0, 0, 0, 0];
    let mut n = 1;
    let mut x = body_len;
    loop {
        let mut byte = (x % 128) as u8;
        x /= 128;
        if x > 0 {
            byte |= 0x80;
        }
        header[n] = byte;
        n += 1;
        if x == 0 {
            break;
        }
    }
    out.copy_within(5..5 + body_len, n);
    out[..n].copy_from_slice(&header[..n]);
    Ok(n + body_len)
}

/// The body is written at offset 5, leaving room for the largest fixed header.
fn body(out: &mut [u8]) -> Result<Writer<'_>, Error> {
    let out = out.get_mut(5..).ok_or(Error::Full)?;
    Ok(Writer { out, len: 0 })
}

/// What the broker publishes when the connection drops (MQTT's last will).
pub struct Will<'a> {
    /// Where.
    pub topic: &'a str,
    /// What.
    pub payload: &'a [u8],
    /// Kept by the broker for later subscribers.
    pub retain: bool,
}

/// A CONNECT with a clean session into `out`; returns the packet length.
pub fn connect(
    out: &mut [u8],
    client_id: &str,
    keep_alive_s: u16,
    will: Option<&Will>,
    user: Option<&str>,
    password: Option<&str>,
) -> Result<usize, Error> {
    let mut w = body(out)?;
    w.string(b"MQTT")?;
    let mut flags = 0x02; // clean session
    if let Some(will) = will {
        flags |= 0x04 | if will.retain { 0x20 } else { 0 };
    }
    if user.is_some() {
        flags |= 0x80;
    }
    if password.is_some() {
        flags |= 0x40;
    }
    w.bytes(&[4, flags])?;
    w.u16(keep_alive_s)?;
    w.string(client_id.as_bytes())?;
    if let Some(will) = will {
        w.string(will.topic.as_bytes())?;
        w.string(will.payload)?;
    }
    if let Some(user) = user {
        w.string(user.as_bytes())?;
    }
    if let Some(password) = password {
        w.string(password.as_bytes())?;
    }
    let len = w.len;
    finish(out, 0x10, len)
}

/// A QoS 0 publish.
pub fn publish(out: &mut [u8], topic: &str, payload: &[u8], retain: bool) -> Result<usize, Error> {
    let mut w = body(out)?;
    w.string(topic.as_bytes())?;
    w.bytes(payload)?;
    let len = w.len;
    finish(out, 0x30 | u8::from(retain), len)
}

/// A QoS 0 publish whose payload `write` formats straight into `out` (no other buffer).
pub fn publish_with(
    out: &mut [u8],
    topic: &str,
    retain: bool,
    write: impl FnOnce(&mut dyn core::fmt::Write) -> core::fmt::Result,
) -> Result<usize, Error> {
    let mut w = body(out)?;
    w.string(topic.as_bytes())?;
    let start = w.len;
    let mut payload = Payload { out: &mut w.out[start..], len: 0 };
    write(&mut payload).map_err(|_| Error::Full)?;
    let len = start + payload.len;
    finish(out, 0x30 | u8::from(retain), len)
}

/// A `fmt::Write` into a byte slice.
struct Payload<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for Payload<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let dst = self.out.get_mut(self.len..self.len + s.len()).ok_or(core::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}

/// Subscribes to `filters` at QoS 0.
pub fn subscribe(out: &mut [u8], packet_id: u16, filters: &[&str]) -> Result<usize, Error> {
    let mut w = body(out)?;
    w.u16(packet_id)?;
    for f in filters {
        w.string(f.as_bytes())?;
        w.bytes(&[0])?;
    }
    let len = w.len;
    finish(out, 0x82, len)
}

/// A packet from the broker.
#[derive(Debug, PartialEq, Eq)]
pub enum Packet<'a> {
    /// The broker's answer to CONNECT.
    ConnAck {
        /// 0 accepted, else the refusal reason.
        code: u8,
    },
    /// A message on a subscribed topic.
    Publish {
        /// Its topic.
        topic: &'a str,
        /// Its payload.
        payload: &'a [u8],
    },
    /// The broker took a SUBSCRIBE.
    SubAck,
    /// The answer to [`PINGREQ`].
    PingResp,
    /// Anything else (ignored).
    Other(u8),
}

/// Parses the packet at the start of `buf`: `Ok(None)` if it is not complete yet, else the
/// packet and its length.
pub fn parse(buf: &[u8]) -> Result<Option<(Packet<'_>, usize)>, Error> {
    let Some(&first) = buf.first() else { return Ok(None) };
    let mut len = 0usize;
    let mut i = 1;
    loop {
        let Some(&b) = buf.get(i) else { return Ok(None) };
        len |= usize::from(b & 0x7f) << (7 * (i - 1));
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
        if i > 4 {
            return Err(Error::Malformed);
        }
    }
    let total = i + len;
    let Some(body) = buf.get(i..total) else { return Ok(None) };
    let packet = match first >> 4 {
        2 => Packet::ConnAck { code: *body.get(1).ok_or(Error::Malformed)? },
        3 => {
            let qos = (first >> 1) & 3;
            let topic_len = usize::from(u16::from_be_bytes([
                *body.first().ok_or(Error::Malformed)?,
                *body.get(1).ok_or(Error::Malformed)?,
            ]));
            let topic = body.get(2..2 + topic_len).ok_or(Error::Malformed)?;
            // QoS 1/2 carry a packet id; we subscribe at QoS 0, but skip it anyway.
            let payload_at = 2 + topic_len + if qos > 0 { 2 } else { 0 };
            Packet::Publish {
                topic: core::str::from_utf8(topic).map_err(|_| Error::Malformed)?,
                payload: body.get(payload_at..).ok_or(Error::Malformed)?,
            }
        }
        9 => Packet::SubAck,
        13 => Packet::PingResp,
        other => Packet::Other(other),
    };
    Ok(Some((packet, total)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_packet() {
        let mut out = [0; 128];
        let will = Will { topic: "a/s", payload: b"offline", retain: true };
        let n = connect(&mut out, "dev", 30, Some(&will), Some("u"), Some("p")).unwrap();
        let expected: &[u8] = &[
            0x10,
            35,
            0,
            4,
            b'M',
            b'Q',
            b'T',
            b'T',
            4,
            0x02 | 0x04 | 0x20 | 0x80 | 0x40,
            0,
            30,
            0,
            3,
            b'd',
            b'e',
            b'v',
            0,
            3,
            b'a',
            b'/',
            b's',
            0,
            7,
            b'o',
            b'f',
            b'f',
            b'l',
            b'i',
            b'n',
            b'e',
            0,
            1,
            b'u',
            0,
            1,
            b'p',
        ];
        assert_eq!(&out[..n], expected);
        let n = connect(&mut out, "d", 60, None, None, None).unwrap();
        assert_eq!(&out[..n], &[0x10, 13, 0, 4, b'M', b'Q', b'T', b'T', 4, 0x02, 0, 60, 0, 1, b'd']);
    }

    #[test]
    fn publish_and_long_lengths() {
        let mut out = [0; 512];
        let n = publish(&mut out, "t", b"hi", true).unwrap();
        assert_eq!(&out[..n], &[0x31, 5, 0, 1, b't', b'h', b'i']);
        // 300 bytes of body: two length bytes.
        let payload = [7u8; 297];
        let n = publish(&mut out, "t", &payload, false).unwrap();
        assert_eq!(&out[..3], &[0x30, 0xac, 0x02]);
        assert_eq!(n, 3 + 300);
        let (p, len) = parse(&out[..n]).unwrap().unwrap();
        assert_eq!((p, len), (Packet::Publish { topic: "t", payload: &payload }, n));
        assert_eq!(publish(&mut [0; 8], "topic", b"payload", false), Err(Error::Full));
        // The same packet, formatted in place.
        let mut other = [0; 512];
        let m = publish_with(&mut other, "t", false, |w| w.write_str(core::str::from_utf8(&payload).unwrap())).unwrap();
        assert_eq!(&other[..m], &out[..n]);
        assert_eq!(publish_with(&mut [0; 12], "t", true, |w| w.write_str("too long")), Err(Error::Full));
    }

    #[test]
    fn subscribe_packet() {
        let mut out = [0; 64];
        let n = subscribe(&mut out, 1, &["a/+/set", "hs"]).unwrap();
        assert_eq!(
            &out[..n],
            &[0x82, 17, 0, 1, 0, 7, b'a', b'/', b'+', b'/', b's', b'e', b't', 0, 0, 2, b'h', b's', 0]
        );
    }

    #[test]
    fn parses_broker_packets() {
        assert_eq!(parse(&[0x20, 2, 0, 0]).unwrap(), Some((Packet::ConnAck { code: 0 }, 4)));
        assert_eq!(parse(&[0x20, 2, 0, 5]).unwrap(), Some((Packet::ConnAck { code: 5 }, 4)));
        assert_eq!(parse(&[0x90, 3, 0, 1, 0]).unwrap(), Some((Packet::SubAck, 5)));
        assert_eq!(parse(&[0xd0, 0]).unwrap(), Some((Packet::PingResp, 2)));
        assert_eq!(parse(&[0x30, 5, 0, 1, b't']).unwrap(), None, "incomplete");
        assert_eq!(parse(&[0x30]).unwrap(), None);
        // QoS 1 publish: packet id skipped.
        assert_eq!(
            parse(&[0x32, 7, 0, 1, b't', 0, 9, b'o', b'k']).unwrap(),
            Some((Packet::Publish { topic: "t", payload: b"ok" }, 9))
        );
        assert_eq!(parse(&[0x30, 0xff, 0xff, 0xff, 0xff, 1]), Err(Error::Malformed));
        // Two packets in a row.
        let two = [0xd0, 0, 0x20, 2, 0, 0];
        let (_, n) = parse(&two).unwrap().unwrap();
        assert_eq!(parse(&two[n..]).unwrap(), Some((Packet::ConnAck { code: 0 }, 4)));
    }
}
