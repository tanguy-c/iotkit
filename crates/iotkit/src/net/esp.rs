//! Wi-Fi station, embassy-net stack, mDNS responder and HTTP GET on the chip.

use core::{
    net::Ipv4Addr,
    sync::atomic::{AtomicI32, Ordering},
};

use crate::config::esp::SharedStore;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::{
    Config as NetConfig, DhcpConfig, IpEndpoint, Runner, Stack, StackResources,
    dns::DnsQueryType,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config as WifiConfig, ControllerConfig, Interface, WifiController,
    ap::AccessPointConfig, sta::StationConfig,
};
use heapless::String;
use static_cell::StaticCell;

use crate::net::{WifiCredentials, backoff::Backoff, http, mdns};

/// DHCP, DNS, mDNS, an HTTP client, an HTTP server, one spare; with Matter, its UDP transport
/// (2 sockets) and its mDNS replace ours; the ESPHome API's server.
const SOCKETS: usize = if cfg!(feature = "matter-wifi") { 8 } else { 6 } + cfg!(feature = "esphome-api") as usize;
/// On the access point: DHCP server, DNS server, the portal, embassy-net's own DNS client, one
/// spare. Too few panics in smoltcp at the first `bind`.
const AP_SOCKETS: usize = 5;
/// Time allowed for each step of an HTTP exchange.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// The running network.
#[derive(Clone, Copy)]
pub struct Net {
    /// The embassy-net stack: sockets, DNS.
    pub stack: Stack<'static>,
    /// `iotkit-xxxx`, answered over mDNS as `iotkit-xxxx.local`.
    pub hostname: &'static str,
    /// The setup access point's network (`AP_ADDRESS`); only up after [`access_point`]`(true)`.
    pub ap_stack: Stack<'static>,
    /// Station MAC address.
    pub mac: [u8; 6],
}

/// The device's address on its own access point.
pub const AP_ADDRESS: [u8; 4] = [192, 168, 4, 1];

impl Net {
    /// Joined, with an IPv4 address.
    pub fn is_online(&self) -> bool {
        online(self.stack)
    }

    /// Waits until [`is_online`](Self::is_online). Use it rather than `stack.wait_config_up()`:
    /// with Matter the IPv6 link-local address is static, so the stack's config is "up" at
    /// boot, before any Wi-Fi.
    pub async fn wait_online(&self) {
        wait_online(self.stack).await
    }
}

fn online(stack: Stack<'_>) -> bool {
    stack.is_link_up() && stack.config_v4().is_some()
}

