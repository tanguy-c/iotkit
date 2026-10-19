//! What the Wi-Fi and Thread assemblies share: the device's identity, its setup code and
//! pairing codes, and the pair / forget requests.

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use heapless::String;
use iotkit_config::esp::SharedStore;
use rs_matter_embassy::matter::{
    BasicCommData,
    crypto::Rng,
    dm::{clusters::basic_info::BasicInfoConfig, devices::test::TEST_DEV_DET},
    pairing::{
        DiscoveryCapabilities,
        qr::{CommFlowType, QrPayload, no_optional_data},
    },
    sc::pase::{Spake2pVerifierPassword, Spake2pVerifierPasswordRef},
};
use static_cell::StaticCell;

use crate::MatterSetup;

/// What the device is, for the Basic Information cluster.
#[derive(Clone, Copy)]
pub struct Product {
    pub name: &'static str,
    pub version: &'static str,
    /// The version as a number (e.g. `0x00_07_00` for 0.7.0), increasing with updates.
    pub version_number: u32,
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

/// Forgets every pairing and restarts (the pairing code stays the same).
pub fn forget_pairings() {
    REQUESTS.signal(Request::Reset);
}

/// The stored setup code, or a new one (stored).
pub(crate) async fn setup_code(store: &SharedStore, rand: &mut impl Rng) -> MatterSetup {
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
