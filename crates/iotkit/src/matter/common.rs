//! What the Wi-Fi and Thread assemblies share: the device's identity, its setup code and
//! pairing codes, and the pair / forget requests.

use crate::config::esp::SharedStore;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use heapless::String;
use rs_matter_embassy::matter::{
    BasicCommData, Matter,
    crypto::Rng,
    dm::{
        AsyncHandler, HandlerContext, InvokeContext, InvokeReply, LifecycleOp, MatchContext, OperationContext,
        ReadContext, ReadReply, WriteContext, clusters::basic_info::BasicInfoConfig, devices::test::TEST_DEV_DET,
    },
    error::Error,
    pairing::{
        DiscoveryCapabilities,
        qr::{CommFlowType, QrPayload, no_optional_data},
    },
    sc::pase::{Spake2pVerifierPassword, Spake2pVerifierPasswordRef},
};
use static_cell::StaticCell;

use crate::health::{HEALTH, MATTER};
use crate::matter::MatterSetup;

/// The OTA requestor's state (`matter-ota`), made once per Matter task: [`updates`].
#[cfg(feature = "matter-ota")]
pub(crate) type Updates = &'static crate::matter::ota::Shared;
#[cfg(not(feature = "matter-ota"))]
pub(crate) type Updates = ();

/// [`Updates`]; call once.
pub(crate) fn updates() -> Updates {
    #[cfg(feature = "matter-ota")]
    return crate::matter::ota::Shared::take();
}

/// A pairing (commissioning) window is open, for [`status`](crate::status).
static PAIRING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether a pairing window is open.
pub(crate) fn pairing() -> bool {
    PAIRING.load(core::sync::atomic::Ordering::Relaxed)
}

/// The next pair or forget request; meanwhile, follows the pairing window for [`pairing`]: it
/// closes once paired, or at its end (checked every 10 s while open).
pub(crate) async fn next_request(matter: &Matter<'_>) -> Request {
    use embassy_futures::select::{Either, select};
    loop {
        let open = matter.comm_window_state().is_open();
        if PAIRING.load(core::sync::atomic::Ordering::Relaxed) != open {
            PAIRING.store(open, core::sync::atomic::Ordering::Relaxed);
            crate::status::notify();
        }
        // Not paired yet: the window opens once the stack runs.
        if !open && matter.has_fabrics() {
            return REQUESTS.wait().await;
        }
        if let Either::First(request) = select(REQUESTS.wait(), embassy_time::Timer::after_secs(10)).await {
            return request;
        }
    }
}

/// Matter has started (its stored state loaded): with no fabric, no controller is to be waited
/// for, and [`MATTER`] passes.
pub(crate) fn started(matter: &Matter<'_>) {
    if !matter.has_fabrics() {
        HEALTH.pass(MATTER);
    }
}

/// The data model, passing [`MATTER`] at the first interaction of a controller: with a fabric,
/// while no commissioning window is open (PASE sessions only exist in one), so over CASE.
pub(crate) struct Tap<H>(pub H);

impl<H> Tap<H> {
    fn seen(ctx: &impl OperationContext) {
        if HEALTH.has_passed(MATTER) || ctx.exchange().matter().comm_window_state().is_open() {
            return;
        }
        if ctx.accessor().is_ok_and(|a| a.fab_idx().is_ok()) {
            log::info!("matter: a controller is connected");
            HEALTH.pass(MATTER);
        }
    }
}

impl<H: AsyncHandler> AsyncHandler for Tap<H> {
    fn read_awaits(&self, ctx: impl ReadContext) -> bool {
        self.0.read_awaits(ctx)
    }

    fn write_awaits(&self, ctx: impl WriteContext) -> bool {
        self.0.write_awaits(ctx)
    }

    fn invoke_awaits(&self, ctx: impl InvokeContext) -> bool {
        self.0.invoke_awaits(ctx)
    }

    fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> impl Future<Output = Result<(), Error>> {
        Self::seen(&ctx);
        self.0.read(ctx, reply)
    }

    fn write(&self, ctx: impl WriteContext) -> impl Future<Output = Result<(), Error>> {
        Self::seen(&ctx);
        self.0.write(ctx)
    }

    fn invoke(&self, ctx: impl InvokeContext, reply: impl InvokeReply) -> impl Future<Output = Result<(), Error>> {
        Self::seen(&ctx);
        self.0.invoke(ctx, reply)
    }

    fn bump_dataver(&self, ctx: impl MatchContext) {
        self.0.bump_dataver(ctx)
    }

    fn lifecycle(&self, ctx: impl HandlerContext, op: LifecycleOp) -> Result<(), Error> {
        self.0.lifecycle(ctx, op)
    }

    fn run(&self, ctx: impl HandlerContext) -> impl Future<Output = Result<(), Error>> {
        self.0.run(ctx)
    }
}

/// What the device is, for the Basic Information cluster.
#[derive(Clone, Copy)]
pub struct Product {
    /// The product name.
    pub name: &'static str,
    /// The version, as shown.
    pub version: &'static str,
    /// The version as a number (e.g. `0x00_07_00` for 0.7.0), increasing with updates.
    pub version_number: u32,
}