async fn wait_online(stack: Stack<'_>) {
    // Polled: the stack's own waits cannot tell IPv4 from the static IPv6.
    while !online(stack) {
        Timer::after_millis(250).await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Why the network could not start.
pub enum StartError {
    /// No Wi-Fi network stored, and none built in (`WIFI_SSID` at build time).
    NoCredentials,
    /// The radio did not start.
    Radio,
}

/// The stored network; the first time, the one given at build time (`WIFI_SSID`, `WIFI_PASS`
/// in `.cargo/secrets.toml`), which is then stored.
pub async fn credentials(store: &SharedStore) -> Option<WifiCredentials> {
    let mut store = store.lock().await;
    match store.load::<WifiCredentials>().await {
        Ok(Some(c)) => return Some(c),
        Ok(None) => {}
        Err(e) => log::warn!("net: stored Wi-Fi network unreadable: {e}"),
    }
    let built_in = WifiCredentials::new(option_env!("WIFI_SSID")?, option_env!("WIFI_PASS").unwrap_or(""))?;
    if let Err(e) = store.save(&built_in).await {
        log::warn!("net: could not store the Wi-Fi network: {e}");
    }
    Some(built_in)
}

/// What a deep-sleeping device remembers to join faster: the access point (channel, BSSID) and
/// the last DHCP lease.
#[cfg(feature = "deep-sleep")]
#[derive(Clone, Debug)]
pub struct FastConnect {
    /// Channel and BSSID: no scan.
    pub ap: Option<(u8, [u8; 6])>,
    /// The address to use instead of asking DHCP.
    pub ipv4: Option<embassy_net::StaticConfigV4>,
}

/// An access point's channel and BSSID.
#[cfg(feature = "deep-sleep")]
type ApCell = embassy_sync::blocking_mutex::Mutex<CriticalSectionRawMutex, core::cell::Cell<Option<(u8, [u8; 6])>>>;
#[cfg(feature = "deep-sleep")]
static AP_HINT: ApCell = embassy_sync::blocking_mutex::Mutex::new(core::cell::Cell::new(None));
#[cfg(feature = "deep-sleep")]
static STATIC_V4: embassy_sync::blocking_mutex::Mutex<
    CriticalSectionRawMutex,
    core::cell::RefCell<Option<embassy_net::StaticConfigV4>>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(None));
#[cfg(feature = "deep-sleep")]
static CONNECTED_AP: ApCell = embassy_sync::blocking_mutex::Mutex::new(core::cell::Cell::new(None));

/// [`start`], joining the remembered access point first (a scan if that fails) and with the
/// remembered address (no DHCP).
#[cfg(feature = "deep-sleep")]
pub fn start_fast(
    spawner: Spawner,
    wifi: WIFI<'static>,
    credentials: Option<WifiCredentials>,
    fast: FastConnect,
) -> Result<Net, StartError> {
    AP_HINT.lock(|h| h.set(fast.ap));
    STATIC_V4.lock(|v| *v.borrow_mut() = fast.ipv4);
    start(spawner, wifi, credentials)
}

/// The access point (channel, BSSID) of the current connection, to remember.
#[cfg(feature = "deep-sleep")]
pub fn connected_ap() -> Option<(u8, [u8; 6])> {
    CONNECTED_AP.lock(core::cell::Cell::get)
}

/// Starts Wi-Fi and the network stack; returns at once. Wait for an address with
/// `net.wait_online().await`. Without `credentials` it waits for [`join`].
pub fn start(spawner: Spawner, wifi: WIFI<'static>, credentials: Option<WifiCredentials>) -> Result<Net, StartError> {
    // Power saving while disconnected starves Bluetooth (provisioning shares the radio).
    let config = ControllerConfig::default().with_sta_disconnected_pm(false);
    let controller = WifiController::new(wifi, config).map_err(|e| {
        log::error!("net: Wi-Fi init failed: {e:?}");
        StartError::Radio
    })?;
    let interface = Interface::station();
    let mac = interface.mac_address();

    static HOSTNAME: StaticCell<String<16>> = StaticCell::new();
    let hostname = HOSTNAME.init(crate::net::hostname(mac)).as_str();

    let mut dhcp = DhcpConfig::default();
    dhcp.hostname = hostname.try_into().ok();
    #[allow(unused_mut)]
    let mut config = NetConfig::dhcpv4(dhcp);
    #[cfg(feature = "deep-sleep")]
    if let Some(v4) = STATIC_V4.lock(|v| v.borrow_mut().take()) {
        log::info!("net: address {} kept from before the sleep", v4.address);
        config.ipv4 = embassy_net::ConfigV4::Static(v4);
    }
    #[cfg(feature = "matter-wifi")]
    {
        // Matter needs IPv6; a link-local address is enough on one network.
        config.ipv6 = embassy_net::ConfigV6::Static(embassy_net::StaticConfigV6 {
            address: embassy_net::Ipv6Cidr::new(crate::net::link_local_ipv6(mac), 64),
            gateway: None,
            dns_servers: Default::default(),
        });
    }
    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();
    let seed = u64::from(esp_hal::rng::Rng::new().random()) << 32 | u64::from(esp_hal::rng::Rng::new().random());
    let (stack, runner) = embassy_net::new(interface, config, RESOURCES.init(StackResources::new()), seed);

    // The setup access point: a fixed address, its own small stack.
    static AP_RESOURCES: StaticCell<StackResources<AP_SOCKETS>> = StaticCell::new();
    let ap_config = NetConfig::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(Ipv4Addr::from(AP_ADDRESS), 24),
        gateway: None,
        dns_servers: Default::default(),
    });
    let (ap_stack, ap_runner) =
        embassy_net::new(Interface::access_point(), ap_config, AP_RESOURCES.init(StackResources::new()), seed ^ 1);

    spawner.spawn(runner_task(runner).map_err(|_| StartError::Radio)?);
    spawner.spawn(runner_task(ap_runner).map_err(|_| StartError::Radio)?);
    spawner.spawn(wifi_task(controller, stack, credentials, hostname).map_err(|_| StartError::Radio)?);
    // With Matter, its mDNS responder answers our host name too.
    #[cfg(not(feature = "matter-wifi"))]
    spawner.spawn(mdns_task(stack, hostname).map_err(|_| StartError::Radio)?);
    Ok(Net { stack, hostname, ap_stack, mac })
}

