//! Updates over Matter (`matter-ota`): the OTA Software Update Requestor on endpoint 0. A
//! provider announces an update (Home Assistant's Matter server does), or the device asks its
//! default providers once a day; the file comes over BDX into the spare slot, then boots on trial
//! like any update ([`ota`](crate::ota)), and the provider hears of it once it runs.
//!
//! The file is a Matter OTA file ([`image::matter`](crate::image::matter)) whose payload starts
//! with iotkit's signed manifest: `cargo iotkit ota build` writes it. With `matter-ota-unsigned`
//! the payload is the bare app image, checked by the header's SHA-256 only.
//!
//! With `IOTKIT_UPDATE_TIME` (`cfg(matter_scheduled)`) an update of the daily checks is downloaded
//! at once and installed at that time, in the controller's time zone (Time Synchronization:
//! `SetUTCTime`, `SetTimeZone`). One a controller announces (a click) installs at once. Without a
//! time since the device started (a restart), it waits for one, an hour at most.

use core::num::NonZeroU8;

use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use heapless::{String, Vec};
use rs_matter_embassy::matter::{
    bdx::{BdxDownloadInitiator, BdxReader},
    dm::{
        AsyncHandler, Cluster, Dataver, HandlerContext, InvokeContext, InvokeReply, LifecycleOp, MatchContext,
        ReadContext, ReadReply, WriteContext,
        clusters::{
            ota_prov::{self, DownloadProtocolEnum, OtaApplyOutcome, StatusEnum},
            ota_req::{
                ClusterHandler as _, OtaRequestorHandler, OtaState, OtaUpdate, Provider, Providers, parse_bdx_url,
            },
        },
        endpoints::ROOT_ENDPOINT_ID,
    },
    error::{Error, ErrorCode},
    transport::exchange::Exchange,
};
use serde::{Deserialize, Serialize};
use static_cell::StaticCell;

use crate::Firmware;
use crate::config::{Setting, esp::SharedStore, keys};
use crate::health::{HEALTH, MATTER};
use crate::image::matter::{self as file, Header, Prefix};
use crate::image::{Digest, Sha256};
use crate::ota::esp::{Claim, Image, Install, OtaError, Sector};

/// The OTA Requestor cluster, for the root endpoint.
pub const CLUSTER: Cluster<'static> = OtaRequestorHandler::CLUSTER;
/// The OTA Provider cluster this endpoint is a client of.
pub const PROVIDER_CLUSTER: u32 = ota_prov::FULL_CLUSTER.id;

/// The root endpoint of [`root_endpoint!`](rs_matter_embassy::matter::root_endpoint), with the OTA
/// Requestor: `$net` is `thread` or `eth`.
macro_rules! root_endpoint {
    ($net:ident) => {{
        const ROOT: rs_matter_embassy::matter::dm::Endpoint<'static> = rs_matter_embassy::matter::dm::Endpoint {
            id: rs_matter_embassy::matter::dm::endpoints::ROOT_ENDPOINT_ID,
            device_types: rs_matter_embassy::matter::devices!(
                rs_matter_embassy::matter::dm::devices::DEV_TYPE_ROOT_NODE,
                rs_matter_embassy::matter::dm::devices::DEV_TYPE_OTA_REQUESTOR
            ),
            clusters: $crate::matter::ota::root_clusters!($net),
            client_clusters: &[$crate::matter::ota::PROVIDER_CLUSTER],
            unique_id: None,
            semantic_tags: &[],
        };
        ROOT
    }};
}
pub(crate) use root_endpoint;

#[cfg(not(matter_scheduled))]
macro_rules! root_clusters {
    ($net:ident) => {
        rs_matter_embassy::matter::clusters!($net; $crate::matter::ota::CLUSTER)
    };
}
/// With the controller's time zone, for the update time.
#[cfg(matter_scheduled)]
macro_rules! root_clusters {
    ($net:ident) => {
        rs_matter_embassy::matter::clusters!($net, time_sync(time_zone); $crate::matter::ota::CLUSTER)
    };
}
pub(crate) use root_clusters;

