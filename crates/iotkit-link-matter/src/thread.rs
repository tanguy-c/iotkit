//! Matter over Thread: rs-matter-embassy's Thread stack owns the 802.15.4 radio and Bluetooth
//! (commissioning over BLE gives the device its Thread network). No Wi-Fi, so no iotkit-net,
//! MQTT or HTTP OTA. Needs a Thread border router (Home Assistant's OpenThread Border Router,
//! a Google or Apple hub) and `clang` + `cmake` to build OpenThread.

use core::pin::pin;

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::select::{Either, select};
use esp_hal::peripherals::{ADC1, BT, IEEE802154, RNG};
use iotkit_config::esp::{NVS_LABEL, SharedFlash, SharedStore, find_partition};
use iotkit_model::Device;
use rs_matter_embassy::{
    matter::{
        crypto::{Crypto, Rng, default_crypto},
        dm::{
            Async, Dataver, EmptyHandler, Node,
            clusters::desc,
            devices::test::{DAC_PRIVKEY, TEST_DEV_ATT},
            endpoints::ROOT_ENDPOINT_ID,
        },
        error::{Error, ErrorCode},
        pairing::DiscoveryCapabilities,
        utils::init::InitMaybeUninit,
    },
    persist::SeqMapKvBlobStore,
    stack::rand::reseeding_csprng,
    wireless::{EmbassyThread, EmbassyThreadMatterStack, esp::EspThreadDriver},
};
use static_cell::StaticCell;

pub use crate::common::{PAIRING_WINDOW_S, PairingCodes, Product, forget_pairings, open_pairing_window, pairing_codes};
use crate::{
    clusters::{Colors, Identifies},
    common::{REQUESTS, Request, identity, setup_code},
    endpoints::{self, COLOR, DESC, IDENTIFY},
    handler::Entities,
};

/// Memory for the stack's futures.
const BUMP_SIZE: usize = 20_000;

type Stack = EmbassyThreadMatterStack<'static, BUMP_SIZE, ()>;

/// The radios, and the TRNG's entropy source (without Wi-Fi, it needs the ADC).
pub struct Peripherals {
    pub radio: IEEE802154<'static>,
    pub bt: BT<'static>,
    pub rng: RNG<'static>,
    pub adc1: ADC1<'static>,
}

/// Runs Matter over Thread for the entities of `device`; never returns. `name`: a unique name
/// for the device (shown in controllers, used as serial number).
#[embassy_executor::task]
pub async fn matter_task(
    p: Peripherals,
    store: &'static SharedStore,
    product: Product,
    name: &'static str,
    device: &'static Device,
) {
    // Kept for the task's lifetime: the TRNG needs it.
    let _entropy = esp_hal::rng::TrngSource::new(p.rng, p.adc1);
    if let Err(e) = run(p.radio, p.bt, store, product, name, device).await {
        log::error!("matter: stopped: {e:?}");
    }
}

async fn run(
    radio: IEEE802154<'static>,
    bt: BT<'static>,
    store: &'static SharedStore,
    product: Product,
    name: &'static str,
    device: &'static Device,
) -> Result<(), Error> {
    let trng = esp_hal::rng::Trng::try_new().map_err(|_| Error::from(ErrorCode::NoSpace))?;
    let Ok(csprng) = reseeding_csprng(trng, 1000);
    let crypto = default_crypto(csprng, DAC_PRIVKEY);
    let mut rand = crypto.weak_rand()?;

    let setup = setup_code(store, &mut rand).await;
    let (comm, info) = identity(setup, product, name, DiscoveryCapabilities::BLE);
    static STACK: StaticCell<Stack> = StaticCell::new();
    let stack = STACK.uninit().init_with(Stack::init(info, comm, &TEST_DEV_ATT));

    let (node, eps) = endpoints::build(device, Stack::root_endpoint(), &mut rand);
    let node: Node<'static> = node;
    // Mutually exclusive matchers: the last chained is tried first.
    let handler = EmptyHandler
        .chain(|e, _| e == ROOT_ENDPOINT_ID, Async(Stack::root_handler(&(), &mut rand)))
        .chain(
            |e, c| e != ROOT_ENDPOINT_ID && c == DESC.id,
            Async(desc::DescHandler::new(Dataver::new_rand(&mut rand)).adapt()),
        )
        .chain(|e, c| e != ROOT_ENDPOINT_ID && c == COLOR.id, Colors::new(eps, &mut rand))
        .chain(|e, c| e != ROOT_ENDPOINT_ID && c == IDENTIFY.id, Async(Identifies::new(eps, &mut rand)))
        .chain(
            |e, c| e != ROOT_ENDPOINT_ID && c != DESC.id && c != COLOR.id && c != IDENTIFY.id,
            Async(Entities { eps }),
        );

    let range = find_partition(NVS_LABEL).ok_or(Error::from(ErrorCode::NoSpace))?;
    let mut kv_store = SeqMapKvBlobStore::new(BlockingAsync::new(SharedFlash), range.clone());
    if let Err(e) = stack.startup(&crypto, &mut kv_store).await {
        log::warn!("matter: stored state unreadable ({e:?}), erasing `nvs`");
        iotkit_config::esp::with(|flash| flash.erase(range.start, range.end))
            .map_err(|_| Error::from(ErrorCode::StdIoError))?;
        stack.startup(&crypto, &mut kv_store).await?;
    }
    let kv = stack.matter().kv(kv_store);

    let mut eui64 = [0; 8];
    rand.fill_bytes(&mut eui64);
    let matter = pin!(stack.run(
        EmbassyThread::new(EspThreadDriver::new(radio, bt), crypto.rand()?, eui64, &kv, stack, true),
        &crypto,
        (node, handler),
        &kv,
        (),
    ));
    let requests = async {
        loop {
            match REQUESTS.wait().await {
                Request::Pair => match stack.matter().open_basic_comm_window(PAIRING_WINDOW_S, &crypto, &()) {
                    Ok(()) => log::info!("matter: pairing window open for {} min", PAIRING_WINDOW_S / 60),
                    Err(e) => log::warn!("matter: cannot open a pairing window: {e:?}"),
                },
                Request::Reset => {
                    log::warn!("matter: forgetting every pairing");
                    let _ = iotkit_config::esp::with(|flash| flash.erase(range.start, range.end));
                    iotkit_diag::esp::restart().await;
                }
            }
        }
    };
    match select(matter, requests).await {
        Either::First(r) => r,
        Either::Second(()) => Ok(()),
    }
}
