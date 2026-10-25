//! Improv on the chip: the serial front-end (fed by the app's console) and the Bluetooth LE GATT
//! server (only while a provisioning window is open).

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};

use core::net::Ipv4Addr;
#[cfg(feature = "improv-ble")]
use embassy_futures::select::select;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

use crate::config::esp::SharedStore;
use crate::net::{
    WifiCredentials,
    esp::{AP_ADDRESS, Net},
};
use embassy_net::{
    IpEndpoint,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
#[cfg(feature = "improv-ble")]
use esp_hal::peripherals::BT;
use esp_println::Printer;
use heapless::String;

use crate::provision::improv::{
    Error, FRAME_MAX, RPC_MAX, Rpc, SerialDecoder, State, Transport, command, frame, parse_rpc, rpc_result,
    serial_frame,
};
use crate::provision::{dhcp, dns, portal};

#[cfg(feature = "improv-ble")]
mod ble;

/// How long a requested window stays open (none is needed while no network is stored).
pub const WINDOW: Duration = Duration::from_secs(600);

/// What the device tells clients about itself.
#[derive(Clone, Copy)]
pub struct DeviceInfo {
    /// The firmware name (package name).
    pub firmware: &'static str,
    /// The firmware version.
    pub version: &'static str,
    /// The chip (`ESP32-C6`, ...).
    pub chip: &'static str,
}

/// Everything provisioning needs.
#[derive(Clone, Copy)]
pub struct Provisioner {
    /// The network to configure.
    pub net: Net,
    /// Where the network is stored once it works.
    pub store: &'static SharedStore,
    /// What Improv clients are told.
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
    crate::status::notify();
}

/// Whether a provisioning window is open.
pub fn window_open() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub(crate) fn close_window() {
    ACTIVE.store(false, Ordering::Relaxed);
    CHANGED.signal(());
    crate::status::notify();
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
        if !crate::net::esp::join(credentials.clone()).await {
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
    /// A decoder with nothing received yet.
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

/// Runs the provisioning windows: the setup access point (`softap`) and Improv over Bluetooth LE
/// (`ble`) while a window is open, both off otherwise.
#[embassy_executor::task]
pub async fn window_task(#[cfg(feature = "improv-ble")] mut bt: BT<'static>, p: Provisioner) {
    // Once: its attribute values are static cells, which cannot be made twice.
    #[cfg(feature = "improv-ble")]
    let server = ble::server(&p);
    loop {
        while !window_open() {
            CHANGED.wait().await;
        }
        #[cfg(feature = "softap")]
        crate::net::esp::access_point(true).await;
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
        #[cfg(feature = "improv-ble")]
        select(ble::run_ble(bt.reborrow(), &p, server.as_ref()), window).await;
        #[cfg(not(feature = "improv-ble"))]
        window.await;
        closed().await;
        #[cfg(feature = "softap")]
        {
            // Leave the portal's client time to see the outcome.
            Timer::after_secs(10).await;
            PORTAL.lock(|s| *s.borrow_mut() = PortalStatus::Idle);
            crate::net::esp::access_point(false).await;
        }
    }
}

async fn has_network(p: &Provisioner) -> bool {
    matches!(p.store.lock().await.load::<WifiCredentials>().await, Ok(Some(_)))
}

async fn closed() {
    while window_open() {
        CHANGED.wait().await;
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
        let (method, target) = crate::net::http::parse_request_line(line).unwrap_or(("", "/"));
        let url = p.url();
        let page: String<2048> = match portal::route(method, crate::net::http::target_path(target)) {
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

pub use crate::provision::gesture::{HELD, wait_press};

/// BOOT button (active low): runs the [gestures](crate::provision::gesture); a factory reset forgets every
/// setting and Matter pairing.
#[embassy_executor::task]
pub async fn button_task(mut button: esp_hal::gpio::Input<'static>, store: &'static SharedStore) {
    use crate::provision::gesture::Gesture;
    loop {
        match crate::provision::gesture::next(&mut button, None).await {
            Gesture::Press => crate::provision::gesture::pressed(),
            Gesture::Provision => {
                log::info!("provision: button held, window open");
                open_window();
            }
            Gesture::FactoryReset => factory_reset(store).await,
        }
    }
}

/// The button held 10 s: forgets every setting and Matter pairing, and restarts.
pub(crate) async fn factory_reset(store: &SharedStore) -> ! {
    log::warn!("provision: button held 10 s, erasing all settings");
    if let Err(e) = crate::config::esp::factory_reset(store).await {
        log::error!("provision: factory reset failed: {e}");
    }
    crate::diag::esp::restart().await
}
