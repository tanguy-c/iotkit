//! Minimal mDNS responder: answers A queries for `<hostname>.local` (RFC 6762), enough for
//! `ping iotkit-xxxx.local` and `cargo xtask ota push --host iotkit-xxxx.local`; and a
//! [`Service`] for discovery (DNS-SD), as the ESPHome API's.

/// mDNS multicast group and port.
pub const GROUP: [u8; 4] = [224, 0, 0, 251];
/// The mDNS port.
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
    let mut w = Writer { out, len: 0, ttl: TTL_S };
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

/// A DNS-SD service (RFC 6763) answered next to the host name: `<host>.<name>._tcp.local`.
pub struct Service {
    /// `_esphomelib`.
    pub name: &'static str,
    /// Its TCP port.
    pub port: u16,
    /// Its TXT record, encoded: length-prefixed `key=value` strings.
    txt: heapless::Vec<u8, 192>,
}

const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;

impl Service {
    /// `None` if the TXT entries do not fit.
    pub fn new(name: &'static str, port: u16, txt: &[&str]) -> Option<Self> {
        let mut record = heapless::Vec::new();
        for entry in txt {
            record.push(u8::try_from(entry.len()).ok()?).ok()?;
            record.extend_from_slice(entry.as_bytes()).ok()?;
        }
        Some(Self { name, port, txt: record })
    }

    /// If `query` asks for the service, or this device's instance of it, writes the answer
    /// (PTR, SRV, TXT, and the address) into `out` and returns its length.
    pub fn answer(&self, query: &[u8], host: &str, ip: [u8; 4], out: &mut [u8]) -> Option<usize> {
        let asked = questions(query, |msg, pos| {
            let (browse, next) = name_eq(msg, pos, &[self.name.as_bytes(), b"_tcp", b"local"])?;
            let (instance, _) = name_eq(msg, pos, &[host.as_bytes(), self.name.as_bytes(), b"_tcp", b"local"])?;
            let qtype = u16::from_be_bytes([*msg.get(next)?, *msg.get(next + 1)?]);
            let matches = (browse && matches!(qtype, TYPE_PTR | TYPE_ANY))
                || (instance && matches!(qtype, TYPE_SRV | TYPE_TXT | TYPE_ANY));
            Some((matches, next))
        })?;
        if asked { self.announcement(host, ip, out) } else { None }
    }

    /// An unsolicited response with the service, to send when the address is set.
    pub fn announcement(&self, host: &str, ip: [u8; 4], out: &mut [u8]) -> Option<usize> {
        self.records(host, ip, TTL_S, out)
    }

    /// The same with a time to live of 0 (RFC 6762 §10.1), before going away: resolvers forget
    /// the service, so the next announcement is new to them (Home Assistant then reconnects at
    /// once instead of at its next retry).
    pub fn goodbye(&self, host: &str, ip: [u8; 4], out: &mut [u8]) -> Option<usize> {
        self.records(host, ip, 0, out)
    }

    fn records(&self, host: &str, ip: [u8; 4], ttl: u32, out: &mut [u8]) -> Option<usize> {
        let service = [self.name, "_tcp", "local"];
        let instance = [host, self.name, "_tcp", "local"];
        let target = [host, "local"];
        let mut w = Writer { out, len: 0, ttl };
        // id 0, flags: response + authoritative; 0 questions, 4 answers.
        w.put(&[0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 0])?;
        // The PTR is shared (other devices answer it too): no cache flush.
        w.record(&service, TYPE_PTR, CLASS_IN, |w| w.name(&instance))?;
        w.record(&instance, TYPE_SRV, CLASS_IN | CACHE_FLUSH, |w| {
            w.put(&[0, 0, 0, 0])?; // priority, weight
            w.put(&self.port.to_be_bytes())?;
            w.name(&target)
        })?;
        w.record(&instance, TYPE_TXT, CLASS_IN | CACHE_FLUSH, |w| w.put(&self.txt))?;
        w.record(&target, TYPE_A, CLASS_IN | CACHE_FLUSH, |w| w.put(&ip))?;
        Some(w.len)
    }
}

/// Runs `question` on each question of `query` (`None` if it is not a query), and returns
/// whether one matched. ([`answer`] keeps its own loop: the C6 build stays as it was.)
fn questions(query: &[u8], mut question: impl FnMut(&[u8], usize) -> Option<(bool, usize)>) -> Option<bool> {
    if query.len() < 12 || query[2] & 0x80 != 0 {
        return None; // too short, or a response
    }
    let mut pos = 12;
    let mut asked = false;
    for _ in 0..u16::from_be_bytes([query[4], query[5]]) {
        let (matches, next) = question(query, pos)?;
        let qclass = u16::from_be_bytes([*query.get(next + 2)?, *query.get(next + 3)?]) & !CACHE_FLUSH;
        asked |= matches && qclass == CLASS_IN;
        pos = next + 4;
    }
    Some(asked)
}

struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
    /// Of the records written.
    ttl: u32,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> Option<()> {
        self.out.get_mut(self.len..self.len + bytes.len())?.copy_from_slice(bytes);
        self.len += bytes.len();
        Some(())
    }

    fn name(&mut self, labels: &[&str]) -> Option<()> {
        for label in labels {
            if label.is_empty() || label.len() > 63 {
                return None;
            }
            self.put(&[label.len() as u8])?;
            self.put(label.as_bytes())?;
        }
        self.put(&[0])
    }

    /// A resource record, its data written by `data`.
    fn record(
        &mut self,
        name: &[&str],
        rtype: u16,
        class: u16,
        data: impl FnOnce(&mut Self) -> Option<()>,
    ) -> Option<()> {
        self.name(name)?;
        self.put(&rtype.to_be_bytes())?;
        self.put(&class.to_be_bytes())?;
        self.put(&self.ttl.to_be_bytes())?;
        let at = self.len;
        self.put(&[0, 0])?;
        data(self)?;
        let len = u16::try_from(self.len - at - 2).ok()?;
        self.out[at..at + 2].copy_from_slice(&len.to_be_bytes());
        Some(())
    }
}

/// Whether the name at `pos` is `<host>.local` (any case), and the position after it.
fn name_is(msg: &[u8], pos: usize, host: &str) -> Option<(bool, usize)> {
    name_eq(msg, pos, &[host.as_bytes(), b"local"])
}

/// Whether the name at `pos` is the `expected` labels (any case), and the position after it.
/// Follows compression pointers (at most 8, against loops).
fn name_eq(msg: &[u8], mut pos: usize, expected: &[&[u8]]) -> Option<(bool, usize)> {
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

    #[test]
    fn answers_the_service() {
        let service = Service::new("_esphomelib", 6053, &["mac=aabbccddeeff", "version=0.8"]).unwrap();
        let mut out = [0; 512];
        for (name, qtype) in [
            (&["_esphomelib", "_tcp", "local"][..], TYPE_PTR),
            (&["iotkit-aefc", "_esphomelib", "_tcp", "local"][..], TYPE_SRV),
            (&["IOTKIT-AEFC", "_esphomelib", "_tcp", "local"][..], TYPE_TXT),
        ] {
            let (q, n) = query(name, qtype, CLASS_IN);
            let len = service.answer(&q[..n], "iotkit-aefc", IP, &mut out).unwrap();
            let answer = &out[..len];
            assert_eq!(answer[7], 4, "PTR, SRV, TXT, A");
            let srv = b"\x00\x21\x80\x01\x00\x00\x00\x78\x00\x19\x00\x00\x00\x00\x17\xa5\x0biotkit-aefc\x05local\x00";
            assert!(answer.windows(srv.len()).any(|w| w == srv), "SRV: port 6053 on iotkit-aefc.local");
            assert!(answer.windows(17).any(|w| w == b"\x10mac=aabbccddeeff"));
            assert!(answer.ends_with(&IP));
        }
        for (name, qtype) in [
            (&["_esphomelib", "_tcp", "local"][..], TYPE_A),
            (&["_http", "_tcp", "local"][..], TYPE_PTR),
            (&["other", "_esphomelib", "_tcp", "local"][..], TYPE_SRV),
            (&["iotkit-aefc", "local"][..], TYPE_A),
        ] {
            let (q, n) = query(name, qtype, CLASS_IN);
            assert_eq!(service.answer(&q[..n], "iotkit-aefc", IP, &mut out), None, "{name:?} {qtype}");
        }
        assert!(Service::new("_x", 1, &[core::str::from_utf8(&[b'a'; 200]).unwrap()]).is_none(), "TXT too long");
        // Goodbye: the same records, a time to live of 0.
        let n = service.goodbye("iotkit-aefc", IP, &mut out).unwrap();
        let srv = b"\x00\x21\x80\x01\x00\x00\x00\x00\x00\x19";
        assert!(out[..n].windows(srv.len()).any(|w| w == srv), "SRV with TTL 0");
        assert_eq!(n, service.announcement("iotkit-aefc", IP, &mut [0; 512]).unwrap());
    }

    fn vec_query() -> heapless::Vec<u8, 128> {
        let mut q = heapless::Vec::<u8, 128>::new();
        q.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0]).unwrap();
        q.extend_from_slice(b"\x05other\x05local\x00\x00\x01\x00\x01").unwrap();
        q.extend_from_slice(b"\x0biotkit-aefc\xc0\x12\x00\x01\x00\x01").unwrap();
        q
    }
}