/// How often the default providers are asked for an update.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
/// Tries to tell the provider that its update runs.
const NOTIFY_ATTEMPTS: u32 = 6;
/// Longest `UpdateToken` (spec: 8 to 32 bytes).
const MAX_TOKEN: usize = 32;
/// Longest `ImageURI` (spec: 256).
const MAX_URI: usize = 256;
/// Bytes read from BDX at a time.
const CHUNK: usize = 1024;
/// How soon to ask again about a downloaded image the provider did not say to apply.
const RETRY_STAGED: Duration = Duration::from_secs(600);
/// Longest wait for the provider's go-ahead once downloaded.
const MAX_WAIT_S: u64 = 24 * 3600;
/// Longest wait for a provider to answer (its address, a session, the response): an exchange
/// stuck on a lost network would hold every later check.
const ANSWER: Duration = Duration::from_secs(120);

/// The requestor's state, kept across restarts of the stack.
pub(crate) struct Shared {
    providers: Providers,
    state: OtaState,
    /// What the controller sets with `SetTimeZone` and `SetDSTOffset` (kept in Matter's storage).
    #[cfg(matter_scheduled)]
    time_zones: rs_matter_embassy::matter::dm::clusters::time_sync::TimeZoneStore,
}

impl Shared {
    /// The state; call once.
    pub(crate) fn take() -> &'static Self {
        static SHARED: StaticCell<Shared> = StaticCell::new();
        SHARED.init(Shared {
            providers: Providers::new(),
            state: OtaState::new(ROOT_ENDPOINT_ID),
            #[cfg(matter_scheduled)]
            time_zones: rs_matter_embassy::matter::dm::clusters::time_sync::TimeZoneStore::new(),
        })
    }
}

/// The large buffers of a download, out of the Matter stack's memory (one download at a time).
struct Buffers {
    sector: Sector,
    header: [u8; file::MAX_HEADER_LEN],
    manifest: [u8; file::bundle::MAX_MANIFEST_LEN],
    chunk: [u8; CHUNK],
    uri: String<MAX_URI>,
}

static BUFFERS: Mutex<CriticalSectionRawMutex, Buffers> = Mutex::new(Buffers {
    sector: [0xff; core::mem::size_of::<Sector>()],
    header: [0; file::MAX_HEADER_LEN],
    manifest: [0; file::bundle::MAX_MANIFEST_LEN],
    chunk: [0; CHUNK],
    uri: String::new(),
});

/// The provider of the update on trial, to tell once it runs (`NotifyUpdateApplied`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Origin {
    fab_idx: u8,
    node_id: u64,
    endpoint: u16,
    token: Vec<u8, MAX_TOKEN>,
    /// The Matter version of the update.
    version: u32,
}

impl Setting for Origin {
    const KEY: u16 = keys::OTA_MATTER_FROM;
    const VERSION: u8 = 1;
}

/// The OTA Requestor cluster on endpoint 0, and the update loop in its background task.
pub(crate) struct Requestor<H> {
    cluster: H,
    shared: &'static Shared,
    firmware: Firmware,
    store: &'static SharedStore,
}

/// The cluster handler for the root endpoint (match it on `ROOT_ENDPOINT_ID` and [`CLUSTER`]),
/// running the updates of `firmware`.
pub(crate) fn requestor(
    shared: &'static Shared,
    firmware: Firmware,
    store: &'static SharedStore,
    dataver: Dataver,
) -> Requestor<impl AsyncHandler> {
    let handler = OtaRequestorHandler::new(dataver, &shared.providers, &shared.state);
    Requestor { cluster: rs_matter_embassy::matter::dm::Async(handler.adapt()), shared, firmware, store }
}

impl<H: AsyncHandler> AsyncHandler for Requestor<H> {
    fn read_awaits(&self, ctx: impl ReadContext) -> bool {
        self.cluster.read_awaits(ctx)
    }

    fn write_awaits(&self, ctx: impl WriteContext) -> bool {
        self.cluster.write_awaits(ctx)
    }

    fn invoke_awaits(&self, ctx: impl InvokeContext) -> bool {
        self.cluster.invoke_awaits(ctx)
    }

    fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> impl Future<Output = Result<(), Error>> {
        self.cluster.read(ctx, reply)
    }

    fn write(&self, ctx: impl WriteContext) -> impl Future<Output = Result<(), Error>> {
        self.cluster.write(ctx)
    }

    fn invoke(&self, ctx: impl InvokeContext, reply: impl InvokeReply) -> impl Future<Output = Result<(), Error>> {
        self.cluster.invoke(ctx, reply)
    }

    fn bump_dataver(&self, ctx: impl MatchContext) {
        self.cluster.bump_dataver(ctx)
    }

    fn lifecycle(&self, ctx: impl HandlerContext, op: LifecycleOp) -> Result<(), Error> {
        self.cluster.lifecycle(ctx, op)
    }

