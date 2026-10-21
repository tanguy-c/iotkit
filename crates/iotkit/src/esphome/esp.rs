//! The ESPHome API server on the chip: one client at a time on TCP 6053, encrypted; entity
//! states out, commands in.

use core::fmt::Write as _;

use crate::config::esp::SharedStore;
use crate::esphome::{
    ApiKey,
    api::{self, id},
    noise::{self, Cipher},
    proto::Writer,
};
use crate::model::{Binding, Device};
use crate::net::{esp::Net, mdns::Service};
use embassy_futures::select::{Either3, select3};
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embedded_io_async::Write;
use heapless::String;

/// The API's port.
pub const PORT: u16 = 6053;
/// Largest frame either way: a message with its 4-byte header and tag (the longest are the
/// entity descriptions with options).
const FRAME_MAX: usize = 1024;
/// A client silent this long is gone (Home Assistant pings every 20 s).
const IDLE: Duration = Duration::from_secs(90);
/// Frame header: the "encrypted" marker and the length, big-endian.
const HEADER: usize = 3;
/// Message header inside a frame: id and length, big-endian.
const MESSAGE: usize = 4;

#[derive(Debug)]
enum Error {
    /// The client closed the connection (or said goodbye).
    Closed,
    Io,
    /// Not the API's encrypted protocol.
    Protocol,
    /// The client does not have the key.
    Key,
    /// A message failed to decrypt, or to fit.
    Frame,
}

/// The stored key; the first time, `ESPHOME_API_KEY` from build time (base64) or a random one,
/// which is then stored.
pub async fn key(store: &SharedStore) -> ApiKey {
    let mut store = store.lock().await;
    if let Ok(Some(key)) = store.load::<ApiKey>().await {
        return key;
    }
    let key = option_env!("ESPHOME_API_KEY").and_then(ApiKey::parse).unwrap_or_else(|| {
        let mut key = [0; 32];
        esp_hal::rng::Rng::new().read(&mut key);
        ApiKey(key)
    });
    let _ = store.save(&key).await;
    key
}

/// Serves the API for `device` as node `net.hostname`, and advertises it over mDNS; never
/// returns.
#[embassy_executor::task]
pub async fn api_task(net: Net, device: &'static Device, key: ApiKey) {
    let binding = Binding::allocate().expect("esphome: no binding left");
    let (mut mac, mut txt_mac, mut txt_name) = (String::<17>::new(), String::<16>::new(), String::<48>::new());
    let _ = txt_mac.push_str("mac=");
    for (i, b) in net.mac.iter().enumerate() {
        let _ = write!(mac, "{}{b:02X}", if i == 0 { "" } else { ":" });
        let _ = write!(txt_mac, "{b:02x}");
    }
    let _ = write!(txt_name, "friendly_name={}", device.name);
    let advertised = Service::new(
        "_esphomelib",
        PORT,
        &[
            &txt_mac,
            &txt_name,
            concat!("version=", env!("CARGO_PKG_VERSION")),
            "network=wifi",
            "api_encryption=Noise_NNpsk0_25519_ChaChaPoly_SHA256",
        ],
    );
    if let Some(service) = advertised {
        crate::net::esp::advertise(service);
    }
    let mut party = crate::runtime::shutdown::register();
    let (mut rx, mut tx) = ([0u8; 1024], [0u8; 2048]);
    log::info!("esphome: API on port {PORT}");
    loop {
        net.wait_online().await;
        let mut socket = TcpSocket::new(net.stack, &mut rx, &mut tx);
        socket.set_timeout(Some(IDLE));
        if socket.accept(PORT).await.is_err() {
            continue;
        }
        let peer = socket.remote_endpoint();
        let result = session(&mut socket, net, device, &key, &mac, binding, party.as_mut()).await;
        match result {
            Err(Error::Closed) | Ok(()) => log::info!("esphome: {peer:?} disconnected"),
            Err(e) => log::warn!("esphome: {peer:?}: {e:?}"),
        }
        socket.close();
        let _ = socket.flush().await;
        if let Some(p) = party.as_mut()
            && p.is_requested()
        {
            p.done();
            core::future::pending::<()>().await;
        }
    }
}

