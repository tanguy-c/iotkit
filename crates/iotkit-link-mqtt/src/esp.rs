//! The MQTT client on the chip: connects, announces the entities to Home Assistant, publishes
//! their states, applies commands, reconnects.

use core::{net::Ipv4Addr, ops::Range};

use embassy_futures::select::{Either4, select4};
use embassy_net::{dns::DnsQueryType, tcp::TcpSocket};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::Write;
use iotkit_config::esp::SharedStore;
use iotkit_model::{Binding, Device, Kind};
use iotkit_net::{backoff::Backoff, esp::Net};

use crate::{
    Broker,
    atomic::{AtomicBool, Ordering},
    codec::{self, Packet, Will},
    ha,
};

const KEEP_ALIVE_S: u16 = 60;
/// Largest packet sent (a discovery payload and its topic) or received.
const PACKET_MAX: usize = 1280;

static CONNECTED: AtomicBool = AtomicBool::new(false);
static UP: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Connected to the broker, entities announced.
pub fn is_connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

/// Waits for the first successful connection (for the health check).
pub async fn wait_connected() {
    while !is_connected() {
        UP.wait().await;
    }
}

/// The stored broker; the first time, the one given at build time (`MQTT_HOST` as `host` or
/// `host:port`, `MQTT_USER`, `MQTT_PASS` in `.cargo/secrets.toml`), which is then stored.
pub async fn broker(store: &SharedStore) -> Option<Broker> {
    let mut store = store.lock().await;
    if let Ok(Some(b)) = store.load::<Broker>().await {
        return Some(b);
    }
    let built_in = Broker::parse(
        option_env!("MQTT_HOST").filter(|h| !h.is_empty())?,
        option_env!("MQTT_USER").unwrap_or(""),
        option_env!("MQTT_PASS").unwrap_or(""),
    )?;
    let _ = store.save(&built_in).await;
    Some(built_in)
}

#[derive(Debug)]
enum Error {
    Dns,
    Connect,
    Io,
    Refused(u8),
    Protocol,
    Timeout,
}

/// Runs the MQTT link for `device` as node `net.hostname`; never returns.
#[embassy_executor::task]
pub async fn mqtt_task(net: Net, device: &'static Device, broker: Broker) {
    let mut party = iotkit_runtime::shutdown::register();
    let mut backoff = Backoff::new();
    loop {
        net.wait_online().await;
        let result = session(net, device, &broker, party.as_mut()).await;
        if let Some(p) = party.as_mut()
            && p.is_requested()
        {
            // Restarting: no reconnection.
            p.done();
            core::future::pending::<()>().await;
        }
        // A session that got connected restarts the backoff: the next attempt comes at once.
        if CONNECTED.swap(false, Ordering::Relaxed) {
            backoff.succeeded();
        }
        let delay = backoff.failed();
        match &result {
            Err(Error::Refused(code)) => log::error!("mqtt: broker refused the connection (code {code})"),
            Err(e) => log::warn!("mqtt: {e:?}, reconnecting in {delay} s"),
            Ok(()) => {}
        }
        Timer::after_secs(delay.into()).await;
    }
}