    async fn run(&self, ctx: impl HandlerContext) -> Result<(), Error> {
        match select(self.cluster.run(&ctx), updates(&ctx, self.shared, self.firmware, self.store)).await {
            Either::First(r) => r,
            Either::Second(never) => match never {},
        }
    }
}

/// Tells the provider of an update on trial that it runs, then checks for updates: when a
/// provider announces one, or the default providers each [`CHECK_EVERY`] (not with
/// `IOTKIT_AUTO_UPDATE=off`, unless about an image downloaded already).
async fn updates(
    ctx: &impl HandlerContext,
    shared: &Shared,
    fw: Firmware,
    store: &'static SharedStore,
) -> core::convert::Infallible {
    let Shared { providers, state, .. } = shared;
    notify_applied(ctx, fw, store).await;
    loop {
        state.set_update_possible(ctx, !crate::ota::esp::busy());
        let staged = crate::ota::esp::staged(store, fw.version).await.is_some();
        let next = if staged { RETRY_STAGED } else { CHECK_EVERY };
        // Kept for the update time: asked about again then (a few seconds past: the clock drifts).
        #[cfg(matter_scheduled)]
        let next = match at::seconds_to(ctx.matter(), shared) {
            Some(s) if staged => Duration::from_secs(s + 5),
            _ => next,
        };
        select(providers.wait_changed(), Timer::after(next)).await;
        let announced = providers.take_announced();
        // Announced: someone asked for it, so it installs at once.
        #[cfg(matter_scheduled)]
        let mut clicked = announced.len();
        #[cfg(matter_scheduled)]
        let due = at::due(ctx.matter(), shared);
        #[cfg(not(auto_update_off))]
        let defaults = (0..providers.len()).filter_map(|i| providers.get(i));
        // Off: asked only about an image downloaded already.
        #[cfg(auto_update_off)]
        let defaults = {
            let staged = crate::ota::esp::staged(store, fw.version).await.is_some();
            (0..providers.len()).filter(move |_| staged).filter_map(|i| providers.get(i))
        };
        for provider in announced.into_iter().chain(defaults) {
            if crate::ota::esp::busy() {
                log::info!("ota: provider {:#x} ignored: an update is running or on trial", provider.node_id);
                break;
            }
            #[cfg(matter_scheduled)]
            {
                at::HOLD.store(clicked == 0 && !due, crate::atomic::Ordering::Relaxed);
                clicked = clicked.saturating_sub(1);
            }
            match update(ctx, state, provider, fw, store).await {
                Ok(Some(version)) => {
                    log::info!("ota: {version} from Matter, restarting");
                    crate::diag::esp::restart().await;
                }
                Ok(None) => {}
                Err(e) => log::error!("ota: from provider {:#x}: {e}", provider.node_id),
            }
        }
    }
}

/// What a provider offers.
struct Offer {
    version: u32,
    token: Vec<u8, MAX_TOKEN>,
}

