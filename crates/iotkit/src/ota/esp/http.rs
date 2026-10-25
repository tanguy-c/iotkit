//! Updates over HTTP: the port-80 endpoint (`cargo xtask ota push`), and the installs and
//! checks Home Assistant and the console ask for.

use core::fmt::Write as _;

use super::{Claim, Firmware, Image, Install, OtaError};
use crate::config::esp::SharedStore;
use crate::image::{Manifest, Version};
use crate::net::{
    esp::{GetError, Net, Sink, get, get_from},
    http,
};
#[cfg(any(not(feature = "ha-update"), not(feature = "ota-push")))]
use embassy_futures::select::{Either, select};
#[cfg(feature = "ha-update")]
use embassy_futures::select::{Either3, select3};
use embassy_net::tcp::TcpSocket;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex, signal::Signal};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
use heapless::String;

/// Largest manifest accepted.
const MAX_MANIFEST: usize = 1024;
/// Downloads of the image, the first included, before giving up.
const ATTEMPTS: u32 = 5;
/// Port of the push endpoint.
pub const PORT: u16 = 80;

/// An install is asked for, running, or done and waiting for the restart (a deep-sleeping
/// device does not sleep then).
pub fn installing() -> bool {
    super::BUSY.load(crate::ota::atomic::Ordering::Acquire) || REQUEST.signaled()
}
/// An install asked for by [`request_install`] (or a download by `request_stage`), run by
/// [`serve_task`] (or `install_task`).
static REQUEST: Signal<CriticalSectionRawMutex, Request> = Signal::new();

struct Request {
    url: String<{ http::MAX_URL }>,
    /// Kept downloaded in the spare slot, not selected ([`Install::stage`]).
    stage: bool,
}
static RESULT: Signal<CriticalSectionRawMutex, Result<Version, OtaError>> = Signal::new();
/// One [`request_install`] at a time.
static REQUESTING: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

/// Has [`serve_task`] or `install_task` (one must run) install the update whose manifest is at
/// `manifest_url`. On success the new firmware is selected: restart to boot it (on trial). Only
/// that task runs an install, so its large buffers exist once.
pub async fn request_install(manifest_url: &str) -> Result<Version, OtaError> {
    request(manifest_url, false).await
}

/// The same, but the image is kept downloaded, not selected: [`apply_staged`](super::apply_staged)
/// selects it later.
#[cfg(esp_scheduled)]
pub async fn request_stage(manifest_url: &str) -> Result<Version, OtaError> {
    request(manifest_url, true).await
}

async fn request(manifest_url: &str, stage: bool) -> Result<Version, OtaError> {
    let Ok(_one) = REQUESTING.try_lock() else { return Err(OtaError::Busy) };
    if super::busy() {
        return Err(OtaError::Busy);
    }
    let url = String::try_from(manifest_url).map_err(|_| OtaError::Download(GetError::Url(http::Error::Url)))?;
    RESULT.reset();
    REQUEST.signal(Request { url, stage });
    RESULT.wait().await
}

/// A newer release, from its manifest.
#[cfg(feature = "ha-update")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: Version,
    /// What it changes (empty: none).
    pub summary: String<{ crate::image::MAX_SUMMARY_LEN }>,
    /// Where its notes are (empty: none).
    pub release_url: String<{ crate::image::MAX_RELEASE_URL_LEN }>,
}

/// A check asked for by [`request_check`], run by [`serve_task`] (or `install_task`).
#[cfg(feature = "ha-update")]
static CHECK: Signal<CriticalSectionRawMutex, String<{ http::MAX_URL }>> = Signal::new();
#[cfg(feature = "ha-update")]
static CHECKED: Signal<CriticalSectionRawMutex, Result<Option<Release>, OtaError>> = Signal::new();

/// Has [`serve_task`] or `install_task` fetch and verify the manifest at `manifest_url`: the
/// release if it is an update for this firmware, `None` if it is not newer.
#[cfg(feature = "ha-update")]
pub async fn request_check(manifest_url: &str) -> Result<Option<Release>, OtaError> {
    let Ok(_one) = REQUESTING.try_lock() else { return Err(OtaError::Busy) };
    let url = String::try_from(manifest_url).map_err(|_| OtaError::Download(GetError::Url(http::Error::Url)))?;
    CHECKED.reset();
    CHECK.signal(url);
    CHECKED.wait().await
}