async fn session(
    net: Net,
    device: &'static Device,
    broker: &Broker,
    mut party: Option<&mut iotkit_runtime::shutdown::Party>,
) -> Result<(), Error> {
    let ip = match broker.host.parse::<Ipv4Addr>() {
        Ok(ip) => ip,
        Err(_) => match net.stack.dns_query(&broker.host, DnsQueryType::A).await.map_err(|_| Error::Dns)?.first() {
            Some(embassy_net::IpAddress::Ipv4(ip)) => *ip,
            // An A query only answers IPv4 (the arm is unreachable without IPv6).
            #[allow(unreachable_patterns)]
            _ => return Err(Error::Dns),
        },
    };
    let (mut rx, mut tx) = ([0u8; 2048], [0u8; 2048]);
    let mut socket = TcpSocket::new(net.stack, &mut rx, &mut tx);
    socket.set_timeout(Some(Duration::from_secs(u64::from(KEEP_ALIVE_S) * 2)));
    with_timeout(Duration::from_secs(10), socket.connect((ip, broker.port)))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Connect)?;

    let node = net.hostname;
    let availability = ha::availability_topic(node);
    let mut out = [0u8; PACKET_MAX];
    let will = Will { topic: &availability, payload: b"offline", retain: true };
    let user = (!broker.user.is_empty()).then_some(broker.user.as_str());
    let password = (!broker.password.is_empty()).then_some(broker.password.as_str());
    let n = codec::connect(&mut out, node, KEEP_ALIVE_S, Some(&will), user, password).map_err(|_| Error::Protocol)?;
    socket.write_all(&out[..n]).await.map_err(|_| Error::Io)?;

    let mut inbox = Inbox { buf: [0; PACKET_MAX], len: 0 };
    match with_timeout(Duration::from_secs(10), inbox.next(&mut socket)).await.map_err(|_| Error::Timeout)?? {
        (0x20, 0) => {}
        (0x20, code) => return Err(Error::Refused(code)),
        _ => return Err(Error::Protocol),
    }

    let commands = ha::commands_filter(node);
    // Commands for one setting (`<id>/<feature>/set`) only when some entity takes them. In a
    // block: the filter is not kept in the task's state.
    let n = if ha::has_feature_commands(device) {
        let features = ha::feature_commands_filter(node);
        codec::subscribe(&mut out, 1, &[&commands, &features, ha::HA_STATUS])
    } else {
        codec::subscribe(&mut out, 1, &[&commands, ha::HA_STATUS])
    }
    .map_err(|_| Error::Protocol)?;
    socket.write_all(&out[..n]).await.map_err(|_| Error::Io)?;
    publish(&mut socket, &mut out, &availability, b"online", true).await?;
    announce(&mut socket, &mut out, device, node).await?;
    log::info!("mqtt: connected to {}:{}, {} entities announced", broker.host, broker.port, device.entities.len());
    CONNECTED.store(true, Ordering::Relaxed);
    UP.signal(());

    let ping_every = Duration::from_secs(u64::from(KEEP_ALIVE_S) / 2);
    let mut last_rx = Instant::now();
    // A fixed deadline: state changes wake the loop often, and must not keep postponing it.
    let mut next_ping = last_rx + ping_every;
    loop {
        // States first: the changes that came while the last packet was handled.
        for entity in device.entities {
            // An update's state is longer than the others: formatted straight into the packet.
            if entity.meta().kind == Kind::Update {
                if entity.take_dirty(Binding::MQTT) {
                    let topic = ha::state_topic(node, entity.meta().id);
                    let n = codec::publish_with(&mut out, &topic, true, |mut w| ha::update_payload(*entity, &mut w))
                        .map_err(|_| Error::Protocol)?;
                    socket.write_all(&out[..n]).await.map_err(|_| Error::Io)?;
                }
                continue;
            }
            if entity.take_dirty(Binding::MQTT)
                && let Some(state) = ha::state_payload(*entity)
            {
                let retain = ha::retained(*entity);
                publish(&mut socket, &mut out, &ha::state_topic(node, entity.meta().id), state.as_bytes(), retain)
                    .await?;
            }
        }
        let ping = Timer::at(next_ping);
        let restart = async {
            match party.as_mut() {
                Some(p) => p.requested().await,
                None => core::future::pending().await,
            }
        };
        match select4(inbox.fill(&mut socket), iotkit_model::changed(Binding::MQTT), ping, restart).await {
            Either4::Fourth(()) => {
                // A planned restart: say so, so Home Assistant shows the device unavailable.
                publish(&mut socket, &mut out, &availability, b"offline", true).await?;
                socket.write_all(&codec::DISCONNECT).await.map_err(|_| Error::Io)?;
                let _ = socket.flush().await;
                socket.close();
                log::info!("mqtt: offline for the restart");
                return Ok(());
            }
            Either4::First(r) => {
                r?;
                last_rx = Instant::now();
                while let Some((topic, payload, end)) = inbox.publish()? {
                    let topic = core::str::from_utf8(&inbox.buf[topic]).unwrap_or("");
                    let payload = &inbox.buf[payload];
                    if topic == ha::HA_STATUS {
                        if payload == b"online" {
                            log::info!("mqtt: Home Assistant restarted, announcing again");
                            inbox.consume(end);
                            announce(&mut socket, &mut out, device, node).await?;
                            continue;
                        }
                    } else if let Some((entity, feature)) =
                        ha::command_entity(node, topic).and_then(|(id, feature)| Some((device.entity(id)?, feature)))
                    {
                        match ha::parse_command(entity, feature, payload) {
                            Some(command) if entity.command(command) => {
                                log::info!("mqtt: {} <- {command:?}", entity.meta().id)
                            }
                            _ => log::warn!("mqtt: bad command for {}", entity.meta().id),
                        }
                    }
                    inbox.consume(end);
                }
            }
            Either4::Second(()) => {}
            Either4::Third(()) => {
                if last_rx.elapsed() > Duration::from_secs(u64::from(KEEP_ALIVE_S) * 3 / 2) {
                    return Err(Error::Timeout);
                }
                socket.write_all(&codec::PINGREQ).await.map_err(|_| Error::Io)?;
                next_ping = Instant::now() + ping_every;
            }
        }
    }
}

