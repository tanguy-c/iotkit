//! Improv on the chip: the serial front-end (fed by the app's console) and the Bluetooth LE GATT
//! server (only while a provisioning window is open).

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};

use bt_hci::{
    cmd::le::{LeConnUpdate, LeReadLocalSupportedFeatures},
    controller::{ControllerCmdAsync, ControllerCmdSync, ExternalController},
};
use core::net::Ipv4Addr;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

use embassy_net::{
    IpEndpoint,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
use esp_hal::peripherals::BT;
use esp_println::Printer;
use esp_radio::ble::controller::BleConnector;
use heapless::String;
use iotkit_config::esp::SharedStore;
use iotkit_net::{
    WifiCredentials,
    esp::{AP_ADDRESS, Net},
};
use trouble_host::prelude::*;

use crate::improv::{
    BleCommand, Error, FRAME_MAX, RPC_MAX, Rpc, SERVICE_DATA_UUID, SERVICE_UUID, SerialDecoder, State, Transport,
    capability, command, frame, parse_rpc, rpc_result, serial_frame, service_data,
};
use crate::{dhcp, dns, portal};

/// How long a requested window stays open (none is needed while no network is stored).
pub const WINDOW: Duration = Duration::from_secs(600);
const CAPABILITIES: u8 = capability::IDENTIFY | capability::DEVICE_INFO;

/// What the device tells clients about itself.
#[derive(Clone, Copy)]
pub struct DeviceInfo {
    pub firmware: &'static str,
    pub version: &'static str,
    pub chip: &'static str,
}

/// Everything provisioning needs.
#[derive(Clone, Copy)]
pub struct Provisioner {
    pub net: Net,
    pub store: &'static SharedStore,
    pub info: DeviceInfo,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Set by an Improv "identify": the app can blink a LED.
pub static IDENTIFY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Opens a provisioning window: the setup access point opens and Bluetooth advertises.
pub fn open_window() {
    ACTIVE.store(true, Ordering::Relaxed);
    CHANGED.signal(());
}

pub fn window_open() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

fn close_window() {
    ACTIVE.store(false, Ordering::Relaxed);
    CHANGED.signal(());
}

impl Provisioner {
    fn url(&self) -> String<48> {
        let mut url = String::new();
        let _ = write!(url, "http://{}.local/", self.net.hostname);
        url
    }

    fn state(&self) -> State {
        if self.net.is_online() { State::Provisioned } else { State::Authorized }
    }

    /// Tries the network; stores it and closes the window when it works.
    async fn provision(&self, ssid: &str, password: &str) -> bool {
        let Some(credentials) = WifiCredentials::new(ssid, password) else {
            log::warn!("provision: refused Wi-Fi '{ssid}': the password must be empty or 8 to 63 characters");
            return false;
        };
        log::info!("provision: received Wi-Fi '{ssid}'");
        if !iotkit_net::esp::join(credentials.clone()).await {
            return false;
        }
        if let Err(e) = self.store.lock().await.save(&credentials).await {
            log::error!("provision: could not store the Wi-Fi network: {e}");
        }
        log::info!("provision: Wi-Fi '{ssid}' stored");
        close_window();
        true
    }
}

/// Improv over the serial port shared with the console.
#[derive(Default)]
pub struct Serial {
    decoder: SerialDecoder,
}

impl Serial {
    pub const fn new() -> Self {
        Self { decoder: SerialDecoder::new() }
    }

    /// Feeds a byte read from the port. `true` while it belongs to an Improv frame: keep it from
    /// the console. Answers complete frames (which can take the time of a Wi-Fi join).
    pub async fn feed(&mut self, p: &Provisioner, byte: u8) -> bool {
        let rpc = match self.decoder.push(byte) {
            None => return self.decoder.in_frame(),
            Some(Ok((frame::RPC, data))) => heapless::Vec::<u8, 256>::from_slice(data).ok(),
            Some(Ok(_)) => return true,
            Some(Err(e)) => {
                send(frame::ERROR, &[e as u8]);
                return true;
            }
        };
        let Some(rpc) = rpc else { return true };
        match parse_rpc(Transport::Serial, &rpc) {
            Ok(Rpc::GetState) => {
                let state = p.state();
                send(frame::STATE, &[state as u8]);
                if state == State::Provisioned {
                    send_result(command::IDENTIFY_OR_STATE, &[&p.url()]);
                }
            }
            Ok(Rpc::DeviceInfo) => send_result(command::DEVICE_INFO, &device_info(p)),
            Ok(Rpc::WifiSettings { ssid, password }) => {
                send(frame::STATE, &[State::Provisioning as u8]);
                if p.provision(ssid, password).await {
                    send(frame::STATE, &[State::Provisioned as u8]);
                    send_result(command::WIFI_SETTINGS, &[&p.url()]);
                } else {
                    send(frame::ERROR, &[Error::UnableToConnect as u8]);
                    send(frame::STATE, &[State::Authorized as u8]);
                }
            }
            Ok(Rpc::Identify) | Err(Error::UnknownRpc) => send(frame::ERROR, &[Error::UnknownRpc as u8]),
            Err(e) => send(frame::ERROR, &[e as u8]),
        }
        true
    }
}

fn device_info(p: &Provisioner) -> [&'static str; 4] {
    [p.info.firmware, p.info.version, p.info.chip, p.net.hostname]
}

fn send(kind: u8, data: &[u8]) {
    let mut out = [0; FRAME_MAX];
    Printer::write_bytes(serial_frame(kind, data, &mut out));
}

fn send_result(id: u8, strings: &[&str]) {
    let mut out = [0; RPC_MAX];
    let result = rpc_result(Transport::Serial, id, strings, &mut out);
    send(frame::RPC_RESULT, result);
}

#[gatt_server]
struct Server {
    improv: ImprovService,
}

#[gatt_service(uuid = "00467768-6228-2272-4663-277478268000")]
struct ImprovService {
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268001", read, notify, value = State::Authorized as u8)]
    state: u8,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268002", read, notify, value = 0)]
    error: u8,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268003", write, write_without_response)]
    rpc_command: heapless::Vec<u8, RPC_MAX>,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268004", read, notify)]
    rpc_result: heapless::Vec<u8, RPC_MAX>,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268005", read, value = CAPABILITIES)]
    capabilities: u8,
}

/// Runs the provisioning windows: the setup access point and Improv over Bluetooth LE while a
/// window is open, both off otherwise.
#[embassy_executor::task]
pub async fn window_task(mut bt: BT<'static>, p: Provisioner) {
    loop {
        while !window_open() {
            CHANGED.wait().await;
        }
        iotkit_net::esp::access_point(true).await;
        let window = async {
            // With no network stored the window stays open until provisioned.
            if p.net.is_online() || has_network(&p).await {
                Timer::after(WINDOW).await;
                log::info!("provision: window closed");
                close_window();
            } else {
                core::future::pending::<()>().await;
            }
        };
        select(run_ble(bt.reborrow(), &p), window).await;
        closed().await;
        // Leave the portal's client time to see the outcome.
        Timer::after_secs(10).await;
        PORTAL.lock(|s| *s.borrow_mut() = PortalStatus::Idle);
        iotkit_net::esp::access_point(false).await;
    }
}

/// Improv over Bluetooth until the window closes (Bluetooth is off afterwards).
async fn run_ble(bt: BT<'_>, p: &Provisioner) {
    let connector = match BleConnector::new(bt, Default::default()) {
        Ok(connector) => connector,
        Err(e) => {
            log::error!("provision: Bluetooth init failed: {e:?}");
            return closed().await;
        }
    };
    let controller: ExternalController<_, 20> = ExternalController::new(connector);
    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address()).build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let gap = GapConfig::Peripheral(PeripheralConfig { name: p.net.hostname, appearance: &appearance::UNKNOWN });
    let server = match Server::new_with_config(gap) {
        Ok(server) => server,
        Err(e) => {
            log::error!("provision: GATT server: {e:?}");
            return closed().await;
        }
    };
    log::info!("provision: Improv over Bluetooth as '{}'", p.net.hostname);
    match select(runner.run(), serve(&stack, &mut peripheral, &server, p)).await {
        Either::First(Err(e)) => {
            log::warn!("provision: Bluetooth stopped: {e:?}");
            closed().await;
        }
        _ => log::info!("provision: Bluetooth off"),
    }
}

async fn has_network(p: &Provisioner) -> bool {
    matches!(p.store.lock().await.load::<WifiCredentials>().await, Ok(Some(_)))
}

/// A static random address from the MAC address: clients see the same device across restarts.
fn address() -> Address {
    let mac = esp_hal::efuse::base_mac_address();
    let mut bytes = [0u8; 6];
    for (b, m) in bytes.iter_mut().zip(mac.as_bytes().iter().rev()) {
        *b = *m;
    }
    bytes[5] |= 0xc0;
    Address::random(bytes)
}

async fn closed() {
    while window_open() {
        CHANGED.wait().await;
    }
}

/// Advertises and serves clients until the window closes.
async fn serve<C: Controller + ControllerCmdAsync<LeConnUpdate> + ControllerCmdSync<LeReadLocalSupportedFeatures>>(
    stack: &Stack<'_, C, DefaultPacketPool>,
    peripheral: &mut Peripheral<'_, C, DefaultPacketPool>,
    server: &Server<'_>,
    p: &Provisioner,
) {
    loop {
        let advertiser = match advertise(peripheral, p.net.hostname).await {
            Ok(advertiser) => advertiser,
            Err(e) => {
                log::warn!("provision: cannot advertise: {e:?}");
                Timer::after_secs(1).await;
                continue;
            }
        };
        log::info!("provision: advertising");
        let conn = match select(advertiser.accept(), closed()).await {
            Either::First(Ok(conn)) => conn,
            Either::First(Err(e)) => {
                log::warn!("provision: Bluetooth connection failed: {e:?}");
                continue;
            }
            Either::Second(()) => return,
        };
        // Joining a Wi-Fi network starves Bluetooth for a while (the radio is shared): ask for a
        // supervision timeout long enough to keep the link through it.
        let params = RequestedConnParams {
            min_connection_interval: Duration::from_millis(30),
            max_connection_interval: Duration::from_millis(50),
            max_latency: 0,
            supervision_timeout: Duration::from_secs(6),
            ..Default::default()
        };
        if let Err(e) = conn.update_connection_params(stack, &params).await {
            log::warn!("provision: connection parameters refused: {e:?}");
        }
        let Ok(conn) = conn.with_attribute_server(server) else { continue };
        log::info!("provision: Bluetooth client connected");
        let _ = server.improv.state.set(server, &(State::Authorized as u8));
        let _ = server.improv.error.set(server, &(Error::None as u8));
        let mut command = BleCommand::new();
        loop {
            let event = match select(conn.next(), closed()).await {
                Either::First(event) => event,
                Either::Second(()) => {
                    // Leave the client time to read the result.
                    Timer::after_secs(2).await;
                    return;
                }
            };
            let event = match event {
                GattConnectionEvent::Disconnected { reason } => {
                    log::info!("provision: Bluetooth client left ({reason:?})");
                    break;
                }
                GattConnectionEvent::Gatt { event } => event,
                _ => continue,
            };
            let packet = match &event {
                GattEvent::Write(write) if write.handle() == server.improv.rpc_command.handle => {
                    write.with_data(|_, data| command.push(data))
                }
                _ => None,
            };
            if let Ok(reply) = event.accept() {
                reply.send().await;
            }
            if let Some(packet) = packet {
                handle(server, &conn, p, &packet).await;
            }
        }
    }
}

async fn advertise<'p, C: Controller>(
    peripheral: &mut Peripheral<'p, C, DefaultPacketPool>,
    name: &str,
) -> Result<Advertiser<'p, C, DefaultPacketPool>, BleHostError<C::Error>> {
    let uuid = SERVICE_UUID.to_le_bytes();
    let data = service_data(State::Authorized, CAPABILITIES);
    let mut adv = [0u8; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids128(&[uuid]),
            AdStructure::ServiceData16 { uuid: SERVICE_DATA_UUID.to_le_bytes(), data: &data },
        ],
        &mut adv,
    )?;
    let mut scan = [0u8; 31];
    let scan_len = AdStructure::encode_slice(&[AdStructure::CompleteLocalName(name.as_bytes())], &mut scan)?;
    peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected { adv_data: &adv[..adv_len], scan_data: &scan[..scan_len] },
        )
        .await
}