/// Time to join a network and get an address in [`join`].
const JOIN_TIMEOUT: Duration = Duration::from_secs(20);

enum Command {
    Join(WifiCredentials),
    AccessPoint(bool),
}

static COMMANDS: Channel<CriticalSectionRawMutex, Command, 2> = Channel::new();
static JOINED: Signal<CriticalSectionRawMutex, bool> = Signal::new();
/// One [`join`] at a time (portal, Bluetooth and serial can each start one): they share [`JOINED`].
static JOINING: embassy_sync::mutex::Mutex<CriticalSectionRawMutex, ()> = embassy_sync::mutex::Mutex::new(());

/// Signal strength of the station link in dBm; 0 while disconnected.
static RSSI: AtomicI32 = AtomicI32::new(0);

/// Signal strength of the network in dBm, updated every 30 s while connected.
pub fn rssi() -> Option<i32> {
    match RSSI.load(Ordering::Relaxed) {
        0 => None,
        r => Some(r),
    }
}

/// Tries `credentials` (join, then an address within 20 s). On success they replace the current
/// network (store them yourself); on failure the previous network is joined again.
pub async fn join(credentials: WifiCredentials) -> bool {
    let _one = JOINING.lock().await;
    JOINED.reset();
    COMMANDS.send(Command::Join(credentials)).await;
    JOINED.wait().await
}

/// Opens (`true`) or closes the open setup access point named after the host; the station keeps
/// its network.
pub async fn access_point(on: bool) {
    COMMANDS.send(Command::AccessPoint(on)).await;
}

/// How often a connected station checks its link and signal: rarely, as the disconnect event
/// comes at once. On battery (`matter-icd`) power saving follows the setup window within it.
const LINK_POLL: Duration = if cfg!(feature = "matter-icd") { Duration::from_secs(2) } else { Duration::from_secs(30) };

#[embassy_executor::task(pool_size = 2)]
async fn runner_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

/// Joins the network, and joins again after each disconnection, with backoff; serves [`join`]
/// and [`access_point`].
#[embassy_executor::task]
async fn wifi_task(
    controller: WifiController<'static>,
    stack: Stack<'static>,
    credentials: Option<WifiCredentials>,
    hostname: &'static str,
) {
    let mut w = Wifi { controller, stack, credentials, ap: None, hostname, channel: 1 };
    let mut backoff = Backoff::new();
    // Set when a command left the station connected: skip the connect.
    let mut connected = false;
    loop {
        let Some(current) = w.credentials.clone() else {
            log::info!("net: no Wi-Fi network yet");
            connected = w.command(COMMANDS.receive().await).await;
            continue;
        };
        if !connected {
            w.configure(Some(&current));
            match select(w.controller.connect_async(), COMMANDS.receive()).await {
                Either::First(Ok(info)) => {
                    log::info!("net: Wi-Fi connected to '{}' (channel {})", current.ssid, info.channel);
                    w.channel = info.channel;
                    #[cfg(feature = "deep-sleep")]
                    CONNECTED_AP.lock(|c| c.set(Some((info.channel, info.bssid))));
                }
                Either::First(Err(e)) => {
                    let delay = backoff.failed();
                    log::warn!("net: cannot join '{}' ({e:?}), retrying in {delay} s", current.ssid);
                    if let Either::Second(c) = select(Timer::after_secs(delay.into()), COMMANDS.receive()).await {
                        connected = w.command(c).await;
                    }
                    continue;
                }
                Either::Second(c) => {
                    connected = w.command(c).await;
                    continue;
                }
            }
        }
        backoff.succeeded();
        // Connected: stay until the link drops or a command comes. esp-radio's disconnect event
        // ends the wait at once; the poll catches a disconnection that came before the wait
        // subscribed to it (esp-radio checks, then subscribes).
        #[cfg(feature = "matter-icd")]
        let mut saving = None;
        let dropped = loop {
            if !w.controller.is_connected() {
                break true;
            }
            #[cfg(feature = "matter-icd")]
            power_save(&mut w.controller, &mut saving);
            if let Ok(rssi) = w.controller.rssi() {
                RSSI.store(rssi, Ordering::Relaxed);
            }
            let woke =
                select3(w.controller.wait_for_disconnect_async(), Timer::after(LINK_POLL), COMMANDS.receive()).await;
            if let Either3::Third(c) = woke {
                connected = w.command(c).await;
                break false;
            }
        };
        if dropped {
            log::warn!("net: Wi-Fi disconnected, reconnecting");
            RSSI.store(0, Ordering::Relaxed);
            connected = false;
            Timer::after_secs(1).await;
        }
    }
}

