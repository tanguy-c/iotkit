//! [Improv Wi-Fi](https://www.improv-wifi.com): giving a device its Wi-Fi network over Bluetooth
//! LE or a serial port, from the Home Assistant app, ESP Web Tools or improv-wifi.com.
//!
//! The protocol only, without allocation: RPC commands and results, serial frames, the BLE
//! advertisement.

/// GATT service and characteristics.
pub const SERVICE_UUID: u128 = 0x00467768_6228_2272_4663_277478268000;
/// The current [`State`] (read, notify).
pub const STATE_UUID: u128 = SERVICE_UUID | 0x01;
/// The last [`Error`] (read, notify).
pub const ERROR_UUID: u128 = SERVICE_UUID | 0x02;
/// Where the client writes RPC commands.
pub const RPC_COMMAND_UUID: u128 = SERVICE_UUID | 0x03;
/// Where RPC results are notified.
pub const RPC_RESULT_UUID: u128 = SERVICE_UUID | 0x04;
/// The [`capability`] bits (read).
pub const CAPABILITIES_UUID: u128 = SERVICE_UUID | 0x05;
/// 16-bit UUID of the advertised service data.
pub const SERVICE_DATA_UUID: u16 = 0x4677;

/// Capability bits (BLE).
pub mod capability {
    /// Can show where it is ([`Rpc::Identify`](super::Rpc::Identify)).
    pub const IDENTIFY: u8 = 0x01;
    /// Answers [`Rpc::DeviceInfo`](super::Rpc::DeviceInfo).
    pub const DEVICE_INFO: u8 = 0x02;
}

/// Largest RPC command or result: id, length, 255 bytes of data, checksum.
pub const RPC_MAX: usize = 258;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
/// Where provisioning is.
pub enum State {
    /// BLE: waiting for a button press before accepting credentials.
    AuthorizationRequired = 0x01,
    /// Ready for credentials (serial: "ready").
    Authorized = 0x02,
    /// Trying the received network.
    Provisioning = 0x03,
    /// Connected.
    Provisioned = 0x04,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
/// What went wrong.
pub enum Error {
    /// No error.
    None = 0x00,
    /// Malformed packet or bad checksum.
    InvalidRpc = 0x01,
    /// A command it does not know.
    UnknownRpc = 0x02,
    /// The network could not be joined.
    UnableToConnect = 0x03,
    /// Credentials before authorization (BLE).
    NotAuthorized = 0x04,
    /// Anything else.
    Unknown = 0xff,
}

/// Command ids.
pub mod command {
    /// Join a network ([`Rpc::WifiSettings`](super::Rpc::WifiSettings)).
    pub const WIFI_SETTINGS: u8 = 0x01;
    /// BLE: identify. Serial: current state.
    pub const IDENTIFY_OR_STATE: u8 = 0x02;
    /// Firmware name and version, chip, device name.
    pub const DEVICE_INFO: u8 = 0x03;
    /// List the visible networks.
    pub const SCAN_NETWORKS: u8 = 0x04;
}

/// BLE commands carry their own checksum; serial frames carry one for the whole frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Bluetooth LE.
    Ble,
    /// A serial port.
    Serial,
}

/// A command from the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rpc<'a> {
    /// Join this network.
    WifiSettings {
        /// Network name.
        ssid: &'a str,
        /// Empty for an open network.
        password: &'a str,
    },
    /// BLE: show where the device is.
    Identify,
    /// Serial: report the current state.
    GetState,
    /// Firmware name and version, chip, device name.
    DeviceInfo,
}

/// Low byte of the sum of `bytes`.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
}

/// Parses a command: id, length, data, then (BLE only) the checksum.
pub fn parse_rpc(transport: Transport, packet: &[u8]) -> Result<Rpc<'_>, Error> {
    let body = match transport {
        Transport::Ble => {
            let (&sum, body) = packet.split_last().ok_or(Error::InvalidRpc)?;
            if checksum(body) != sum {
                return Err(Error::InvalidRpc);
            }
            body
        }
        Transport::Serial => packet,
    };
    let [id, len, data @ ..] = body else { return Err(Error::InvalidRpc) };
    if data.len() != *len as usize {
        return Err(Error::InvalidRpc);
    }
    match *id {
        command::WIFI_SETTINGS => {
            let (ssid, rest) = string(data)?;
            let (password, rest) = string(rest)?;
            if !rest.is_empty() || ssid.is_empty() {
                return Err(Error::InvalidRpc);
            }
            Ok(Rpc::WifiSettings { ssid: utf8(ssid)?, password: utf8(password)? })
        }
        command::IDENTIFY_OR_STATE if transport == Transport::Ble => Ok(Rpc::Identify),
        command::IDENTIFY_OR_STATE => Ok(Rpc::GetState),
        command::DEVICE_INFO => Ok(Rpc::DeviceInfo),
        _ => Err(Error::UnknownRpc),
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, Error> {
    core::str::from_utf8(bytes).map_err(|_| Error::InvalidRpc)
}

/// Splits a length-prefixed string off `data`.
fn string(data: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let (&len, rest) = data.split_first().ok_or(Error::InvalidRpc)?;
    if rest.len() < len as usize {
        return Err(Error::InvalidRpc);
    }
    Ok(rest.split_at(len as usize))
}

/// An RPC result: command id, length, each string with its length, then (BLE only) the
/// checksum. Strings that do not fit in 255 bytes of data are left out.
pub fn rpc_result<'o>(transport: Transport, id: u8, strings: &[&str], out: &'o mut [u8; RPC_MAX]) -> &'o [u8] {
    out[0] = id;
    let mut len = 2;
    for s in strings {
        let bytes = s.as_bytes();
        if len - 2 + 1 + bytes.len() > 255 {
            break;
        }
        out[len] = bytes.len() as u8;
        out[len + 1..len + 1 + bytes.len()].copy_from_slice(bytes);
        len += 1 + bytes.len();
    }
    out[1] = (len - 2) as u8;
    if transport == Transport::Ble {
        out[len] = checksum(&out[..len]);
        len += 1;
    }
    &out[..len]
}