/// Asks `provider` for an update, and installs it: `Some(version)` once selected for the next
/// boot, `None` if there is none.
async fn update(
    ctx: &impl HandlerContext,
    state: &OtaState,
    provider: Provider,
    fw: Firmware,
    store: &'static SharedStore,
) -> Result<Option<crate::image::Version>, OtaError> {
    // Fits: `firmware!` refuses other versions under `matter-ota`.
    let running = file::version(fw.version).unwrap_or(0);
    let session = state.initiate_update(ctx);
    session.querying();
    let mut buffers = BUFFERS.lock().await;
    let Buffers { sector, header, manifest, chunk, uri } = &mut *buffers;
    uri.clear();
    let offer = answered(provider.query(
        ctx.matter(),
        ctx.crypto(),
        &[DownloadProtocolEnum::BDXSynchronous],
        Some(running),
        false,
        |r| {
            match r.status()? {
                StatusEnum::UpdateAvailable => {}
                StatusEnum::Busy => {
                    log::info!("ota: the provider is busy");
                    return Ok(None);
                }
                _ => return Ok(None),
            }
            let image_uri = r.image_uri()?.ok_or(ErrorCode::InvalidData)?;
            uri.push_str(image_uri).map_err(|_| ErrorCode::InvalidData)?;
            let token = r.update_token()?.ok_or(ErrorCode::InvalidData)?;
            let token = Vec::from_slice(token.0).map_err(|_| ErrorCode::InvalidData)?;
            let version = r.software_version()?.ok_or(ErrorCode::InvalidData)?;
            Ok(Some(Offer { version, token }))
        },
    ))
    .await
    .map_err(matter_error)?;
    let Some(offer) = offer else { return Ok(None) };
    if offer.version <= running {
        return Ok(None);
    }
    log::info!("ota: provider {:#x} offers {}", provider.node_id, file::from_version(offer.version));

    let (node, fd) = parse_bdx_url(uri).map_err(matter_error)?;
    let exchange =
        answered(Exchange::initiate(ctx.matter(), ctx.crypto(), provider.fab_idx, node)).await.map_err(matter_error)?;
    let mut reader = exchange.download(fd.as_bytes(), None).await.map_err(matter_error)?;
    session.downloading(Some(0));

    let mut prefix = [0; file::PREFIX_LEN];
    read_exact(&mut reader, &mut prefix).await?;
    let prefix = Prefix::parse(&prefix).map_err(OtaError::Image)?;
    let header_bytes = &mut header[..prefix.header_size as usize];
    read_exact(&mut reader, header_bytes).await?;
    let header = Header::parse(header_bytes).map_err(OtaError::Image)?;
    header.check_prefix(prefix).map_err(OtaError::Image)?;
    let dev = ctx.matter().dev_det();
    header.check_update(dev.vid, dev.pid, running).map_err(OtaError::Manifest)?;
    if header.version != offer.version {
        return Err(OtaError::Image(crate::image::Error::Mismatch));
    }
    let staged = match crate::ota::esp::staged(store, fw.version).await {
        // Downloaded before (a restart since, say): straight to the provider's go-ahead.
        Some(staged) if staged.source == header.digest => {
            log::info!("ota: {} already downloaded", staged.version());
            staged
        }
        _ => download(&mut reader, &header, fw, store, &session, sector, manifest, chunk).await?,
    };
    // The transfer is over: no exchange kept open while the provider decides.
    drop(reader);
    #[cfg(matter_scheduled)]
    if at::HOLD.load(crate::atomic::Ordering::Relaxed) {
        log::info!("ota: {} downloaded, kept for {}", staged.version(), env!("IOTKIT_UPDATE_TIME"));
        return Ok(None);
    }

    // The image waits in the spare slot meanwhile, other updates allowed: it is kept if the wait
    // gets too long or the provider cannot be reached, and asked about again.
    let mut waited = 0;
    loop {
        match answered(provider.apply_update(ctx.matter(), ctx.crypto(), &offer.token, offer.version)).await {
            Ok(OtaApplyOutcome::Proceed { delay_secs }) if waited + u64::from(delay_secs) <= MAX_WAIT_S => {
                Timer::after_secs(delay_secs.into()).await;
                break;
            }
            // The spec's floor: 2 minutes.
            Ok(OtaApplyOutcome::Await { delay_secs }) if waited + u64::from(delay_secs.max(120)) <= MAX_WAIT_S => {
                waited += u64::from(delay_secs.max(120));
                Timer::after_secs(delay_secs.max(120).into()).await;
            }
            Ok(OtaApplyOutcome::Discontinue) => {
                log::warn!("ota: the provider discontinued the update");
                crate::ota::esp::forget_staged(store).await;
                return Ok(None);
            }
            Ok(_) => {
                log::warn!("ota: the provider delays the update past {} h: asking again later", MAX_WAIT_S / 3600);
                return Ok(None);
            }
            Err(e) => return Err(matter_error(e)),
        }
    }
    session.applying();
    let from = Origin {
        fab_idx: provider.fab_idx.get(),
        node_id: provider.node_id,
        endpoint: provider.endpoint,
        token: offer.token,
        version: offer.version,
    };
    // Before the switch: a record without an update on trial is dropped at the next boot.
    if store.lock().await.save(&from).await.is_err() {
        log::warn!("ota: cannot keep the provider: it will not hear of the update");
    }
    crate::ota::esp::apply_staged(store, &staged, sector).await.map(Some)
}

