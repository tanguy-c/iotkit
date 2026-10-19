//! A small DHCP server for the setup access point: leases from a fixed pool, with the device as
//! router and DNS server, and the portal's address as captive-portal URL (RFC 8910).

/// The device's address on its access point, and the pool after it.
pub const SERVER: [u8; 4] = [192, 168, 4, 1];
pub const POOL_START: u8 = 2;
pub const POOL_SIZE: usize = 8;
const LEASE_S: u32 = 600;
/// `http://192.168.4.1/`
pub const PORTAL_URL: &str = "http://192.168.4.1/";

const MAGIC: [u8; 4] = [99, 130, 83, 99];
const OPTIONS: usize = 240;

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;

/// Hands out addresses `192.168.4.2..` by client MAC address.
#[derive(Debug, Default)]
pub struct Leases {
    macs: [Option<[u8; 6]>; POOL_SIZE],
    /// Next slot to reuse when the pool is full.
    next: usize,
}

impl Leases {
    pub const fn new() -> Self {
        Self { macs: [None; POOL_SIZE], next: 0 }
    }

    fn address(&mut self, mac: [u8; 6]) -> [u8; 4] {
        let slot = match self.macs.iter().position(|m| *m == Some(mac)) {
            Some(i) => i,
            None => {
                let i = self.macs.iter().position(Option::is_none).unwrap_or(self.next);
                self.next = (i + 1) % POOL_SIZE;
                self.macs[i] = Some(mac);
                i
            }
        };
        [SERVER[0], SERVER[1], SERVER[2], POOL_START + slot as u8]
    }

    /// Answers a client packet (sent to the broadcast address); `None` to ignore it.
    pub fn respond(&mut self, packet: &[u8], out: &mut [u8; 576]) -> Option<usize> {
        if packet.len() < OPTIONS || packet[0] != 1 || packet[1] != 1 || packet[2] != 6 || packet[236..240] != MAGIC {
            return None;
        }
        let mac: [u8; 6] = packet[28..34].try_into().ok()?;
        let kind = option(packet, 53).and_then(|o| o.first().copied())?;
        let offered = self.address(mac);
        let reply = match kind {
            DISCOVER => OFFER,
            REQUEST => {
                let requested = option(packet, 50)
                    .and_then(|o| o.try_into().ok())
                    .or_else(|| packet[12..16].try_into().ok().filter(|ip| *ip != [0; 4]));
                if requested.is_none_or(|ip: [u8; 4]| ip == offered) { ACK } else { NAK }
            }
            _ => return None,
        };

        out.fill(0);
        out[0] = 2; // reply
        out[1..3].copy_from_slice(&[1, 6]);
        out[4..8].copy_from_slice(&packet[4..8]); // xid
        out[10] = 0x80; // broadcast
        if reply != NAK {
            out[16..20].copy_from_slice(&offered);
        }
        out[20..24].copy_from_slice(&SERVER);
        out[28..44].copy_from_slice(&packet[28..44]);
        out[236..240].copy_from_slice(&MAGIC);
        let mut w = OPTIONS;
        let mut put = |code: u8, data: &[u8]| {
            out[w] = code;
            out[w + 1] = data.len() as u8;
            out[w + 2..w + 2 + data.len()].copy_from_slice(data);
            w += 2 + data.len();
        };
        put(53, &[reply]);
        put(54, &SERVER);
        if reply != NAK {
            put(51, &LEASE_S.to_be_bytes());
            put(1, &[255, 255, 255, 0]);
            put(3, &SERVER);
            put(6, &SERVER);
            put(114, PORTAL_URL.as_bytes());
        }
        out[w] = 255;
        Some(w + 1)
    }
}

/// The data of option `code` in a client packet.
fn option(packet: &[u8], code: u8) -> Option<&[u8]> {
    let mut i = OPTIONS;
    while i < packet.len() {
        match packet[i] {
            0 => i += 1,
            255 => return None,
            c => {
                let len = *packet.get(i + 1)? as usize;
                let data = packet.get(i + 2..i + 2 + len)?;
                if c == code {
                    return Some(data);
                }
                i += 2 + len;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(kind: u8, mac: [u8; 6], requested: Option<[u8; 4]>) -> heapless::Vec<u8, 300> {
        let mut p = heapless::Vec::<u8, 300>::new();
        p.resize(OPTIONS, 0).unwrap();
        p[0..3].copy_from_slice(&[1, 1, 6]);
        p[4..8].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        p[28..34].copy_from_slice(&mac);
        p[236..240].copy_from_slice(&MAGIC);
        p.extend_from_slice(&[53, 1, kind]).unwrap();
        if let Some(ip) = requested {
            p.extend_from_slice(&[50, 4]).unwrap();
            p.extend_from_slice(&ip).unwrap();
        }
        p.push(255).unwrap();
        p
    }

    const A: [u8; 6] = [2, 0, 0, 0, 0, 0xa];
    const B: [u8; 6] = [2, 0, 0, 0, 0, 0xb];

    #[test]
    fn discover_request_ack() {
        let mut leases = Leases::new();
        let mut out = [0; 576];
        let n = leases.respond(&client(DISCOVER, A, None), &mut out).unwrap();
        let reply = &out[..n];
        assert_eq!(reply[0], 2);
        assert_eq!(reply[4..8], [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(reply[16..20], [192, 168, 4, 2]);
        assert_eq!(option(reply, 53), Some(&[OFFER][..]));
        assert_eq!(option(reply, 3), Some(&SERVER[..]));
        assert_eq!(option(reply, 6), Some(&SERVER[..]));
        assert_eq!(option(reply, 114), Some(PORTAL_URL.as_bytes()));

        let n = leases.respond(&client(REQUEST, A, Some([192, 168, 4, 2])), &mut out).unwrap();
        assert_eq!(option(&out[..n], 53), Some(&[ACK][..]));
        // Same client, same address; another client, the next one.
        let n = leases.respond(&client(DISCOVER, B, None), &mut out).unwrap();
        assert_eq!(out[16..20], [192, 168, 4, 3]);
        assert!(n > OPTIONS);
        // Asking for someone else's address: NAK.
        let n = leases.respond(&client(REQUEST, B, Some([192, 168, 4, 2])), &mut out).unwrap();
        assert_eq!(option(&out[..n], 53), Some(&[NAK][..]));
    }

    #[test]
    fn pool_wraps_and_ignores_garbage() {
        let mut leases = Leases::new();
        let mut out = [0; 576];
        for i in 0..POOL_SIZE as u8 + 1 {
            leases.respond(&client(DISCOVER, [2, 0, 0, 0, 1, i], None), &mut out).unwrap();
        }
        // The 9th client took the oldest slot.
        assert_eq!(out[16..20], [192, 168, 4, POOL_START]);
        assert_eq!(leases.respond(&[0; 100], &mut out), None);
        let mut reply = client(DISCOVER, A, None);
        reply[0] = 2;
        assert_eq!(leases.respond(&reply, &mut out), None);
    }
}
