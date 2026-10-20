//! The MQTT client on the chip: connects, announces the entities to Home Assistant, publishes
//! their states, applies commands, reconnects.

use core::{net::Ipv4Addr, ops::Range};

use crate::config::esp::SharedStore;
use crate::model::{Binding, Device, Kind};
use crate::net::{backoff::Backoff, esp::Net};
use embassy_futures::select::{Either4, select4};
use embassy_net::{dns::DnsQueryType, tcp::TcpSocket};
#[cfg(not(feature = "deep-sleep"))]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::Write;

use crate::mqtt::{
    Broker,
    atomic::{AtomicBool, Ordering},
    codec::{self, Packet, Will},
    ha,
};

const KEEP_ALIVE_S: u16 = 60;
/// Largest packet sent (a discovery payload and its topic) or received.
const PACKET_MAX: usize = 1280;

static CONNECTED: AtomicBool = AtomicBool::new(false);
#[cfg(not(feature = "deep-sleep"))]
static UP: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Connected to the broker, entities announced.
pub fn is_connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

/// Waits for the first successful connection (for the health check; one waiter).
#[cfg(not(feature = "deep-sleep"))]
pub async fn wait_connected() {
    while !is_connected() {
        UP.wait().await;
    }
}

/// Waits for the first successful connection. A sleepy device has several waiters (a signal
/// wakes only one), so the state is polled.
#[cfg(feature = "deep-sleep")]
pub async fn wait_connected() {
    while !is_connected() {
        Timer::after_millis(50).await;
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

/// A deep-sleeping device's MQTT options.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sleepy {
    /// How long a reading stays valid in Home Assistant (about twice the sleep).
    pub expire_after_s: u32,
    /// The discovery hash sent at the last wake-up (`discovery_hash()` then): unchanged, the
    /// discovery is not sent again.
    pub discovery_hash: u32,
}

#[cfg(feature = "deep-sleep")]
static DISCOVERY_HASH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The hash of the discovery sent (or found unchanged), to keep for the next wake-up.
#[cfg(feature = "deep-sleep")]
pub fn discovery_hash() -> u32 {
    DISCOVERY_HASH.load(Ordering::Relaxed)
}

/// Runs the MQTT link for `device` as node `net.hostname`; never returns.
#[embassy_executor::task]
pub async fn mqtt_task(net: Net, device: &'static Device, broker: Broker) {
    let mut party = crate::runtime::shutdown::register();
    let mut backoff = Backoff::new();
    loop {
        net.wait_online().await;
        let result = session::<false>(
            net,
            device,
            &broker,
            party.as_mut(),
            #[cfg(feature = "deep-sleep")]
            Sleepy::default(),
        )
        .await;
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

/// [`mqtt_task`] for a deep-sleeping device: no last will or availability (asleep is not
/// offline), retained commands (one-shot ones cleared once applied), discovery only when it
/// changed, and on shutdown the pending states then a clean disconnect.
#[cfg(feature = "deep-sleep")]
#[embassy_executor::task]
pub async fn mqtt_sleepy_task(net: Net, device: &'static Device, broker: Broker, sleepy: Sleepy) {
    let mut party = crate::runtime::shutdown::register();
    let mut backoff = Backoff::new();
    loop {
        net.wait_online().await;
        let result = session::<true>(net, device, &broker, party.as_mut(), sleepy).await;
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

async fn session<const SLEEPY: bool>(
    net: Net,
    device: &'static Device,
    broker: &Broker,
    mut party: Option<&mut crate::runtime::shutdown::Party>,
    #[cfg(feature = "deep-sleep")] sleepy: Sleepy,
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
    let will = if SLEEPY { None } else { Some(&will) };
    let n = codec::connect(&mut out, node, KEEP_ALIVE_S, will, user, password).map_err(|_| Error::Protocol)?;
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
    if !SLEEPY {
        publish(&mut socket, &mut out, &availability, b"online", true).await?;
    }
    announce::<SLEEPY>(
        &mut socket,
        &mut out,
        device,
        node,
        #[cfg(feature = "deep-sleep")]
        sleepy,
    )
    .await?;
    log::info!("mqtt: connected to {}:{}, {} entities announced", broker.host, broker.port, device.entities.len());
    CONNECTED.store(true, Ordering::Relaxed);
    #[cfg(not(feature = "deep-sleep"))]
    UP.signal(());

    let ping_every = Duration::from_secs(u64::from(KEEP_ALIVE_S) / 2);
    let mut last_rx = Instant::now();
    // A fixed deadline: state changes wake the loop often, and must not keep postponing it.
    let mut next_ping = last_rx + ping_every;
    // A sleepy device going to sleep: the pending states are being sent.
    #[cfg(feature = "deep-sleep")]
    let mut closing = false;
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
        #[cfg(feature = "deep-sleep")]
        if SLEEPY && closing {
            socket.write_all(&codec::DISCONNECT).await.map_err(|_| Error::Io)?;
            let _ = socket.flush().await;
            socket.close();
            log::info!("mqtt: states sent, disconnected for the sleep");
            return Ok(());
        }
        let restart = async {
            match party.as_mut() {
                Some(p) => p.requested().await,
                None => core::future::pending().await,
            }
        };
        match select4(inbox.fill(&mut socket), crate::model::changed(Binding::MQTT), ping, restart).await {
            Either4::Fourth(()) => {
                #[cfg(feature = "deep-sleep")]
                if SLEEPY && !closing {
                    // Going to sleep: once more round the loop, which sends the states not sent
                    // yet, then a clean disconnect.
                    closing = true;
                    continue;
                }
                if !SLEEPY {
                    // A planned restart: say so, so Home Assistant shows the device unavailable.
                    publish(&mut socket, &mut out, &availability, b"offline", true).await?;
                }
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
                            announce::<SLEEPY>(
                                &mut socket,
                                &mut out,
                                device,
                                node,
                                #[cfg(feature = "deep-sleep")]
                                Sleepy::default(),
                            )
                            .await?;
                            continue;
                        }
                    } else if let Some((entity, feature)) =
                        ha::command_entity(node, topic).and_then(|(id, feature)| Some((device.entity(id)?, feature)))
                    {
                        match ha::parse_command(entity, feature, payload) {
                            Some(command) if entity.command(command) => {
                                log::info!("mqtt: {} <- {command:?}", entity.meta().id);
                                // A retained one-shot command would run again at each wake-up.
                                if SLEEPY && ha::one_shot(&command) {
                                    let n = codec::publish(&mut out, topic, b"", true).map_err(|_| Error::Protocol)?;
                                    socket.write_all(&out[..n]).await.map_err(|_| Error::Io)?;
                                }
                            }
                            // An empty payload: a cleared retained command.
                            _ if SLEEPY && payload.is_empty() => {}
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

/// Discovery for every entity, then every known state (all retained). A sleepy device skips
/// the discovery when its hash is `sleepy.discovery_hash` (0 forces it: after Home Assistant
/// restarts).
async fn announce<const SLEEPY: bool>(
    socket: &mut TcpSocket<'_>,
    out: &mut [u8; PACKET_MAX],
    device: &Device,
    node: &str,
    #[cfg(feature = "deep-sleep")] sleepy: Sleepy,
) -> Result<(), Error> {
    #[cfg(feature = "deep-sleep")]
    if SLEEPY {
        let hash = device.entities.iter().fold(0x811c_9dc5, |h, e| {
            let p = ha::sleepy_discovery_payload::<1024>(device, node, *e, sleepy.expire_after_s);
            ha::hash(p.as_deref().unwrap_or("").as_bytes(), h)
        });
        DISCOVERY_HASH.store(hash, Ordering::Relaxed);
        if hash == sleepy.discovery_hash {
            log::info!("mqtt: discovery unchanged, not sent");
            device.entities.iter().for_each(|e| e.mark_dirty(Binding::MQTT));
            return Ok(());
        }
    }
    for entity in device.entities {
        #[cfg(feature = "deep-sleep")]
        let config = if SLEEPY {
            ha::sleepy_discovery_payload::<1024>(device, node, *entity, sleepy.expire_after_s)
        } else {
            ha::discovery_payload::<1024>(device, node, *entity)
        };
        #[cfg(not(feature = "deep-sleep"))]
        let config = ha::discovery_payload::<1024>(device, node, *entity);
        let Some(config) = config else {
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
