//! Captive DNS: every A query gets the device's address, so a phone's connectivity check reaches
//! the portal. Other types get an empty answer at once.

const TYPE_A: u16 = 1;

/// Answers `query` into `out`; `None` for something that is not a query.
pub fn answer(query: &[u8], ip: [u8; 4], out: &mut [u8; 512]) -> Option<usize> {
    if query.len() < 12 || query[2] & 0x80 != 0 || u16::from_be_bytes([query[4], query[5]]) != 1 {
        return None;
    }
    // The one question: name, type, class.
    let mut end = 12;
    loop {
        let len = *query.get(end)? as usize;
        if len == 0 {
            end += 1;
            break;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        end += 1 + len;
    }
    let qtype = u16::from_be_bytes([*query.get(end)?, *query.get(end + 1)?]);
    let question_end = end + 4;
    let question = query.get(12..question_end)?;

    out[0..2].copy_from_slice(&query[0..2]);
    out[2] = 0x84 | (query[2] & 0x01); // response, authoritative, RD copied
    out[3] = 0x80; // RA
    out[4..12].copy_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
    let mut n = 12;
    out.get_mut(n..n + question.len())?.copy_from_slice(question);
    n += question.len();
    if qtype == TYPE_A {
        out[7] = 1;
        let answer = [0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, ip[0], ip[1], ip[2], ip[3]];
        out.get_mut(n..n + answer.len())?.copy_from_slice(&answer);
        n += answer.len();
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &[&str], qtype: u16) -> heapless::Vec<u8, 128> {
        let mut q = heapless::Vec::<u8, 128>::new();
        q.extend_from_slice(&[0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        for l in name {
            q.push(l.len() as u8).unwrap();
            q.extend_from_slice(l.as_bytes()).unwrap();
        }
        q.push(0).unwrap();
        q.extend_from_slice(&qtype.to_be_bytes()).unwrap();
        q.extend_from_slice(&[0, 1]).unwrap();
        q
    }

    #[test]
    fn answers_everything_with_our_address() {
        let q = query(&["connectivitycheck", "gstatic", "com"], TYPE_A);
        let mut out = [0; 512];
        let n = answer(&q, [192, 168, 4, 1], &mut out).unwrap();
        assert_eq!(out[0..2], [0x12, 0x34]);
        assert_eq!(out[2] & 0x80, 0x80);
        assert_eq!(out[6..8], [0, 1]);
        assert_eq!(&out[12..q.len()], &q[12..]);
        assert_eq!(&out[n - 4..n], &[192, 168, 4, 1]);
    }

    #[test]
    fn empty_answer_for_other_types_and_none_for_junk() {
        let q = query(&["example", "com"], 28);
        let mut out = [0; 512];
        let n = answer(&q, [192, 168, 4, 1], &mut out).unwrap();
        assert_eq!((&out[6..8], n), (&[0u8, 0][..], q.len()));
        let mut response = query(&["x"], TYPE_A);
        response[2] = 0x81;
        assert_eq!(answer(&response, [0; 4], &mut out), None);
        assert_eq!(answer(&[0; 4], [0; 4], &mut out), None);
    }
}