/// On battery (`matter-icd`): the radio sleeps between the access point's DTIM beacons, except
/// while a setup window is open (Bluetooth shares the radio, and power saving starves it). Not
/// `Maximum`: waking every `listen_interval` beacons can miss the broadcasts sent after a DTIM
/// beacon (ARP, mDNS, which Matter needs). On the C6 it needs esp-radio to keep the Wi-Fi power
/// controller's clock on while the modem dozes (esp-hal `main`, not in 1.0.0-beta.1): without it
/// the radio misses every beacon and the device drops off the network.
#[cfg(feature = "matter-icd")]
fn power_save(controller: &mut WifiController<'static>, saving: &mut Option<bool>) {
    let on = !crate::provision::esp::window_open();
    if *saving == Some(on) {
        return;
    }
    let mode = if on { esp_radio::wifi::PowerSaveMode::Minimum } else { esp_radio::wifi::PowerSaveMode::None };
    match controller.set_power_saving(mode) {
        Ok(()) => *saving = Some(on),
        Err(e) => log::warn!("net: power save {mode:?}: {e:?}"),
    }
}

struct Wifi {
    controller: WifiController<'static>,
    stack: Stack<'static>,
    credentials: Option<WifiCredentials>,
    /// The setup access point, while open.
    ap: Option<AccessPointConfig>,
    hostname: &'static str,
    /// Channel of the last station connection: the access point must use it too.
    channel: u8,
}

impl Wifi {
    /// Runs a command; returns whether the station is connected afterwards.
    async fn command(&mut self, command: Command) -> bool {
        match command {
            Command::Join(new) => {
                let ok = try_join(self, &new).await;
                if ok {
                    self.credentials = Some(new);
                }
                ok
            }
            Command::AccessPoint(on) => {
                self.ap = on.then(|| {
                    AccessPointConfig::default()
                        .with_ssid(self.hostname.try_into().unwrap_or_default())
                        .with_channel(self.channel)
                        .with_max_connections(4)
                });
                log::info!("net: setup access point '{}' {}", self.hostname, if on { "open" } else { "closed" });
                let current = self.credentials.clone();
                self.configure(current.as_ref());
                self.controller.is_connected()
            }
        }
    }

    /// Applies the station network (or none) and the access point, if open.
    fn configure(&mut self, credentials: Option<&WifiCredentials>) {
        let station = credentials.map_or_else(StationConfig::default, station_config);
        // The remembered access point, for the first connection only: later ones scan.
        #[cfg(feature = "deep-sleep")]
        let station = match AP_HINT.lock(core::cell::Cell::take) {
            Some((channel, bssid)) => station.with_channel(channel).with_bssid(bssid),
            None => station,
        };
        let config = match &self.ap {
            Some(ap) => WifiConfig::AccessPointStation(station, ap.clone()),
            None => WifiConfig::Station(station),
        };
        if let Err(e) = self.controller.set_config(&config) {
            log::error!("net: invalid Wi-Fi configuration: {e:?}");
        }
    }
}

fn station_config(credentials: &WifiCredentials) -> StationConfig {
    let authentication = if credentials.password.is_empty() {
        AuthenticationMethodConfig::Open
    } else {
        // The weakest method accepted: WPA2/WPA3 and WPA3 access points are joined too.
        AuthenticationMethodConfig::Wpa2Personal(credentials.password.as_str().try_into().unwrap_or_default())
    };
    StationConfig::default()
        .with_ssid(credentials.ssid.as_str().try_into().unwrap_or_default())
        .with_authentication(authentication)
}

