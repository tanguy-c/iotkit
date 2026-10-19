//! Minimal mDNS responder: answers A queries for `<hostname>.local` (RFC 6762), enough for
//! `ping iotkit-xxxx.local` and `cargo xtask ota push --host iotkit-xxxx.local`.

/// mDNS multicast group and port.
pub const GROUP: [u8; 4] = [224, 0, 0, 251];
pub const PORT: u16 = 5353;
/// Seconds a resolver may cache the answer.
const TTL_S: u32 = 120;

const TYPE_A: u16 = 1;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
/// "Cache flush": this answer replaces any cached one (unique record).
const CACHE_FLUSH: u16 = 0x8000;

/// If `query` asks for the A record of `<host>.local`, writes the answer into `out` and returns
/// its length.
pub fn answer(query: &[u8], host: &str, ip: [u8; 4], out: &mut [u8]) -> Option<usize> {
    if query.len() < 12 || query[2] & 0x80 != 0 {
        return None; // too short, or a response
    }
    let questions = u16::from_be_bytes([query[4], query[5]]);
    let mut pos = 12;
    let mut asked = false;
    for _ in 0..questions {
        let (matches, next) = name_is(query, pos, host)?;
        let qtype = u16::from_be_bytes([*query.get(next)?, *query.get(next + 1)?]);
        let qclass = u16::from_be_bytes([*query.get(next + 2)?, *query.get(next + 3)?]) & !CACHE_FLUSH;
        asked |= matches && matches!(qtype, TYPE_A | TYPE_ANY) && qclass == CLASS_IN;
        pos = next + 4;
    }
    if asked { announcement(host, ip, out) } else { None }
}

/// An unsolicited response with our address, to send when the IP address is set.
pub fn announcement(host: &str, ip: [u8; 4], out: &mut [u8]) -> Option<usize> {
    let mut w = Writer { out, len: 0 };
    // id 0, flags: response + authoritative; 0 questions, 1 answer.
    w.put(&[0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0])?;
    for label in [host, "local"] {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        w.put(&[label.len() as u8])?;
        w.put(label.as_bytes())?;
    }
    w.put(&[0])?;
    w.put(&TYPE_A.to_be_bytes())?;
    w.put(&(CLASS_IN | CACHE_FLUSH).to_be_bytes())?;
    w.put(&TTL_S.to_be_bytes())?;
    w.put(&4u16.to_be_bytes())?;
    w.put(&ip)?;
    Some(w.len)
}

struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> Option<()> {
        self.out.get_mut(self.len..self.len + bytes.len())?.copy_from_slice(bytes);
        self.len += bytes.len();
        Some(())
    }
}

/// Whether the name at `pos` is `<host>.local` (any case), and the position after it.
/// Follows compression pointers (at most 8, against loops).
fn name_is(msg: &[u8], mut pos: usize, host: &str) -> Option<(bool, usize)> {
    let expected = [host.as_bytes(), b"local"];
    let mut label = 0;
    let mut matches = true;
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *msg.get(pos)? as usize;
        if len & 0xc0 == 0xc0 {
            let target = (len & 0x3f) << 8 | *msg.get(pos + 1)? as usize;
            end.get_or_insert(pos + 2);
            jumps += 1;
            if jumps > 8 {
                return None;
            }
            pos = target;
            continue;
        }
        if len == 0 {
            let next = end.unwrap_or(pos + 1);
            return Some((matches && label == expected.len(), next));
        }
        let text = msg.get(pos + 1..pos + 1 + len)?;
        matches &= expected.get(label).is_some_and(|e| e.eq_ignore_ascii_case(text));
        label += 1;
        pos += 1 + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A query as avahi sends it: one question, `<name>` A IN, QU bit optional.
    fn query(name: &[&str], qtype: u16, class: u16) -> ([u8; 128], usize) {
        let mut q = [0; 128];
        q[5] = 1; // one question
        let mut n = 12;
        for label in name {
            q[n] = label.len() as u8;
            q[n + 1..n + 1 + label.len()].copy_from_slice(label.as_bytes());
            n += 1 + label.len();
        }
        n += 1;
        q[n..n + 2].copy_from_slice(&qtype.to_be_bytes());
        q[n + 2..n + 4].copy_from_slice(&class.to_be_bytes());
        (q, n + 4)
    }

    const IP: [u8; 4] = [10, 152, 66, 42];

    #[test]
    fn answers_our_name() {
        let (q, n) = query(&["iotkit-aefc", "local"], TYPE_A, CLASS_IN);
        let mut out = [0; 128];
        let len = answer(&q[..n], "iotkit-aefc", IP, &mut out).unwrap();
        let expected: &[u8] = &[
            0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, // header
            11, b'i', b'o', b't', b'k', b'i', b't', b'-', b'a', b'e', b'f', b'c', 5, b'l', b'o', b'c', b'a', b'l', 0,
            0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 10, 152, 66, 42,
        ];
        assert_eq!(&out[..len], expected);
        // Case-insensitive, ANY, unicast-response bit.
        let (q, n) = query(&["IOTKIT-AEFC", "Local"], TYPE_ANY, CLASS_IN | 0x8000);
        assert!(answer(&q[..n], "iotkit-aefc", IP, &mut out).is_some());
    }

    #[test]
    fn ignores_other_queries() {
        let mut out = [0; 128];
        for (name, qtype) in [
            (&["other", "local"][..], TYPE_A),
            (&["iotkit-aefc", "local"][..], 28), // AAAA
            (&["iotkit-aefc", "lan"][..], TYPE_A),
            (&["iotkit-aefc"][..], TYPE_A),
            (&["iotkit-aefc", "local", "x"][..], TYPE_A),
        ] {
            let (q, n) = query(name, qtype, CLASS_IN);
            assert_eq!(answer(&q[..n], "iotkit-aefc", IP, &mut out), None, "{name:?} {qtype}");
        }
        // A response, and garbage.
        let (mut q, n) = query(&["iotkit-aefc", "local"], TYPE_A, CLASS_IN);
        q[2] = 0x84;
        assert_eq!(answer(&q[..n], "iotkit-aefc", IP, &mut out), None);
        assert_eq!(answer(&[0; 5], "iotkit-aefc", IP, &mut out), None);
        assert_eq!(answer(&q[..20], "iotkit-aefc", IP, &mut out), None);
    }

    #[test]
    fn follows_compression_and_stops_loops() {
        // Two questions: "other.local" then "iotkit-aefc" + pointer to "local" (offset 18).
        let mut q = vec_query();
        assert!(answer(&q, "iotkit-aefc", IP, &mut [0; 128]).is_some());
        // A pointer to itself must not hang.
        let at = q.len() - 6;
        q[at] = 0xc0;
        q[at + 1] = at as u8;
        assert_eq!(answer(&q, "iotkit-aefc", IP, &mut [0; 128]), None);
    }

    fn vec_query() -> heapless::Vec<u8, 128> {
        let mut q = heapless::Vec::<u8, 128>::new();
        q.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0]).unwrap();
        q.extend_from_slice(b"\x05other\x05local\x00\x00\x01\x00\x01").unwrap();
        q.extend_from_slice(b"\x0biotkit-aefc\xc0\x12\x00\x01\x00\x01").unwrap();
        q
    }
}