/// Collects a BLE command written in several chunks (20 bytes each with the default MTU).
#[derive(Debug, Default)]
pub struct BleCommand {
    buf: heapless::Vec<u8, RPC_MAX>,
}

impl BleCommand {
    /// Nothing received yet.
    pub const fn new() -> Self {
        Self { buf: heapless::Vec::new() }
    }

    /// Adds a write; returns the complete command (id, length, data, checksum) when it is there.
    pub fn push(&mut self, chunk: &[u8]) -> Option<heapless::Vec<u8, RPC_MAX>> {
        if self.buf.extend_from_slice(chunk).is_err() {
            self.buf.clear();
            return None;
        }
        let len = *self.buf.get(1)? as usize + 3;
        if self.buf.len() < len {
            return None;
        }
        let mut packet = core::mem::take(&mut self.buf);
        packet.truncate(len);
        Some(packet)
    }

    /// Drops a partial command (new connection).
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

/// Advertised service data: state, capabilities, 4 reserved bytes.
pub const fn service_data(state: State, capabilities: u8) -> [u8; 6] {
    [state as u8, capabilities, 0, 0, 0, 0]
}

/// Serial frame types.
pub mod frame {
    /// The current [`State`](super::State).
    pub const STATE: u8 = 0x01;
    /// An [`Error`](super::Error).
    pub const ERROR: u8 = 0x02;
    /// A command.
    pub const RPC: u8 = 0x03;
    /// A command's result.
    pub const RPC_RESULT: u8 = 0x04;
}

/// Start of every serial frame.
pub const SERIAL_HEADER: &[u8; 6] = b"IMPROV";
/// Protocol version of the serial frames.
pub const SERIAL_VERSION: u8 = 1;
/// Largest serial frame: header, version, type, length, 255 bytes, checksum, newline.
pub const FRAME_MAX: usize = 6 + 3 + 255 + 2;

/// A serial frame: `IMPROV`, version, type, length, data, checksum of all that, newline.
pub fn serial_frame<'o>(kind: u8, data: &[u8], out: &'o mut [u8; FRAME_MAX]) -> &'o [u8] {
    let data = &data[..data.len().min(255)];
    out[..6].copy_from_slice(SERIAL_HEADER);
    out[6..9].copy_from_slice(&[SERIAL_VERSION, kind, data.len() as u8]);
    out[9..9 + data.len()].copy_from_slice(data);
    let end = 9 + data.len();
    out[end] = checksum(&out[..end]);
    out[end + 1] = b'\n';
    &out[..end + 2]
}

/// Finds serial frames among other traffic (a console) on the same port.
#[derive(Debug, Default)]
pub struct SerialDecoder {
    buf: heapless::Vec<u8, FRAME_MAX>,
    /// `buf` holds a frame already returned.
    complete: bool,
}

impl SerialDecoder {
    /// Nothing received yet.
    pub const fn new() -> Self {
        Self { buf: heapless::Vec::new(), complete: false }
    }

    /// True while the bytes so far could be the start of a frame: the caller can hold them
    /// back from its console.
    pub fn in_frame(&self) -> bool {
        !self.complete && !self.buf.is_empty()
    }