/// [`join`]: leaves the current network, tries `new`, answers. Leaves the station disconnected
/// on failure, so the caller's loop rejoins the previous network.
async fn try_join(w: &mut Wifi, new: &WifiCredentials) -> bool {
    log::info!("net: trying Wi-Fi '{}'", new.ssid);
    if w.controller.is_connected() {
        let _ = w.controller.disconnect_async().await;
        // Let DHCP drop the old lease before waiting for a new one.
        Timer::after_millis(500).await;
    }
    w.configure(Some(new));
    let stack = w.stack;
    let joined = with_timeout(JOIN_TIMEOUT, async {
        let info = w.controller.connect_async().await.map_err(|e| log::warn!("net: join failed: {e:?}")).ok()?;
        wait_online(stack).await;
        Some(info.channel)
    })
    .await;
    if joined.is_err() {
        log::warn!("net: no address within {} s", JOIN_TIMEOUT.as_secs());
    }
    match joined {
        Ok(Some(channel)) => {
            w.channel = channel;
            log::info!("net: joined '{}'", new.ssid);
            JOINED.signal(true);
            true
        }
        _ => {
            log::warn!("net: could not join '{}'", new.ssid);
            let _ = w.controller.disconnect_async().await;
            JOINED.signal(false);
            false
        }
    }
}

/// The DNS-SD service answered next to the host name (`esphome-api`).
#[cfg(feature = "esphome-api")]
static SERVICE: embassy_sync::once_lock::OnceLock<mdns::Service> = embassy_sync::once_lock::OnceLock::new();

/// Answers `service` over mDNS from now on (once: a second one is ignored). Without Matter's
/// mDNS, which does not take it.
#[cfg(feature = "esphome-api")]
pub(crate) fn advertise(service: mdns::Service) {
    let _ = SERVICE.init(service);
}

/// Answers `<hostname>.local` (and the [`advertise`]d service), and announces it whenever the
/// address changes.
#[embassy_executor::task]
async fn mdns_task(stack: Stack<'static>, hostname: &'static str) {
    let (mut rx_meta, mut tx_meta) = ([PacketMetadata::EMPTY; 4], [PacketMetadata::EMPTY; 4]);
    let (mut rx, mut tx) = ([0u8; 512], [0u8; 512]);
    let mut out = [0u8; if cfg!(feature = "esphome-api") { 512 } else { 128 }];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    if let Err(e) = socket.bind(mdns::PORT) {
        log::error!("net: mDNS bind failed: {e:?}");
        return;
    }
    let group = IpEndpoint::new(Ipv4Addr::from(mdns::GROUP).into(), mdns::PORT);
    let mut buf = [0u8; 512];
    // A deep-sleeping device says goodbye before it sleeps: Home Assistant reconnects as soon as
    // it announces itself again, not at its next retry.
    #[cfg(all(feature = "esphome-api", feature = "deep-sleep"))]
    let mut party = crate::runtime::shutdown::register();
    loop {
        wait_online(stack).await;
        let Some(ip) = stack.config_v4().map(|c| c.address.address().octets()) else { continue };
        if let Err(e) = stack.join_multicast_group(Ipv4Addr::from(mdns::GROUP)) {
            log::warn!("net: mDNS group join failed: {e:?}");
        }
        log::info!("net: address {}, answering {hostname}.local", Ipv4Addr::from(ip));
        if let Some(n) = mdns::announcement(hostname, ip, &mut out) {
            let _ = socket.send_to(&out[..n], group).await;
        }
        #[cfg(feature = "esphome-api")]
        if let Some(n) = SERVICE.try_get().and_then(|s| s.announcement(hostname, ip, &mut out)) {
            let _ = socket.send_to(&out[..n], group).await;
        }
        // Serve until the address goes away.
        loop {
            #[cfg(all(feature = "esphome-api", feature = "deep-sleep"))]
            let going = async {
                match party.as_mut() {
                    Some(p) => p.requested().await,
                    None => core::future::pending().await,
                }
            };
            #[cfg(not(all(feature = "esphome-api", feature = "deep-sleep")))]
            let going = core::future::pending::<()>();
            match select3(socket.recv_from(&mut buf), stack.wait_config_down(), going).await {
                Either3::Third(()) => {
                    #[cfg(all(feature = "esphome-api", feature = "deep-sleep"))]
                    {
                        if let Some(n) = SERVICE.try_get().and_then(|s| s.goodbye(hostname, ip, &mut out)) {
                            let _ = socket.send_to(&out[..n], group).await;
                        }
                        if let Some(p) = party.as_mut() {
                            p.done();
                        }
                    }
                    core::future::pending::<()>().await;
                }
                Either3::First(Ok((n, _from))) => {
                    if let Some(len) = mdns::answer(&buf[..n], hostname, ip, &mut out) {
                        let _ = socket.send_to(&out[..len], group).await;
                    }
                    #[cfg(feature = "esphome-api")]
                    if let Some(len) = SERVICE.try_get().and_then(|s| s.answer(&buf[..n], hostname, ip, &mut out)) {
                        let _ = socket.send_to(&out[..len], group).await;
                    }
                }
                Either3::First(Err(_)) => {}
                Either3::Second(()) => break,
            }
        }
    }
}