async fn handle(server: &Server<'_>, conn: &GattConnection<'_, '_, DefaultPacketPool>, p: &Provisioner, packet: &[u8]) {
    let notify_state = async |state: State| {
        let _ = server.improv.state.notify(conn, &(state as u8), true).await;
    };
    let notify_error = async |error: Error| {
        let _ = server.improv.error.notify(conn, &(error as u8), true).await;
    };
    let notify_result = async |id: u8, strings: &[&str]| {
        let mut out = [0; RPC_MAX];
        let bytes = rpc_result(Transport::Ble, id, strings, &mut out);
        let value = heapless::Vec::from_slice(bytes).unwrap_or_default();
        let _ = server.improv.rpc_result.notify(conn, &value, true).await;
    };
    notify_error(Error::None).await;
    match parse_rpc(Transport::Ble, packet) {
        Ok(Rpc::WifiSettings { ssid, password }) => {
            notify_state(State::Provisioning).await;
            if p.provision(ssid, password).await {
                notify_result(command::WIFI_SETTINGS, &[&p.url()]).await;
                notify_state(State::Provisioned).await;
            } else {
                notify_state(State::Authorized).await;
                notify_error(Error::UnableToConnect).await;
            }
        }
        Ok(Rpc::Identify) => IDENTIFY.signal(()),
        Ok(Rpc::DeviceInfo) => notify_result(command::DEVICE_INFO, &device_info(p)).await,
        Ok(Rpc::GetState) => notify_error(Error::UnknownRpc).await,
        Err(e) => notify_error(e).await,
    }
}