/// Received bytes, cut into frames.
struct Inbox {
    buf: [u8; FRAME_MAX],
    len: usize,
}

impl Inbox {
    /// The next whole frame's payload, if it is in.
    fn frame(&self) -> Result<Option<core::ops::Range<usize>>, Error> {
        if self.len < HEADER {
            return Ok(None);
        }
        if self.buf[0] != 1 {
            return Err(Error::Protocol);
        }
        let end = HEADER + usize::from(u16::from_be_bytes([self.buf[1], self.buf[2]]));
        if end > FRAME_MAX {
            return Err(Error::Frame);
        }
        Ok((self.len >= end).then_some(HEADER..end))
    }

    /// Reads until a whole frame is in.
    async fn next(&mut self, socket: &mut TcpSocket<'_>) -> Result<core::ops::Range<usize>, Error> {
        loop {
            if let Some(frame) = self.frame()? {
                return Ok(frame);
            }
            match socket.read(&mut self.buf[self.len..]).await {
                Ok(0) | Err(_) => return Err(Error::Closed),
                Ok(n) => self.len += n,
            }
        }
    }

    /// Drops the frame ending at `end`.
    fn consume(&mut self, end: usize) {
        self.buf.copy_within(end..self.len, 0);
        self.len -= end;
    }
}

/// Sends a frame of `payload` as is (the handshake).
async fn send_plain(socket: &mut TcpSocket<'_>, payload: &[u8]) -> Result<(), Error> {
    let len = u16::try_from(payload.len()).map_err(|_| Error::Frame)?;
    let [hi, lo] = len.to_be_bytes();
    socket.write_all(&[1, hi, lo]).await.map_err(|_| Error::Io)?;
    socket.write_all(payload).await.map_err(|_| Error::Io)
}

/// Encrypts and sends the message `write` puts in a frame (`None`: nothing to send).
async fn send(
    socket: &mut TcpSocket<'_>,
    cipher: &mut Cipher,
    out: &mut [u8; FRAME_MAX],
    write: impl FnOnce(&mut Writer) -> Option<u16>,
) -> Result<(), Error> {
    let body = HEADER + MESSAGE;
    let mut w = Writer::new(&mut out[body..FRAME_MAX - noise::TAG]);
    let Some(id) = write(&mut w) else { return Ok(()) };
    let Some(n) = w.finish() else {
        log::warn!("esphome: message {id} too long, not sent");
        return Ok(());
    };
    out[HEADER..HEADER + 2].copy_from_slice(&id.to_be_bytes());
    out[HEADER + 2..body].copy_from_slice(&(n as u16).to_be_bytes());
    let tag = cipher.encrypt(&mut out[HEADER..body + n]);
    out[body + n..body + n + noise::TAG].copy_from_slice(&tag);
    let frame = MESSAGE + n + noise::TAG;
    out[0] = 1;
    out[1..HEADER].copy_from_slice(&(frame as u16).to_be_bytes());
    socket.write_all(&out[..HEADER + frame]).await.map_err(|_| Error::Io)
}

