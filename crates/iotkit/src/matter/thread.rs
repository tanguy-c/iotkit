//! Matter over Thread: rs-matter-embassy's Thread stack owns the 802.15.4 radio and Bluetooth
//! (commissioning over BLE gives the device its Thread network). No Wi-Fi, so no `iotkit::net`
//! or MQTT; updates come over Matter (`matter-ota`). Needs a Thread border router (e.g. Home
//! Assistant's OpenThread Border Router) and `clang` + `cmake` to build
//! OpenThread. [`NETWORK`](crate::health::NETWORK) passes once attached to the Thread network.

use core::pin::pin;

use crate::Firmware;
use crate::config::esp::{NVS_LABEL, SharedFlash, SharedStore, find_partition};
use crate::health::{HEALTH, NETWORK, Radio, radio_failed};
use crate::model::Device;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::select::{Either, select};
use esp_hal::peripherals::{ADC1, BT, IEEE802154, RNG};
#[cfg(not(feature = "matter-icd"))]
use rs_matter_embassy::wireless::{EmbassyThread, EmbassyThreadMatterStack};
use rs_matter_embassy::{
    matter::{
        crypto::{Crypto, Rng, default_crypto},
        dm::networks::NetChangeNotif,
        dm::{
            Async, Dataver, EmptyHandler, Node,
            clusters::{desc, gen_diag::NetifDiag},
            devices::test::{DAC_PRIVKEY, TEST_DEV_ATT},
            endpoints::ROOT_ENDPOINT_ID,
        },
        error::{Error, ErrorCode},
        pairing::DiscoveryCapabilities,
        utils::init::InitMaybeUninit,
    },
    persist::SeqMapKvBlobStore,
    stack::{UserTask, nal::NetStack, rand::reseeding_csprng},
    wireless::esp::EspThreadDriver,
};
use static_cell::StaticCell;

pub use crate::matter::common::{
    PAIRING_WINDOW_S, PairingCodes, Product, forget_pairings, open_pairing_window, pairing_codes,
};
use crate::matter::{
    clusters::{Colors, Identifies},
    common::{Request, Tap, identity, next_request, setup_code, started, updates},
    endpoints::{self, COLOR, DESC, IDENTIFY},
    handler::Entities,
};

/// Memory for the stack's futures, the OTA requestor's included: all allocated when the stack
/// starts (a shortfall panics then), measured on a C6 with `matter-ota` at 19.4 KB, plus 1.1 KB
/// while commissioning over Bluetooth (`matter-icd`: the same).
#[cfg(not(feature = "matter-ota"))]
const BUMP_SIZE: usize = 20_000;
#[cfg(feature = "matter-ota")]
const BUMP_SIZE: usize = 23_500;

#[cfg(not(feature = "matter-icd"))]
type Stack = EmbassyThreadMatterStack<'static, BUMP_SIZE, ()>;
// On battery, a sleepy end device ([`crate::matter::icd`]).
#[cfg(feature = "matter-icd")]
type Stack = crate::matter::icd::Stack<BUMP_SIZE>;

/// The radios, and the TRNG's entropy source (without Wi-Fi, it needs the ADC).
pub struct Peripherals {
    /// The 802.15.4 radio, for Thread.
    pub radio: IEEE802154<'static>,
    /// Bluetooth LE, for commissioning.
    pub bt: BT<'static>,
    /// The random number generator.
    pub rng: RNG<'static>,
    /// The TRNG's entropy source.
    pub adc1: ADC1<'static>,
}

/// Runs Matter over Thread for the entities of `device`, as `product_name` at the version of
/// `firmware` (which `matter-ota` updates); never returns. `name`: a unique name for the device
/// (shown in controllers, used as serial number).
#[embassy_executor::task]
pub async fn matter_task(
    p: Peripherals,
    store: &'static SharedStore,
    product_name: &'static str,
    firmware: Firmware,
    name: &'static str,
    device: &'static Device,
) {
    // Kept for the task's lifetime: the TRNG needs it.
    let _entropy = esp_hal::rng::TrngSource::new(p.rng, p.adc1);
    if let Err(e) = run(p.radio, p.bt, store, product_name, firmware, name, device).await {
        log::error!("matter: stopped: {e:?}");
        radio_failed(Radio::Thread);
    }
}

/// Passes [`NETWORK`] once the Thread interface is operational (attached, with addresses). The
/// stack runs it on the operational network only.
struct ThreadCheck;

impl UserTask for ThreadCheck {
    async fn run<S, N>(&mut self, _net_stack: S, netif: N) -> Result<(), Error>
    where
        S: NetStack,
        N: NetifDiag + NetChangeNotif,
    {
        loop {
            let mut up = false;
            netif.netifs(&mut |ni| {
                up |= ni.operational;
                Ok(())
            })?;
            if up {
                if !HEALTH.has_passed(NETWORK) {
                    log::info!("thread: attached");
                    HEALTH.pass(NETWORK);
                }
                // Returning would stop the stack.
                core::future::pending::<()>().await;
            }
            // Also polled: OpenThread wakes one waiter per change, and the stack's network
            // manager waits too, so the attach can wake it instead.
            select(netif.wait_changed(), embassy_time::Timer::after_secs(1)).await;
        }
    }
}

