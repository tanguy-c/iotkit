//! `cargo iotkit wifi`: gives the device on USB the Wi-Fi network of `.cargo/secrets.toml`, over
//! Improv (`improv-serial`). The device joins it, then stores it: a published build, which leaves
//! the network out, then keeps it.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iotkit_core::improv::{
    Error, FRAME_MAX, RPC_MAX, SerialDecoder, State, Transport, command, frame, rpc_command, serial_frame,
};
use serialport::{SerialPort, SerialPortType};

/// Espressif's USB vendor id: the USB serial port of the C3, C5, C6, H2 and S3.
const ESPRESSIF: u16 = 0x303a;

/// Longer than the device's own wait for an address (30 s), and its join before that.
const JOIN_TIMEOUT: Duration = Duration::from_secs(60);

pub fn run(port: Option<String>) -> Result<()> {
    let ssid = value("WIFI_SSID")?.context("no WIFI_SSID in .cargo/secrets.toml (or the environment)")?;
    let password = value("WIFI_PASS")?.unwrap_or_default();
    let port = match port {
        Some(p) => p,
        None => find_port()?,
    };
    // Opened without setting DTR or RTS: toggling them resets the C6.
    let mut serial = serialport::new(&port, 115_200)
        .timeout(Duration::from_millis(100))
        .preserve_dtr_on_open()
        .open()
        .with_context(|| format!("opening {port}"))?;
    let mut link = Link { serial: &mut *serial, decoder: SerialDecoder::new(), log: Vec::new() };

    link.send_rpc(command::IDENTIFY_OR_STATE, &[])?;
    let state =
        link.wait(Duration::from_secs(3), |kind, data| (kind == frame::STATE).then(|| data.first().copied()))?;
    ensure!(
        state.is_some(),
        "no Improv answer on {port}: build the firmware with `improv-serial` (a default feature), and close any serial monitor"
    );
    println!("sending the Wi-Fi network of .cargo/secrets.toml to the device on {port}");
    link.send_rpc(command::WIFI_SETTINGS, &[&ssid, &password])?;
    let provisioned = link.wait(JOIN_TIMEOUT, |kind, data| match (kind, data.first().copied()) {
        (frame::STATE, Some(s)) if s == State::Provisioned as u8 => Some(Ok(())),
        (frame::ERROR, Some(e)) if e == Error::UnableToConnect as u8 => Some(Err("it could not join the network")),
        (frame::ERROR, Some(e)) if e != Error::None as u8 => Some(Err("it refused the network")),
        _ => None,
    })?;
    let failed = match provisioned {
        Some(Ok(())) => None,
        Some(Err(why)) => {
            Some(format!("{why}: check WIFI_SSID and WIFI_PASS (the password: empty, or 8 to 63 characters)"))
        }
        None => Some(format!("no answer from the device within {} s", JOIN_TIMEOUT.as_secs())),
    };
    if let Some(why) = failed {
        let log = String::from_utf8_lossy(&link.log);
        let lines: Vec<_> = log.lines().filter(|l| !l.trim().is_empty()).collect();
        eprintln!("the device's log:");
        for line in &lines[lines.len().saturating_sub(20)..] {
            eprintln!("  {}", line.trim_end());
        }
        bail!(why);
    }
    // The result names the device's address.
    let url = link.wait(Duration::from_secs(2), |kind, data| {
        (kind == frame::RPC_RESULT && data.first() == Some(&command::WIFI_SETTINGS)).then(|| first_string(data))
    })?;
    println!(
        "the device joined the network and stored it{}",
        url.flatten().map(|u| format!(": {u}")).unwrap_or_default()
    );
    Ok(())
}