/// Downloads the payload after `header` into the spare slot and keeps it there, checked
/// ([`Staged`](crate::ota::esp::Staged)).
#[allow(clippy::too_many_arguments)]
async fn download(
    reader: &mut BdxReader<'_>,
    header: &Header<'_>,
    fw: Firmware,
    store: &SharedStore,
    session: &OtaUpdate<'_>,
    sector: &mut Sector,
    manifest: &mut [u8; file::bundle::MAX_MANIFEST_LEN],
    chunk: &mut [u8; CHUNK],
) -> Result<crate::ota::esp::Staged, OtaError> {
    let mut digest = Sha256::new();

    #[cfg(not(feature = "matter-ota-unsigned"))]
    let (parsed, image_size) = {
        let mut start = [0; file::bundle::PREFIX_LEN];
        read_exact(reader, &mut start).await?;
        digest.update(start);
        let len = file::bundle::manifest_len(&start).map_err(OtaError::Manifest)?;
        let json = &mut manifest[..len];
        read_exact(reader, json).await?;
        digest.update(&*json);
        let m = crate::image::Manifest::parse(json).map_err(OtaError::Manifest)?;
        m.verify(fw.key).map_err(OtaError::Manifest)?;
        m.check_update(fw.product, fw.version).map_err(OtaError::Manifest)?;
        let bundled = (file::bundle::PREFIX_LEN + len) as u64 + u64::from(m.size);
        if file::version(m.version) != Some(header.version) || bundled != header.payload_size {
            return Err(OtaError::Manifest(crate::image::Error::Mismatch));
        }
        (m, m.size)
    };
    #[cfg(not(feature = "matter-ota-unsigned"))]
    let image = Image::Signed(&parsed);
    #[cfg(feature = "matter-ota-unsigned")]
    let (image, image_size) = {
        let _ = manifest;
        let size = u32::try_from(header.payload_size).map_err(|_| OtaError::TooLarge)?;
        (Image::Bare { header, product: fw.product }, size)
    };

    let mut install = Install::begin(Claim::take()?, image, sector)?;
    // Its slot is written over now.
    crate::ota::esp::forget_staged(store).await;
    let mut percent = 0;
    while install.received() < image_size {
        let n = chunk.len().min((image_size - install.received()) as usize);
        read_exact(reader, &mut chunk[..n]).await?;
        digest.update(&chunk[..n]);
        install.write(&chunk[..n]).await?;
        let now = (u64::from(install.received()) * 100 / u64::from(image_size.max(1))) as u8;
        if now / 5 != percent / 5 {
            session.downloading(Some(now));
            percent = now;
        }
    }
    if reader.read(&mut chunk[..1]).await.map_err(matter_error)? != 0 {
        return Err(OtaError::Image(crate::image::Error::Size));
    }
    if <[u8; 32]>::from(digest.finalize()) != header.digest {
        return Err(OtaError::Image(crate::image::Error::Hash));
    }
    install.stage(store, header.digest).await
}

/// After an update from Matter: tells its provider it runs (`NotifyUpdateApplied`), which passes
/// [`MATTER`]. Forgets the provider once told, or if the update is not this firmware (rolled back).
async fn notify_applied(ctx: &impl HandlerContext, fw: Firmware, store: &'static SharedStore) {
    let Ok(Some(from)) = store.lock().await.load::<Origin>().await else { return };
    let running = file::version(fw.version).unwrap_or(0);
    if let Some(fab_idx) = NonZeroU8::new(from.fab_idx)
        && from.version == running
    {
        let provider = Provider { fab_idx, node_id: from.node_id, endpoint: from.endpoint };
        for attempt in 1..=NOTIFY_ATTEMPTS {
            match answered(provider.notify_applied(ctx.matter(), ctx.crypto(), &from.token, running)).await {
                Ok(()) => {
                    log::info!("ota: provider {:#x} told that {} runs", from.node_id, fw.version);
                    HEALTH.pass(MATTER);
                    break;
                }
                Err(e) if attempt < NOTIFY_ATTEMPTS => {
                    log::warn!("ota: cannot reach provider {:#x} ({e:?}), again in 10 s", from.node_id);
                    Timer::after_secs(10).await;
                }
                Err(e) => log::warn!("ota: provider {:#x} not told: {e:?}", from.node_id),
            }
        }
    }
    let _ = store.lock().await.remove::<Origin>().await;
}

/// Fills `buf` from the transfer; an early end is a short file.
async fn read_exact(reader: &mut BdxReader<'_>, buf: &mut [u8]) -> Result<(), OtaError> {
    let mut at = 0;
    while at < buf.len() {
        match reader.read(&mut buf[at..]).await.map_err(matter_error)? {
            0 => return Err(OtaError::Image(crate::image::Error::Size)),
            n => at += n,
        }
    }
    Ok(())
}