/// A connected stream: TCP, or TLS over it (`tls`). The methods return the socket's own futures,
/// so code written for a `Stream` compiles for a TCP socket as if written for it.
pub(crate) trait Stream {
    /// Why a read or write failed.
    type Error: core::fmt::Debug;
    /// Reads what is there (`Ok(0)`: the peer closed).
    fn recv<'a>(&'a mut self, buf: &'a mut [u8]) -> impl Future<Output = Result<usize, Self::Error>> + 'a;
    /// Sends `bytes` now (TLS: the record goes out instead of waiting in its buffer).
    fn send<'a>(&'a mut self, bytes: &'a [u8]) -> impl Future<Output = Result<(), Self::Error>> + 'a;
    /// Closes the connection after what was sent (TLS: the TCP socket closes when dropped).
    fn shut(&mut self);
}

impl Stream for TcpSocket<'_> {
    type Error = embassy_net::tcp::Error;
    fn recv<'a>(&'a mut self, buf: &'a mut [u8]) -> impl Future<Output = Result<usize, Self::Error>> + 'a {
        TcpSocket::read(self, buf)
    }
    fn send<'a>(&'a mut self, bytes: &'a [u8]) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        self.write_all(bytes)
    }
    fn shut(&mut self) {
        self.close();
    }
}

#[cfg(feature = "tls")]
impl Stream for super::tls::Connection<'_> {
    type Error = embedded_tls::TlsError;
    fn recv<'a>(&'a mut self, buf: &'a mut [u8]) -> impl Future<Output = Result<usize, Self::Error>> + 'a {
        self.read(buf)
    }
    async fn send(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.write_all(bytes).await?;
        self.flush().await
    }
    fn shut(&mut self) {}
}

/// Why a GET failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GetError<E> {
    /// Not a valid URL.
    Url(http::Error),
    /// The host name did not resolve.
    Dns,
    /// The TCP connection failed.
    Connect,
    /// The TLS handshake failed (`tls`): the certificate, or the server.
    #[cfg(feature = "tls")]
    Tls,
    /// Read/write failure or timeout.
    Io,
    /// Not a valid HTTP response.
    BadResponse(http::Error),
    /// Not `200` (`206` for a range).
    Status(u16),
    /// The body ended after `received` of `expected` bytes.
    Truncated {
        /// Bytes received.
        received: u32,
        /// Bytes announced.
        expected: u32,
    },
    /// The sink refused the data.
    Sink(E),
}