/// A value of `.cargo/secrets.toml`'s `[env]`; the environment wins, as for cargo.
fn value(name: &str) -> Result<Option<String>> {
    if let Ok(v) = std::env::var(name) {
        return Ok(Some(v));
    }
    let path = Path::new(".cargo/secrets.toml");
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(None) };
    let doc: toml::Table = text.parse().with_context(|| format!("reading {}", path.display()))?;
    let v = doc.get("env").and_then(|env| env.get(name));
    Ok(v.and_then(|v| v.as_str().or_else(|| v.get("value")?.as_str())).map(str::to_owned))
}

/// The one Espressif USB serial port; else the one USB serial port.
fn find_port() -> Result<String> {
    let ports = serialport::available_ports().context("listing the serial ports")?;
    let usb = |espressif: bool| -> Vec<String> {
        ports
            .iter()
            .filter(|p| matches!(&p.port_type, SerialPortType::UsbPort(u) if !espressif || u.vid == ESPRESSIF))
            .map(|p| p.port_name.clone())
            .collect()
    };
    let (espressif, any) = (usb(true), usb(false));
    match (espressif.as_slice(), any.as_slice()) {
        ([one], _) | ([], [one]) => Ok(one.clone()),
        ([], []) => bail!("no device on USB: plug it in, or pass --port"),
        ([], all) | (all, _) => bail!("several serial ports ({}): pass --port", all.join(", ")),
    }
}

/// The first string of an RPC result: id, length, then length-prefixed strings.
fn first_string(data: &[u8]) -> Option<String> {
    let len = *data.get(2)? as usize;
    Some(String::from_utf8_lossy(data.get(3..3 + len)?).into_owned())
}

struct Link<'a> {
    serial: &'a mut dyn SerialPort,
    decoder: SerialDecoder,
    /// What the device printed besides the frames: shown when it fails.
    log: Vec<u8>,
}

impl Link<'_> {
    fn send_rpc(&mut self, id: u8, strings: &[&str]) -> Result<()> {
        let mut rpc = [0; RPC_MAX];
        let rpc = rpc_command(Transport::Serial, id, strings, &mut rpc);
        let mut out = [0; FRAME_MAX];
        self.serial.write_all(serial_frame(frame::RPC, rpc, &mut out))?;
        Ok(self.serial.flush()?)
    }

    /// Reads frames until `pick` returns something, or `timeout`; the device's log lines in
    /// between are skipped.
    fn wait<T>(&mut self, timeout: Duration, mut pick: impl FnMut(u8, &[u8]) -> Option<T>) -> Result<Option<T>> {
        let end = Instant::now() + timeout;
        let mut buf = [0; 256];
        while Instant::now() < end {
            let n = match self.serial.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => return Err(e.into()),
            };
            for &byte in &buf[..n] {
                let picked = match self.decoder.push(byte) {
                    Some(Ok((kind, data))) => pick(kind, data),
                    Some(Err(_)) => None,
                    None => {
                        self.log.extend(self.decoder.take_released());
                        if !self.decoder.in_frame() {
                            self.log.push(byte);
                        }
                        None
                    }
                };
                if picked.is_some() {
                    return Ok(picked);
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_settings_frame() {
        let mut rpc = [0; RPC_MAX];
        let rpc = rpc_command(Transport::Serial, command::WIFI_SETTINGS, &["net", "password"], &mut rpc);
        // What the device parses.
        assert_eq!(
            iotkit_core::improv::parse_rpc(Transport::Serial, rpc),
            Ok(iotkit_core::improv::Rpc::WifiSettings { ssid: "net", password: "password" })
        );
        let mut out = [0; FRAME_MAX];
        let frame = serial_frame(frame::RPC, rpc, &mut out);
        let mut decoder = SerialDecoder::new();
        let got = frame.iter().find_map(|&b| decoder.push(b).map(|r| r.map(|(k, d)| (k, d.to_vec()))));
        assert_eq!(got, Some(Ok((frame::RPC, rpc.to_vec()))));
        assert_eq!(first_string(&[1, 4, 3, b'u', b'r', b'l']), Some("url".into()));
    }
}