/// `f`, or [`ErrorCode::RxTimeout`] once the provider took longer than [`ANSWER`].
async fn answered<T>(f: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    let f = core::pin::pin!(f);
    timed(f).await
}

/// Behind `dyn`: the timer is not copied into each exchange's state machine.
async fn timed<T>(f: core::pin::Pin<&mut dyn Future<Output = Result<T, Error>>>) -> Result<T, Error> {
    embassy_time::with_timeout(ANSWER, f).await.unwrap_or_else(|_| Err(ErrorCode::RxTimeout.into()))
}

fn matter_error(e: Error) -> OtaError {
    OtaError::Matter(e.code())
}

/// The update time (`IOTKIT_UPDATE_TIME`), in the time and time zone a controller gives.
#[cfg(matter_scheduled)]
pub(crate) mod at {
    use rs_matter_embassy::matter::{
        Matter,
        dm::{
            Async, AsyncHandler, Dataver,
            clusters::time_sync::{ClusterHandler as _, TimeSyncHandler},
        },
        utils::epoch::MATTER_EPOCH_SECS,
    };

    use super::Shared;
    use crate::atomic::AtomicBool;
    use crate::time::Zone;

    /// The Time Synchronization cluster, to chain [`handler`] on the root endpoint.
    pub(crate) const CLUSTER_ID: u32 = TimeSyncHandler::CLUSTER.id;

    /// Time Synchronization with the time zone, in place of the root handler's.
    pub(crate) fn handler(dataver: Dataver, shared: &'static Shared) -> impl AsyncHandler {
        Async(TimeSyncHandler::new_with_time_zone(dataver, &shared.time_zones).adapt())
    }

    /// `IOTKIT_UPDATE_TIME`, minutes after midnight.
    const MINUTE: u16 = match env!("IOTKIT_UPDATE_TIME").as_bytes() {
        &[h1 @ b'0'..=b'2', h2 @ b'0'..=b'9', b':', m1 @ b'0'..=b'5', m2 @ b'0'..=b'9']
            if (h1 - b'0') * 10 + h2 - b'0' < 24 =>
        {
            ((h1 - b'0') * 10 + h2 - b'0') as u16 * 60 + ((m1 - b'0') * 10 + m2 - b'0') as u16
        }
        _ => panic!("IOTKIT_UPDATE_TIME: `HH:MM` (02:00)"),
    };

    /// How long past the update time an update still installs: a provider can take a while.
    const WINDOW_S: i64 = 3600;

    /// Set before asking a default provider outside the update time: what it offers is downloaded
    /// and kept for then.
    pub(crate) static HOLD: AtomicBool = AtomicBool::new(false);

    /// Unix seconds and the time zone, if a controller gave the time since the device started.
    fn now(matter: &Matter<'_>, shared: &Shared) -> Option<(i64, Zone)> {
        let us = matter.with_rtc(|rtc| rtc.utc_time()).reliable()?;
        let zones = &shared.time_zones;
        let offset = zones.active_time_zone(us).0 + zones.active_dst_offset(us).unwrap_or(0);
        // Matter counts the offset east of UTC, POSIX (`Zone`) west.
        let zone = Zone { std_offset_s: -offset, dst_offset_s: -offset, ..Zone::UTC };
        Some(((us / 1_000_000 + MATTER_EPOCH_SECS) as i64, zone))
    }

    /// How long after the start an update waits for a controller to give the time: one that never
    /// does (no Time Synchronization) installs then.
    const NO_TIME_S: u64 = 3600;

    /// Whether an update installs now: within the hour past the update time, or with no time an
    /// hour after the start.
    pub(crate) fn due(matter: &Matter<'_>, shared: &Shared) -> bool {
        let Some((now, zone)) = now(matter, shared) else {
            let late = embassy_time::Instant::now().as_secs() >= NO_TIME_S;
            if late {
                log::info!("ota: no time from a controller an hour after the start: installing when offered");
            } else {
                log::info!("ota: no time from a controller yet: kept until it gives one, or an hour after the start");
            }
            return late;
        };
        let last = zone.next(now - 86_400, MINUTE);
        last <= now && now - last < WINDOW_S
    }

    /// Seconds to the next update time (`None`: no time yet).
    pub(crate) fn seconds_to(matter: &Matter<'_>, shared: &Shared) -> Option<u64> {
        let (now, zone) = now(matter, shared)?;
        Some((zone.next(now, MINUTE) - now) as u64)
    }
}