/// Where a response body goes.
// The executor is single-threaded: the futures need not be `Send`.
#[allow(async_fn_in_trait)]
pub trait Sink {
    /// Why the sink refused data.
    type Error;
    /// The head arrived; the body length, if announced.
    fn begin(&mut self, content_length: Option<u32>) -> Result<(), Self::Error>;
    /// The next bytes of the body.
    async fn data(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

/// GETs `url` (`http://`, or `https://` with the `tls` feature) into `sink`.
pub async fn get<S: Sink>(stack: Stack<'static>, url: &str, sink: &mut S) -> Result<(), GetError<S::Error>> {
    get_from(stack, url, 0, sink).await
}

/// The GET over the connected `$socket` (TCP, or TLS over it). A macro, expanded per stream
/// type, so a build without `tls` keeps one state machine for the whole GET.
macro_rules! exchange {
    ($socket:ident, $url:ident, $from:ident, $sink:ident) => {{
        let mut request = [0u8; 384];
        let n = http::write_get(&$url, $from, &mut request).map_err(GetError::Url)?;
        Stream::send(&mut $socket, &request[..n]).await.map_err(|_| GetError::Io)?;

        // Head, possibly with the start of the body.
        let mut buf = [0u8; 1024];
        let mut filled = 0;
        let head = loop {
            let n = $socket.read(&mut buf[filled..]).await.map_err(|_| GetError::Io)?;
            if n == 0 {
                return Err(GetError::BadResponse(http::Error::BadResponse));
            }
            filled += n;
            match http::parse_response_head(&buf[..filled]).map_err(GetError::BadResponse)? {
                Some(head) => break head,
                None if filled == buf.len() => return Err(GetError::BadResponse(http::Error::BadResponse)),
                None => {}
            }
        };
        if head.status != if $from > 0 { 206 } else { 200 } {
            return Err(GetError::Status(head.status));
        }
        $sink.begin(head.content_length).map_err(GetError::Sink)?;

        let mut received = (filled - head.len) as u32;
        if received > 0 {
            $sink.data(&buf[head.len..filled]).await.map_err(GetError::Sink)?;
        }
        while head.content_length.is_none_or(|len| received < len) {
            let n = match $socket.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    log::warn!("net: GET {}: read failed after {received} bytes: {e:?}", $url.path);
                    0
                }
            };
            if n == 0 {
                break;
            }
            received += n as u32;
            $sink.data(&buf[..n]).await.map_err(GetError::Sink)?;
        }
        Stream::shut(&mut $socket);
        if let Some(expected) = head.content_length.filter(|&len| received != len) {
            return Err(GetError::Truncated { received, expected });
        }
        Ok(())
    }};
}

/// GETs `url` from byte `from` on (a `Range` request when `from > 0`): resumes an interrupted
/// download. The sink's `begin` gets the length of the rest.
pub async fn get_from<S: Sink>(
    stack: Stack<'static>,
    url: &str,
    from: u32,
    sink: &mut S,
) -> Result<(), GetError<S::Error>> {
    let url = http::Url::parse(url).map_err(GetError::Url)?;
    let ip = match url.host.parse::<Ipv4Addr>() {
        Ok(ip) => ip,
        Err(_) => {
            let found = stack.dns_query(url.host, DnsQueryType::A).await.map_err(|_| GetError::Dns)?;
            match found.first() {
                Some(embassy_net::IpAddress::Ipv4(ip)) => *ip,
                // An A query only answers IPv4 (the arm is unreachable without IPv6).
                #[allow(unreachable_patterns)]
                _ => return Err(GetError::Dns),
            }
        }
    };

    // A large window keeps data coming while the caller writes flash.
    #[cfg(not(feature = "tls"))]
    let (mut rx, mut tx) = ([0u8; 16 * 1024], [0u8; 512]);
    #[cfg(not(feature = "tls"))]
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    // With TLS, in a static: in the future, it is copied through the OTA task's stack frame with
    // the handshake's futures, which then overflows the stack.
    #[cfg(feature = "tls")]
    static WINDOW: embassy_sync::mutex::Mutex<CriticalSectionRawMutex, ([u8; 16 * 1024], [u8; 512])> =
        embassy_sync::mutex::Mutex::new(([0; 16 * 1024], [0; 512]));
    #[cfg(feature = "tls")]
    let mut window = WINDOW.lock().await;
    #[cfg(feature = "tls")]
    let (rx, tx) = &mut *window;
    #[cfg(feature = "tls")]
    let mut socket = TcpSocket::new(stack, rx, tx);
    socket.set_timeout(Some(HTTP_TIMEOUT));
    with_timeout(HTTP_TIMEOUT, socket.connect((ip, url.port)))
        .await
        .map_err(|_| GetError::Connect)?
        .map_err(|_| GetError::Connect)?;

    #[cfg(feature = "tls")]
    if url.tls {
        static BUFFERS: super::tls::SharedBuffers = super::tls::Buffers::shared();
        let mut buffers = BUFFERS.lock().await;
        let super::tls::Buffers { read, write } = &mut *buffers;
        let mut socket = super::tls::open(socket, url.host, read, write).await.map_err(|e| {
            log::warn!("net: TLS with {}: {e:?}", url.host);
            GetError::Tls
        })?;
        return exchange!(socket, url, from, sink);
    }
    exchange!(socket, url, from, sink)
}