async fn session(
    socket: &mut TcpSocket<'_>,
    net: Net,
    device: &'static Device,
    key: &ApiKey,
    mac: &str,
    binding: Binding,
    mut party: Option<&mut crate::runtime::shutdown::Party>,
) -> Result<(), Error> {
    let mut inbox = Inbox { buf: [0; FRAME_MAX], len: 0 };
    let mut out = [0u8; FRAME_MAX];

    // The client's hello (an empty frame), then its handshake message after a 0 byte.
    let hello = inbox.next(socket).await?;
    inbox.consume(hello.end);
    let handshake = inbox.next(socket).await?;
    let message = inbox.buf.get(handshake.clone()).and_then(|m| m.split_first());
    // Ours: the protocol chosen, our name and MAC address.
    let mut hello = heapless::Vec::<u8, 64>::new();
    let _ = hello.push(1);
    for part in [net.hostname, mac] {
        let _ = hello.extend_from_slice(part.as_bytes());
        let _ = hello.push(0);
    }
    send_plain(socket, &hello).await?;
    let mut reply = [0u8; 1 + noise::HANDSHAKE];
    let mut ephemeral = [0u8; 32];
    esp_hal::rng::Rng::new().read(&mut ephemeral);
    let ciphers = match message {
        Some((0, message)) => noise::respond(&key.0, ephemeral, message, (&mut reply[1..]).try_into().unwrap()),
        _ => None,
    };
    let Some((mut rx, mut tx)) = ciphers else {
        // The text clients show for a wrong key.
        send_plain(socket, b"\x01Handshake MAC failure").await?;
        return Err(Error::Key);
    };
    send_plain(socket, &reply).await?;
    inbox.consume(handshake.end);

    let mut subscribed = false;
    loop {
        if subscribed {
            for entity in device.entities {
                if entity.take_dirty(binding) {
                    send(socket, &mut tx, &mut out, |w| api::state(w, *entity)).await?;
                }
            }
        }
        let changed = async {
            match subscribed {
                true => crate::model::changed(binding).await,
                false => core::future::pending().await,
            }
        };
        let restart = async {
            match party.as_mut() {
                Some(p) => p.requested().await,
                None => core::future::pending().await,
            }
        };
        let frame = match select3(inbox.next(socket), changed, restart).await {
            Either3::First(frame) => frame?,
            Either3::Second(()) => continue,
            Either3::Third(()) => {
                // A planned restart: Home Assistant shows the device unavailable at once.
                send(socket, &mut tx, &mut out, |_| Some(id::DISCONNECT_REQUEST)).await?;
                return Ok(());
            }
        };
        let end = frame.end;
        let n = rx.decrypt(&mut inbox.buf[frame]).ok_or(Error::Frame)?;
        let message = &inbox.buf[HEADER..HEADER + n];
        let &[a, b, c, d] = message.first_chunk::<MESSAGE>().ok_or(Error::Frame)?;
        let (msg, len) = (u16::from_be_bytes([a, b]), usize::from(u16::from_be_bytes([c, d])));
        let body = message.get(MESSAGE..MESSAGE + len).ok_or(Error::Frame)?;
        match msg {
            id::HELLO_REQUEST => send(socket, &mut tx, &mut out, |w| Some(api::hello(w, net.hostname))).await?,
            id::DEVICE_INFO_REQUEST => {
                send(socket, &mut tx, &mut out, |w| Some(api::device_info(w, device, mac))).await?
            }
            id::LIST_ENTITIES_REQUEST => {
                for entity in device.entities {
                    send(socket, &mut tx, &mut out, |w| Some(api::list_entity(w, *entity))).await?;
                }
                send(socket, &mut tx, &mut out, |_| Some(id::LIST_ENTITIES_DONE)).await?;
            }
            id::SUBSCRIBE_STATES_REQUEST => {
                subscribed = true;
                device.entities.iter().for_each(|e| e.mark_dirty(binding));
            }
            id::PING_REQUEST => send(socket, &mut tx, &mut out, |_| Some(id::PING_RESPONSE)).await?,
            id::DISCONNECT_REQUEST => {
                send(socket, &mut tx, &mut out, |_| Some(id::DISCONNECT_RESPONSE)).await?;
                return Err(Error::Closed);
            }
            id::DISCONNECT_RESPONSE => return Err(Error::Closed),
            // Commands; other requests (logs, Home Assistant states, services) are not served.
            _ => {
                api::command(device, msg, body, |entity, command| {
                    if entity.command(command) {
                        log::info!("esphome: {} <- {command:?}", entity.meta().id);
                    } else {
                        log::warn!("esphome: {} takes no {command:?}", entity.meta().id);
                    }
                });
            }
        }
        inbox.consume(end);
    }
}