/// DHCP server of the setup access point.
#[embassy_executor::task]
pub async fn dhcp_task(p: Provisioner) {
    let (mut rx_meta, mut tx_meta) = ([PacketMetadata::EMPTY; 2], [PacketMetadata::EMPTY; 2]);
    let (mut rx, mut tx) = ([0u8; 600], [0u8; 600]);
    let mut socket = UdpSocket::new(p.net.ap_stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    if let Err(e) = socket.bind(67) {
        log::error!("provision: DHCP bind failed: {e:?}");
        return;
    }
    let mut leases = dhcp::Leases::new();
    let (mut buf, mut out) = ([0u8; 600], [0u8; 576]);
    loop {
        let Ok((n, _)) = socket.recv_from(&mut buf).await else { continue };
        if let Some(len) = leases.respond(&buf[..n], &mut out) {
            let to = IpEndpoint::new(Ipv4Addr::BROADCAST.into(), 68);
            if let Err(e) = socket.send_to(&out[..len], to).await {
                log::warn!("provision: DHCP reply failed: {e:?}");
            }
        }
    }
}

/// Captive DNS of the setup access point.
#[embassy_executor::task]
pub async fn dns_task(p: Provisioner) {
    let (mut rx_meta, mut tx_meta) = ([PacketMetadata::EMPTY; 4], [PacketMetadata::EMPTY; 4]);
    let (mut rx, mut tx) = ([0u8; 1024], [0u8; 1024]);
    let mut socket = UdpSocket::new(p.net.ap_stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    if let Err(e) = socket.bind(53) {
        log::error!("provision: DNS bind failed: {e:?}");
        return;
    }
    let (mut buf, mut out) = ([0u8; 512], [0u8; 512]);
    loop {
        let Ok((n, meta)) = socket.recv_from(&mut buf).await else { continue };
        if let Some(len) = dns::answer(&buf[..n], AP_ADDRESS, &mut out) {
            let _ = socket.send_to(&out[..len], meta.endpoint).await;
        }
    }
}

/// What the portal shows on `/`.
#[derive(Clone)]
enum PortalStatus {
    Idle,
    Trying(String<32>),
    Joined(String<32>),
    Failed(String<32>),
}

static PORTAL: embassy_sync::blocking_mutex::Mutex<CriticalSectionRawMutex, core::cell::RefCell<PortalStatus>> =
    embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(PortalStatus::Idle));
static PORTAL_JOB: Signal<CriticalSectionRawMutex, (String<32>, String<64>)> = Signal::new();

/// Joins the networks sent through the portal (outside the HTTP exchange: joining restarts the
/// access point).
#[embassy_executor::task]
pub async fn portal_job_task(p: Provisioner) {
    loop {
        let (ssid, password) = PORTAL_JOB.wait().await;
        // Let the "joining" page reach the client first.
        Timer::after_millis(300).await;
        let status =
            if p.provision(&ssid, &password).await { PortalStatus::Joined(ssid) } else { PortalStatus::Failed(ssid) };
        PORTAL.lock(|s| *s.borrow_mut() = status);
    }
}

/// The setup page on the access point (port 80).
#[embassy_executor::task]
pub async fn portal_task(p: Provisioner) {
    let (mut rx, mut tx) = ([0u8; 1536], [0u8; 2560]);
    loop {
        let mut socket = TcpSocket::new(p.net.ap_stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(10)));
        if socket.accept(80).await.is_err() {
            continue;
        }
        let mut buf = [0u8; 1536];
        let Some((head_len, total)) = read_request(&mut socket, &mut buf).await else {
            socket.abort();
            continue;
        };
        let head = core::str::from_utf8(&buf[..head_len]).unwrap_or("");
        let line = head.lines().next().unwrap_or("");
        let (method, target) = iotkit_net::http::parse_request_line(line).unwrap_or(("", "/"));
        let url = p.url();
        let page: String<2048> = match portal::route(method, iotkit_net::http::target_path(target)) {
            portal::Route::Redirect => {
                let _ = socket
                    .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://192.168.4.1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                finish(&mut socket).await;
                continue;
            }
            portal::Route::Page => {
                let status = PORTAL.lock(|s| s.borrow().clone());
                match &status {
                    PortalStatus::Idle => portal::page(p.net.hostname, portal::Notice::None),
                    PortalStatus::Trying(ssid) => portal::page(p.net.hostname, portal::Notice::Trying { ssid }),
                    PortalStatus::Joined(ssid) => {
                        portal::page(p.net.hostname, portal::Notice::Joined { ssid, url: &url })
                    }
                    PortalStatus::Failed(ssid) => portal::page(p.net.hostname, portal::Notice::Failed { ssid }),
                }
            }
            portal::Route::Connect => match portal::parse_form(&buf[head_len + 4..total]) {
                Some((ssid, password)) => {
                    let page = portal::page(p.net.hostname, portal::Notice::Trying { ssid: &ssid });
                    PORTAL.lock(|s| *s.borrow_mut() = PortalStatus::Trying(ssid.clone()));
                    PORTAL_JOB.signal((ssid, password));
                    page
                }
                None => portal::page(p.net.hostname, portal::Notice::None),
            },
        };
        let mut head: String<160> = String::new();
        let _ = write!(
            head,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            page.len()
        );
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(page.as_bytes()).await;
        finish(&mut socket).await;
    }
}