#[cfg(feature = "ha-update")]
async fn check(net: Net, fw: Firmware, manifest_url: &str) -> Result<Option<Release>, OtaError> {
    let mut collect = Collect { buf: [0; MAX_MANIFEST], len: 0 };
    get(net.stack, manifest_url, &mut collect).await.map_err(|e| OtaError::Download(drop_sink(e)))?;
    let manifest = Manifest::parse(&collect.buf[..collect.len]).map_err(OtaError::Manifest)?;
    manifest.verify(fw.key).map_err(OtaError::Manifest)?;
    match manifest.check_update(fw.product, fw.version) {
        Ok(()) => Ok(Some(Release {
            version: manifest.version,
            // Both fit: the manifest's lengths are checked when parsed.
            summary: String::try_from(manifest.summary).unwrap_or_default(),
            release_url: String::try_from(manifest.release_url).unwrap_or_default(),
        })),
        Err(crate::image::Error::NotNewer) => Ok(None),
        Err(e) => Err(OtaError::Manifest(e)),
    }
}

/// Downloads and installs the update described by the manifest at `manifest_url`. On success
/// the new firmware is selected (restart to boot it, on trial), or with `stage` kept unselected.
async fn install(
    net: Net,
    store: &SharedStore,
    fw: Firmware,
    manifest_url: &str,
    stage: bool,
) -> Result<Version, OtaError> {
    #[cfg(not(esp_stage))]
    let _ = stage;
    // Before the first await: `REQUEST` is consumed, and a sleepy device must not sleep now.
    let claim = Claim::take()?;
    let mut collect = Collect { buf: [0; MAX_MANIFEST], len: 0 };
    get(net.stack, manifest_url, &mut collect).await.map_err(|e| OtaError::Download(drop_sink(e)))?;
    let manifest = Manifest::parse(&collect.buf[..collect.len]).map_err(OtaError::Manifest)?;
    manifest.verify(fw.key).map_err(OtaError::Manifest)?;
    manifest.check_update(fw.product, fw.version).map_err(OtaError::Manifest)?;
    let url = http::Url::parse(manifest_url)
        .and_then(|base| base.join(manifest.url))
        .map_err(|e| OtaError::Download(GetError::Url(e)))?;
    let mut sector = [0xff; super::SECTOR];
    let mut install = Install::begin(claim, Image::Signed(&manifest), &mut sector)?;
    // Its slot is written over now.
    #[cfg(esp_stage)]
    super::forget_staged(store).await;

    let started = embassy_time::Instant::now();
    let mut downloaded = Ok(());
    for attempt in 1..=ATTEMPTS {
        downloaded = get_from(net.stack, &url, install.received(), &mut install).await;
        match downloaded {
            // A dropped connection: carry on from where it stopped.
            Err(GetError::Truncated { .. } | GetError::Connect | GetError::Io) if attempt < ATTEMPTS => {
                log::warn!(
                    "ota: download interrupted at {} of {} bytes ({:?}), resuming",
                    install.received(),
                    manifest.size,
                    downloaded
                );
                Timer::after_secs(1).await;
            }
            _ => break,
        }
    }
    let (size, ms) = (install.received(), started.elapsed().as_millis().max(1) as u32);
    log::info!("ota: {size} bytes in {}.{:01} s ({} KiB/s)", ms / 1000, ms % 1000 / 100, size / ms * 1000 / 1024);
    match downloaded {
        #[cfg(esp_stage)]
        Ok(()) if stage => install.stage(store, manifest.sha256).await.map(|s| s.version()),
        Ok(()) => install.commit(store).await,
        Err(GetError::Sink(e)) => Err(e),
        Err(e) => Err(OtaError::Download(drop_sink(e))),
    }
}

fn drop_sink<E>(e: GetError<E>) -> GetError<()> {
    match e {
        GetError::Url(e) => GetError::Url(e),
        GetError::Dns => GetError::Dns,
        GetError::Connect => GetError::Connect,
        #[cfg(feature = "tls")]
        GetError::Tls => GetError::Tls,
        GetError::Io => GetError::Io,
        GetError::BadResponse(e) => GetError::BadResponse(e),
        GetError::Status(s) => GetError::Status(s),
        GetError::Truncated { received, expected } => GetError::Truncated { received, expected },
        GetError::Sink(_) => GetError::Sink(()),
    }
}

/// Keeps the manifest in memory.
struct Collect {
    buf: [u8; MAX_MANIFEST],
    len: usize,
}