#[cfg_attr(not(feature = "matter-ota"), allow(unused_variables))]
async fn run(
    radio: IEEE802154<'static>,
    bt: BT<'static>,
    store: &'static SharedStore,
    product_name: &'static str,
    firmware: Firmware,
    name: &'static str,
    device: &'static Device,
) -> Result<(), Error> {
    let product = Product::of(product_name, firmware);
    let trng = esp_hal::rng::Trng::try_new().map_err(|_| Error::from(ErrorCode::NoSpace))?;
    let Ok(csprng) = reseeding_csprng(trng, 1000);
    let crypto = default_crypto(csprng, DAC_PRIVKEY);
    let mut rand = crypto.weak_rand()?;

    let setup = setup_code(store, &mut rand).await;
    let (comm, info) = identity(setup, product, name, DiscoveryCapabilities::BLE);
    static STACK: StaticCell<Stack> = StaticCell::new();
    let stack = STACK.uninit().init_with(Stack::init(info, comm, &TEST_DEV_ATT));

    #[cfg(not(feature = "matter-ota"))]
    let root = Stack::root_endpoint();
    #[cfg(feature = "matter-ota")]
    let root = crate::matter::ota::root_endpoint!(thread);
    let (node, eps) = endpoints::build(device, root, &mut rand);
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
    #[cfg(feature = "matter-ota")]
    let updates = updates();
    #[cfg(feature = "matter-ota")]
    let handler = handler.chain(
        |e, c| e == ROOT_ENDPOINT_ID && c == crate::matter::ota::CLUSTER.id,
        crate::matter::ota::requestor(updates, firmware, store, Dataver::new_rand(&mut rand)),
    );
    // Tried before the root handler's: with the time zone.
    #[cfg(matter_scheduled)]
    let handler = handler.chain(
        |e, c| e == ROOT_ENDPOINT_ID && c == crate::matter::ota::at::CLUSTER_ID,
        crate::matter::ota::at::handler(Dataver::new_rand(&mut rand), updates),
    );

    let range = find_partition(NVS_LABEL).ok_or(Error::from(ErrorCode::NoSpace))?;
    let mut kv_store = SeqMapKvBlobStore::new(BlockingAsync::new(SharedFlash), range.clone());
    if let Err(e) = stack.startup(&crypto, &mut kv_store).await {
        log::warn!("matter: stored state unreadable ({e:?}), erasing `nvs`");
        crate::config::esp::with(|flash| flash.erase(range.start, range.end))
            .map_err(|_| Error::from(ErrorCode::StdIoError))?;
        stack.startup(&crypto, &mut kv_store).await?;
    }
    let kv = stack.matter().kv(kv_store);
    started(stack.matter());
    if !stack.matter().has_fabrics() {
        // Not commissioned: no Thread network to join yet.
        HEALTH.pass(NETWORK);
    }

    let mut eui64 = [0; 8];
    rand.fill_bytes(&mut eui64);
    #[cfg(not(feature = "matter-icd"))]
    let thread = EmbassyThread::new(EspThreadDriver::new(radio, bt), crypto.rand()?, eui64, &kv, stack, true);
    #[cfg(feature = "matter-icd")]
    let thread =
        crate::matter::icd::SleepyThread::new(EspThreadDriver::new(radio, bt), crypto.rand()?, eui64, &kv, stack);
    let matter = pin!(stack.run(thread, &crypto, (node, Tap(handler)), &kv, ThreadCheck));
    let requests = async {
        loop {
            match next_request(stack.matter()).await {
                Request::Pair => match stack.matter().open_basic_comm_window(PAIRING_WINDOW_S, &crypto, &()) {
                    Ok(()) => log::info!("matter: pairing window open for {} min", PAIRING_WINDOW_S / 60),
                    Err(e) => log::warn!("matter: cannot open a pairing window: {e:?}"),
                },
                Request::Close if !stack.matter().has_fabrics() => {
                    log::info!("matter: not paired yet, the pairing window stays open")
                }
                Request::Close => match stack.matter().close_comm_window(&()) {
                    Ok(_) => log::info!("matter: pairing window closed"),
                    Err(e) => log::warn!("matter: cannot close the pairing window: {e:?}"),
                },
                Request::Reset => {
                    log::warn!("matter: forgetting every pairing");
                    let _ = crate::config::esp::with(|flash| flash.erase(range.start, range.end));
                    crate::diag::esp::restart().await;
                }
            }
        }
    };
    match select(matter, requests).await {
        Either::First(r) => r,
        Either::Second(()) => Ok(()),
    }
}