/// Reads a request head and its body (by `Content-Length`); returns the head length (without
/// the blank line) and the bytes read.
async fn read_request(socket: &mut TcpSocket<'_>, buf: &mut [u8; 1536]) -> Option<(usize, usize)> {
    let mut filled = 0;
    loop {
        let n = with_timeout(Duration::from_secs(5), socket.read(&mut buf[filled..])).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        filled += n;
        if let Some(end) = buf[..filled].windows(4).position(|w| w == b"\r\n\r\n") {
            let head = core::str::from_utf8(&buf[..end]).ok()?;
            let total = end + 4 + portal::content_length(head).unwrap_or(0);
            if total > buf.len() {
                return None;
            }
            while filled < total {
                let n = with_timeout(Duration::from_secs(5), socket.read(&mut buf[filled..total])).await.ok()?.ok()?;
                if n == 0 {
                    return None;
                }
                filled += n;
            }
            return Some((end, total));
        }
        if filled == buf.len() {
            return None;
        }
    }
}

/// Flushes, closes, and waits for the client's FIN (dropping the socket earlier leaves it hanging).
async fn finish(socket: &mut TcpSocket<'_>) {
    let _ = socket.flush().await;
    socket.close();
    let _ = with_timeout(Duration::from_secs(2), async {
        let mut rest = [0u8; 64];
        while let Ok(n) = socket.read(&mut rest).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
}

/// How long the BOOT button has been held, for the app's LED: 0 not held or under 3 s,
/// 1 past [`PROVISION_MS`](crate::gesture::PROVISION_MS), 2 past
/// [`FACTORY_RESET_MS`](crate::gesture::FACTORY_RESET_MS).
pub static HELD: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// BOOT button (active low): hold 3 s to open provisioning, 10 s to forget every setting and
/// Matter pairing.
#[embassy_executor::task]
pub async fn button_task(button: esp_hal::gpio::Input<'static>, store: &'static SharedStore) {
    use crate::gesture::{FACTORY_RESET_MS, Gesture, PROVISION_MS, on_release};
    loop {
        while button.is_high() {
            Timer::after_millis(50).await;
        }
        let pressed = embassy_time::Instant::now();
        while button.is_low() {
            let held = pressed.elapsed().as_millis() as u32;
            HELD.store(u8::from(held >= PROVISION_MS) + u8::from(held >= FACTORY_RESET_MS), Ordering::Relaxed);
            Timer::after_millis(50).await;
        }
        HELD.store(0, Ordering::Relaxed);
        match on_release(pressed.elapsed().as_millis() as u32) {
            Gesture::Press => {}
            Gesture::Provision => {
                log::info!("provision: BOOT held, window open");
                open_window();
            }
            Gesture::FactoryReset => {
                log::warn!("provision: BOOT held 10 s, erasing all settings");
                if let Err(e) = iotkit_config::esp::factory_reset(store).await {
                    log::error!("provision: factory reset failed: {e}");
                }
                iotkit_diag::esp::restart().await;
            }
        }
    }
}