/// Discovery for every entity, then every known state (all retained).
async fn announce(
    socket: &mut TcpSocket<'_>,
    out: &mut [u8; PACKET_MAX],
    device: &Device,
    node: &str,
) -> Result<(), Error> {
    for entity in device.entities {
        let Some(config) = ha::discovery_payload::<1024>(device, node, *entity) else {
            log::error!("mqtt: discovery payload of {} too large", entity.meta().id);
            continue;
        };
        publish(socket, out, &ha::discovery_topic(node, *entity), config.as_bytes(), true).await?;
        entity.mark_dirty(Binding::MQTT);
    }
    Ok(())
}

async fn publish(
    socket: &mut TcpSocket<'_>,
    out: &mut [u8; PACKET_MAX],
    topic: &str,
    payload: &[u8],
    retain: bool,
) -> Result<(), Error> {
    let n = codec::publish(out, topic, payload, retain).map_err(|_| Error::Protocol)?;
    socket.write_all(&out[..n]).await.map_err(|_| Error::Io)
}

/// A received PUBLISH in [`Inbox::buf`]: topic, payload, end of the packet.
type PublishAt = (Range<usize>, Range<usize>, usize);

/// Bytes received, cut into packets.
struct Inbox {
    buf: [u8; PACKET_MAX],
    len: usize,
}

impl Inbox {
    /// Reads more bytes.
    async fn fill(&mut self, socket: &mut TcpSocket<'_>) -> Result<(), Error> {
        if self.len == self.buf.len() {
            // A packet larger than the buffer: drop what is there.
            log::warn!("mqtt: incoming packet too large, dropped");
            self.len = 0;
        }
        match socket.read(&mut self.buf[self.len..]).await {
            Ok(0) | Err(_) => Err(Error::Io),
            Ok(n) => {
                log::debug!("mqtt: {n} bytes in, first {:02x?}", &self.buf[self.len..self.len + n.min(8)]);
                self.len += n;
                Ok(())
            }
        }
    }

    /// The next whole packet: its type and, for CONNACK, the code.
    async fn next(&mut self, socket: &mut TcpSocket<'_>) -> Result<(u8, u8), Error> {
        loop {
            if let Some((packet, n)) = codec::parse(&self.buf[..self.len]).map_err(|_| Error::Protocol)? {
                let r = match packet {
                    Packet::ConnAck { code } => (0x20, code),
                    _ => (self.buf[0] & 0xf0, 0),
                };
                self.consume(n);
                return Ok(r);
            }
            self.fill(socket).await?;
        }
    }

    /// The next whole PUBLISH as ranges of `buf`: topic, payload, and the packet's end. Other
    /// packets are skipped.
    fn publish(&mut self) -> Result<Option<PublishAt>, Error> {
        loop {
            let Some((packet, n)) = codec::parse(&self.buf[..self.len]).map_err(|_| Error::Protocol)? else {
                return Ok(None);
            };
            if let Packet::Publish { topic, payload } = packet {
                let at = |s: &[u8]| {
                    let start = s.as_ptr() as usize - self.buf.as_ptr() as usize;
                    start..start + s.len()
                };
                return Ok(Some((at(topic.as_bytes()), at(payload), n)));
            }
            self.consume(n);
        }
    }

    fn consume(&mut self, n: usize) {
        self.buf.copy_within(n..self.len, 0);
        self.len -= n;
    }
}