    /// Adds a byte; returns a complete frame as `Ok((type, data))`, or `Err` for a bad checksum.
    pub fn push(&mut self, byte: u8) -> Option<Result<(u8, &[u8]), Error>> {
        if self.complete {
            self.buf.clear();
            self.complete = false;
        }
        let at = self.buf.len();
        let expected = match at {
            0..6 => Some(SERIAL_HEADER[at]),
            6 => Some(SERIAL_VERSION),
            _ => None,
        };
        if expected.is_some_and(|b| b != byte) {
            self.buf.clear();
            if byte == SERIAL_HEADER[0] {
                let _ = self.buf.push(byte);
            }
            return None;
        }
        let _ = self.buf.push(byte);
        if at < 9 || at < 9 + self.buf[8] as usize {
            return None;
        }
        self.complete = true;
        let (&sum, body) = self.buf.split_last()?;
        Some(if checksum(body) == sum { Ok((body[7], &body[9..])) } else { Err(Error::InvalidRpc) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "MyWiFi" / "pass", from the protocol description.
    const BLE_WIFI: [u8; 15] = [1, 12, 6, 77, 121, 87, 105, 70, 105, 4, 112, 97, 115, 115, 3];
    const SERIAL_WIFI: [u8; 24] =
        [73, 77, 80, 82, 79, 86, 1, 3, 14, 1, 12, 6, 77, 121, 87, 105, 70, 105, 4, 112, 97, 115, 115, 242];

    #[test]
    fn uuids() {
        assert_eq!(STATE_UUID, 0x00467768_6228_2272_4663_277478268001);
        assert_eq!(CAPABILITIES_UUID, 0x00467768_6228_2272_4663_277478268005);
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse_rpc(Transport::Ble, &BLE_WIFI), Ok(Rpc::WifiSettings { ssid: "MyWiFi", password: "pass" }));
        assert_eq!(parse_rpc(Transport::Ble, &[2, 0, 2]), Ok(Rpc::Identify));
        assert_eq!(parse_rpc(Transport::Serial, &[2, 0]), Ok(Rpc::GetState));
        assert_eq!(parse_rpc(Transport::Serial, &[3, 0]), Ok(Rpc::DeviceInfo));
        // Open network.
        assert_eq!(
            parse_rpc(Transport::Serial, &[1, 3, 1, b'x', 0]),
            Ok(Rpc::WifiSettings { ssid: "x", password: "" })
        );
    }

    #[test]
    fn rejects_bad_commands() {
        let mut bad = BLE_WIFI;
        bad[14] ^= 1;
        assert_eq!(parse_rpc(Transport::Ble, &bad), Err(Error::InvalidRpc));
        assert_eq!(parse_rpc(Transport::Ble, &[]), Err(Error::InvalidRpc));
        assert_eq!(parse_rpc(Transport::Ble, &[0x42, 0, 0x42]), Err(Error::UnknownRpc));
        assert_eq!(parse_rpc(Transport::Serial, &[1, 2, 0, 0]), Err(Error::InvalidRpc), "empty SSID");
        assert_eq!(parse_rpc(Transport::Serial, &[1, 3, 1, 0xff, 0]), Err(Error::InvalidRpc), "not UTF-8");
        assert_eq!(parse_rpc(Transport::Serial, &[1, 5, 1, b'a', 9, b'b', b'c']), Err(Error::InvalidRpc), "overrun");
        assert_eq!(parse_rpc(Transport::Serial, &[1, 9, 1, b'a', 0]), Err(Error::InvalidRpc), "length");
    }

    #[test]
    fn results() {
        let mut out = [0; RPC_MAX];
        assert_eq!(
            rpc_result(Transport::Serial, 1, &["http://x"], &mut out),
            &[1, 9, 8, b'h', b't', b't', b'p', b':', b'/', b'/', b'x']
        );
        let ble = rpc_result(Transport::Ble, 3, &["a", "bc"], &mut out);
        assert_eq!(ble, &[3, 5, 1, b'a', 2, b'b', b'c', checksum(&[3, 5, 1, b'a', 2, b'b', b'c'])]);
        // Too long: strings that do not fit are dropped.
        let long = [0u8; 200];
        let s = core::str::from_utf8(&long).unwrap();
        let r = rpc_result(Transport::Ble, 3, &[s, s], &mut out);
        assert_eq!(r[1], 201);
        assert_eq!(r.len(), 2 + 201 + 1);
    }

    #[test]
    fn ble_commands_in_chunks() {
        let mut c = BleCommand::new();
        assert_eq!(c.push(&BLE_WIFI[..5]), None);
        assert_eq!(c.push(&BLE_WIFI[5..]).as_deref(), Some(&BLE_WIFI[..]));
        // Starts over after a command; trailing garbage is cut.
        let mut extra = BLE_WIFI.to_vec();
        extra.extend_from_slice(&[9, 9]);
        assert_eq!(c.push(&extra).as_deref(), Some(&BLE_WIFI[..]));
    }

    #[test]
    fn serial_frames() {
        let mut out = [0; FRAME_MAX];
        let rpc = &SERIAL_WIFI[9..23];
        let f = serial_frame(frame::RPC, rpc, &mut out);
        assert_eq!(&f[..24], &SERIAL_WIFI[..]);
        assert_eq!(f[24], b'\n');
    }

    #[test]
    fn decodes_frames_among_console_traffic() {
        let mut d = SerialDecoder::new();
        let mut found = None;
        for &b in b"help\r\nIMPRO".iter().chain(&SERIAL_WIFI) {
            if let Some(r) = d.push(b) {
                found = Some(r.map(|(k, data)| (k, data.to_vec())));
            }
        }
        assert_eq!(found, Some(Ok((frame::RPC, SERIAL_WIFI[9..23].to_vec()))));
        assert!(!d.in_frame());
        let mut bad = SERIAL_WIFI;
        bad[23] ^= 1;
        let mut last = None;
        for &b in &bad {
            last = d.push(b).map(|r| r.map(|_| ()));
        }
        assert_eq!(last, Some(Err(Error::InvalidRpc)));
    }
}