impl Sink for Collect {
    type Error = ();
    fn begin(&mut self, len: Option<u32>) -> Result<(), ()> {
        if len.is_some_and(|l| l as usize > MAX_MANIFEST) { Err(()) } else { Ok(()) }
    }
    async fn data(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let dst = self.buf.get_mut(self.len..self.len + bytes.len()).ok_or(())?;
        dst.copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

impl Sink for Install<'_> {
    type Error = OtaError;

    /// `len`: what is left of the image (all of it, or the rest after a resume).
    fn begin(&mut self, len: Option<u32>) -> Result<(), OtaError> {
        self.expect(len)
    }

    async fn data(&mut self, bytes: &[u8]) -> Result<(), OtaError> {
        self.write(bytes).await
    }
}

/// HTTP endpoint on port 80, and the installs of [`request_install`]:
/// - `GET /` answers `<product> <version>`;
/// - `GET /ota?manifest=<url>` installs that update, answers the result, then restarts.
#[embassy_executor::task]
pub async fn serve_task(net: Net, store: &'static SharedStore, fw: Firmware) {
    let (mut rx, mut tx) = ([0u8; 1024], [0u8; 512]);
    loop {
        let mut socket = TcpSocket::new(net.stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(10)));
        #[cfg(not(feature = "ha-update"))]
        match select(socket.accept(PORT), REQUEST.wait()).await {
            Either::First(Ok(())) => {}
            Either::First(Err(_)) => continue,
            Either::Second(r) => {
                drop(socket);
                RESULT.signal(install(net, store, fw, &r.url, r.stage).await);
                continue;
            }
        }
        #[cfg(feature = "ha-update")]
        match select3(socket.accept(PORT), REQUEST.wait(), CHECK.wait()).await {
            Either3::First(Ok(())) => {}
            Either3::First(Err(_)) => continue,
            Either3::Second(r) => {
                drop(socket);
                RESULT.signal(install(net, store, fw, &r.url, r.stage).await);
                continue;
            }
            Either3::Third(url) => {
                drop(socket);
                CHECKED.signal(check(net, fw, &url).await);
                continue;
            }
        }
        let mut buf = [0u8; 512];
        let mut filled = 0;
        let head_end = loop {
            match with_timeout(Duration::from_secs(5), socket.read(&mut buf[filled..])).await {
                Ok(Ok(n)) if n > 0 => filled += n,
                _ => break None,
            }
            if let Some(end) = buf[..filled].windows(4).position(|w| w == b"\r\n\r\n") {
                break Some(end);
            }
            if filled == buf.len() {
                break None;
            }
        };
        let Some(end) = head_end else {
            socket.abort();
            continue;
        };
        let line = core::str::from_utf8(&buf[..end]).ok().and_then(|h| h.lines().next()).unwrap_or("");
        let mut url: String<{ http::MAX_URL }> = String::new();
        let mut reply: String<192> = String::new();
        let (status, restart) = match http::parse_request_line(line) {
            Some(("GET", "/")) => {
                let _ = writeln!(reply, "{} {}", fw.product, fw.version);
                ("200 OK", false)
            }
            Some(("GET", target)) if http::target_path(target) == "/ota" => {
                match http::query_param(target, "manifest").map(|m| url.push_str(m)) {
                    Some(Ok(())) => {
                        log::info!("ota: push from the network: {url}");
                        // The client waits, silent, for the whole install.
                        socket.set_timeout(None);
                        match install(net, store, fw, &url, false).await {
                            Ok(v) => {
                                let _ = writeln!(reply, "installed {v}, restarting");
                                ("200 OK", true)
                            }
                            Err(e) => {
                                log::error!("ota: {e}");
                                let _ = writeln!(reply, "{e}");
                                ("500 Internal Server Error", false)
                            }
                        }
                    }
                    _ => {
                        let _ = reply.push_str("usage: /ota?manifest=http://host:port/path/manifest.json\n");
                        ("400 Bad Request", false)
                    }
                }
            }
            _ => {
                let _ = reply.push_str("not found\n");
                ("404 Not Found", false)
            }
        };
        let mut head: String<128> = String::new();
        let _ = write!(head, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(reply.as_bytes()).await;
        let _ = socket.flush().await;
        socket.close();
        // Wait for the client's FIN: dropping the socket earlier would leave it waiting.
        let _ = with_timeout(Duration::from_secs(2), async {
            let mut rest = [0u8; 64];
            while let Ok(n) = socket.read(&mut rest).await {
                if n == 0 {
                    break;
                }
            }
        })
        .await;
        if restart {
            // Let the reply leave first.
            Timer::after_millis(500).await;
            crate::diag::esp::restart().await;
        }
    }
}

/// The installs and checks of [`request_install`] and [`request_check`] without the port-80
/// endpoint: `ha-update` without `ota-push`.
#[cfg(all(feature = "ha-update", not(feature = "ota-push")))]
#[embassy_executor::task]
pub async fn install_task(net: Net, store: &'static SharedStore, fw: Firmware) {
    loop {
        match select(REQUEST.wait(), CHECK.wait()).await {
            Either::First(r) => RESULT.signal(install(net, store, fw, &r.url, r.stage).await),
            Either::Second(url) => CHECKED.signal(check(net, fw, &url).await),
        }
    }
}