impl Product {
    /// `name`, at the version of `firmware`.
    pub const fn of(name: &'static str, firmware: crate::Firmware) -> Self {
        Self {
            name,
            version: firmware.version_text,
            // Versions past x.255.255 do not fit: they show as 0.
            version_number: match crate::image::matter::version(firmware.version) {
                Some(n) => n,
                None => 0,
            },
        }
    }
}

/// The codes to pair the device, for the app to show (console, label).
#[derive(Clone, Debug)]
pub struct PairingCodes {
    /// 11 digits, `1234-567-8901`.
    pub manual: String<13>,
    /// `MT:...`, the text of the QR code.
    pub qr: String<64>,
}

static CODES: Mutex<CriticalSectionRawMutex, core::cell::RefCell<Option<PairingCodes>>> =
    Mutex::new(core::cell::RefCell::new(None));

/// The pairing codes, once Matter has started.
pub fn pairing_codes() -> Option<PairingCodes> {
    CODES.lock(|c| c.borrow().clone())
}

pub(crate) enum Request {
    /// Open a commissioning window, to pair (another) controller.
    Pair,
    /// Forget every pairing.
    Reset,
    /// Close the pairing window, if paired (else it stays: the way to pair).
    Close,
}

pub(crate) static REQUESTS: embassy_sync::signal::Signal<CriticalSectionRawMutex, Request> =
    embassy_sync::signal::Signal::new();
/// How long `open_pairing_window` lets a controller pair.
pub const PAIRING_WINDOW_S: u16 = 900;

/// Lets a controller pair for [`PAIRING_WINDOW_S`] with the pairing code, even when already
/// paired (Matter devices can have several controllers).
pub fn open_pairing_window() {
    REQUESTS.signal(Request::Pair);
}

/// Closes an open pairing window (a short press of the button), once paired.
pub(crate) fn close_pairing_window() {
    REQUESTS.signal(Request::Close);
}

/// Forgets every pairing and restarts (the pairing code stays the same).
pub fn forget_pairings() {
    REQUESTS.signal(Request::Reset);
}

/// The code built into the firmware (`MATTER_PASSCODE` and `MATTER_DISCRIMINATOR`, e.g. from
/// `cargo iotkit new`: one code for the project's label); else the stored one, or a new one
/// (stored).
pub(crate) async fn setup_code(store: &SharedStore, rand: &mut impl Rng) -> MatterSetup {
    if let Some(passcode) = option_env!("MATTER_PASSCODE") {
        match MatterSetup::parse(passcode, option_env!("MATTER_DISCRIMINATOR").unwrap_or("")) {
            Some(setup) => return setup,
            None => log::error!("matter: invalid MATTER_PASSCODE or MATTER_DISCRIMINATOR; using the device's own code"),
        }
    }
    let mut store = store.lock().await;
    if let Ok(Some(s)) = store.load::<MatterSetup>().await {
        return s;
    }
    let setup = loop {
        if let Some(s) = MatterSetup::from_random(rand.next_u32(), rand.next_u32()) {
            break s;
        }
    };
    if let Err(e) = store.save(&setup).await {
        log::error!("matter: could not store the setup code: {e}");
    }
    setup
}

/// The commissioning data, the Basic Information, and the pairing codes (logged and kept for
/// [`pairing_codes`]). `name`: the device's host name or another unique name.
pub(crate) fn identity(
    setup: MatterSetup,
    product: Product,
    name: &'static str,
    discovery: DiscoveryCapabilities,
) -> (BasicCommData, &'static BasicInfoConfig<'static>) {
    let comm = BasicCommData {
        password: Spake2pVerifierPassword::new_from_ref(Spake2pVerifierPasswordRef::new(&setup.passcode.to_le_bytes())),
        discriminator: setup.discriminator,
    };
    static INFO: StaticCell<BasicInfoConfig<'static>> = StaticCell::new();
    let info = &*INFO.init(BasicInfoConfig {
        vendor_name: "iotkit",
        product_name: product.name,
        sw_ver: product.version_number,
        sw_ver_str: product.version,
        serial_no: name,
        device_name: name,
        // A short-idle-time ICD: how long a controller waits for its replies.
        #[cfg(feature = "matter-icd")]
        sii: Some(crate::matter::icd::POLL_MS),
        #[cfg(feature = "matter-icd")]
        sai: Some(crate::matter::icd::ACTIVE_POLL_MS),
        ..TEST_DEV_DET
    });
    let mut qr_buf = [0u8; 256];
    let qr = QrPayload::new_from_basic_info(discovery, CommFlowType::Standard, comm.clone(), info, no_optional_data);
    let codes = PairingCodes {
        manual: comm.compute_pretty_pairing_code(),
        qr: match qr.as_str(&mut qr_buf) {
            Ok((s, _)) => s.try_into().unwrap_or_default(),
            Err(e) => {
                log::warn!("matter: no QR text: {e:?}");
                String::new()
            }
        },
    };
    log::info!("matter: pair with the code {} (QR: {})", codes.manual, codes.qr);
    CODES.lock(|c| *c.borrow_mut() = Some(codes));
    (comm, info)
}
