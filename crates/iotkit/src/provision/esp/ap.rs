//! The setup access point's servers (`softap`): DHCP, a captive DNS and the setup page, only
//! while a provisioning window is open.

use core::{fmt::Write as _, net::Ipv4Addr};

use embassy_net::{
    IpEndpoint,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
use heapless::String;

use super::Provisioner;
use crate::net::esp::AP_ADDRESS;
use crate::provision::{dhcp, dns, portal};

/// What the setup access point's servers work in, in the work area.
pub(crate) struct ApBuffers {
    dhcp: DhcpBuffers,
    dns: DnsBuffers,
    portal: PortalBuffers,
}

/// The setup access point's servers, until dropped. The portal starts with no notice.
pub(super) async fn servers(p: &Provisioner, buffers: &mut ApBuffers) {
    PORTAL.lock(|s| *s.borrow_mut() = PortalStatus::Idle);
    let ApBuffers { dhcp: d, dns: n, portal: w } = buffers;
    embassy_futures::join::join3(dhcp(p, d), dns(p, n), portal(p, w)).await;
}

struct DhcpBuffers {
    rx: [u8; 600],
    tx: [u8; 600],
    buf: [u8; 600],
    out: [u8; 576],
}

/// DHCP server of the setup access point.
async fn dhcp(p: &Provisioner, b: &mut DhcpBuffers) {
    let (mut rx_meta, mut tx_meta) = ([PacketMetadata::EMPTY; 2], [PacketMetadata::EMPTY; 2]);
    let DhcpBuffers { rx, tx, buf, out } = b;
    let mut socket = UdpSocket::new(p.net.ap_stack, &mut rx_meta, rx, &mut tx_meta, tx);
    if let Err(e) = socket.bind(67) {
        log::error!("provision: DHCP bind failed: {e:?}");
        return;
    }
    let mut leases = dhcp::Leases::new();
    loop {
        let Ok((n, _)) = socket.recv_from(buf).await else { continue };
        if let Some(len) = leases.respond(&buf[..n], out) {
            let to = IpEndpoint::new(Ipv4Addr::BROADCAST.into(), 68);
            if let Err(e) = socket.send_to(&out[..len], to).await {
                log::warn!("provision: DHCP reply failed: {e:?}");
            }
        }
    }
}

struct DnsBuffers {
    rx: [u8; 1024],
    tx: [u8; 1024],
    buf: [u8; 512],
    out: [u8; 512],
}

/// Captive DNS of the setup access point.
async fn dns(p: &Provisioner, b: &mut DnsBuffers) {
    let (mut rx_meta, mut tx_meta) = ([PacketMetadata::EMPTY; 4], [PacketMetadata::EMPTY; 4]);
    let DnsBuffers { rx, tx, buf, out } = b;
    let mut socket = UdpSocket::new(p.net.ap_stack, &mut rx_meta, rx, &mut tx_meta, tx);
    if let Err(e) = socket.bind(53) {
        log::error!("provision: DNS bind failed: {e:?}");
        return;
    }
    loop {
        let Ok((n, meta)) = socket.recv_from(buf).await else { continue };
        if let Some(len) = dns::answer(&buf[..n], AP_ADDRESS, out) {
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
/// access point). Its own task, not one of the [`servers`]: a join can outlast the window, and
/// must not be cut short.
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

struct PortalBuffers {
    rx: [u8; 1536],
    tx: [u8; 2560],
    buf: [u8; 1536],
}

/// The setup page on the access point (port 80).
async fn portal(p: &Provisioner, b: &mut PortalBuffers) {
    let PortalBuffers { rx, tx, buf } = b;
    loop {
        let mut socket = TcpSocket::new(p.net.ap_stack, rx, tx);
        socket.set_timeout(Some(Duration::from_secs(10)));
        if socket.accept(80).await.is_err() {
            continue;
        }
        let Some((head_len, total)) = read_request(&mut socket, buf).await else {
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
