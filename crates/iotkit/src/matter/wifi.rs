//! Matter over Wi-Fi, on `iotkit::net`'s stack, commissioned on that network. The setup code is
//! the device's own ([`MatterSetup`], printed at boot); pairings live in the `nvs` partition.
//! Attestation uses the test vendor's credentials (development only).
//! The endpoints come from the model (see the [crate] docs).

use core::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
    pin::pin,
};

use crate::Firmware;
use crate::config::esp::{NVS_LABEL, SharedFlash, SharedStore, find_partition};
use crate::model::Device;
use crate::net::esp::Net;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::select::{Either, select};

pub use crate::matter::common::{
    PAIRING_WINDOW_S, PairingCodes, Product, forget_pairings, open_pairing_window, pairing_codes,
};
use crate::matter::{
    clusters::{Colors, Identifies},
    common::{Request, Tap, Updates, identity, next_request, setup_code, started, updates},
    endpoints::{self, COLOR, DESC, Ep, IDENTIFY},
    handler::Entities,
};
use rs_matter_embassy::{
    enet::{EnetMatterUdpBuffers, EnetNetif, EnetStack},
    matter::{
        Matter,
        crypto::{Crypto, default_crypto},
        dm::{
            Async, Dataver, EmptyHandler, Node,
            clusters::{desc, gen_diag::InterfaceTypeEnum},
            devices::test::{DAC_PRIVKEY, TEST_DEV_ATT},
            endpoints::ROOT_ENDPOINT_ID,
        },
        error::{Error, ErrorCode},
        pairing::DiscoveryCapabilities,
        transport::network::mdns::{
            MDNS_IPV4_BROADCAST_ADDR, MDNS_IPV6_BROADCAST_ADDR, MDNS_PORT,
            builtin::{BuiltinMdns, Host},
        },
        utils::init::InitMaybeUninit,
    },
    persist::SeqMapKvBlobStore,
    stack::{
        eth::EthMatterStack,
        mdns::Mdns,
        nal::{MulticastV4, MulticastV6, UdpBind, UdpSplit},
        rand::reseeding_csprng,
        udp,
    },
};
use static_cell::StaticCell;

/// Memory for the stack's futures; the OTA requestor's (a CASE session, a BDX download) too.
#[cfg(not(feature = "matter-ota"))]
const BUMP_SIZE: usize = 20_000;
#[cfg(feature = "matter-ota")]
const BUMP_SIZE: usize = 32_000;

type Stack = EthMatterStack<'static, BUMP_SIZE, ()>;

/// rs-matter's mDNS responder, answering `<hostname>.local` (instead of a MAC-derived name).
struct HostMdns {
    mdns: BuiltinMdns,
    hostname: &'static str,
}

impl Mdns for HostMdns {
    async fn run<C, U>(
        &mut self,
        matter: &Matter<'_>,
        crypto: C,
        udp: U,
        _mac: &[u8],
        ipv4: Ipv4Addr,
        ipv6: &[Ipv6Addr],
        interface: u32,
    ) -> Result<(), Error>
    where
        C: Crypto,
        U: UdpBind,
    {
        let map = |_| Error::from(ErrorCode::StdIoError);
        let mut socket =
            udp.bind(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, MDNS_PORT, 0, 0))).await.map_err(map)?;
        socket.join_v4(MDNS_IPV4_BROADCAST_ADDR, ipv4).await.map_err(map)?;
        socket.join_v6(MDNS_IPV6_BROADCAST_ADDR, interface).await.map_err(map)?;
        let (recv, send) = socket.split();
        let host = Host { hostname: self.hostname, ip: ipv4, ipv6 };
        self.mdns.run(udp::Udp(send), udp::Udp(recv), &host, Some(ipv4), Some(interface), matter, crypto).await
    }
}

/// Runs Matter on `net` for the entities of `device` (see the [crate] docs), as `product_name`
/// at the version of `firmware` (which `matter-ota` updates); never returns. After an error it
/// starts again (it also answers `<hostname>.local`).
#[embassy_executor::task]
pub async fn matter_task(
    net: Net,
    store: &'static SharedStore,
    product_name: &'static str,
    firmware: Firmware,
    device: &'static Device,
) {
    let product = Product::of(product_name, firmware);
    let Ok(trng) = esp_hal::rng::Trng::try_new() else {
        log::error!("matter: no random number generator");
        return;
    };
    let Ok(csprng) = reseeding_csprng(trng, 1000);
    let crypto = default_crypto(csprng, DAC_PRIVKEY);
    let Ok(mut rand) = crypto.weak_rand() else { return };

    let setup = setup_code(store, &mut rand).await;
    let (comm, info) = identity(setup, product, net.hostname, DiscoveryCapabilities::IP);

    static STACK: StaticCell<Stack> = StaticCell::new();
    let stack = STACK.uninit().init_with(Stack::init(info, comm, &TEST_DEV_ATT));
    static BUFFERS: StaticCell<EnetMatterUdpBuffers> = StaticCell::new();
    let buffers = BUFFERS.init(EnetMatterUdpBuffers::new());
    #[cfg(not(feature = "matter-ota"))]
    let root = Stack::root_endpoint();
    #[cfg(feature = "matter-ota")]
    let root = crate::matter::ota::root_endpoint!(eth);
    let (node, eps) = endpoints::build(device, root, &mut rand);
    let node: &'static [rs_matter_embassy::matter::dm::Endpoint<'static>] = node.endpoints;

    #[cfg_attr(not(feature = "matter-ota"), allow(clippy::let_unit_value))]
    let updates = updates();
    loop {
        let node = Node { endpoints: node };
        if let Err(e) = run(net, stack, &crypto, node, eps, buffers, store, firmware, updates).await {
            log::error!("matter: stopped: {e:?}, starting again in 10 s");
        }
        embassy_time::Timer::after_secs(10).await;
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(feature = "matter-ota"), allow(unused_variables))]
async fn run<C: Crypto>(
    net: Net,
    stack: &mut Stack,
    crypto: &C,
    node: Node<'static>,
    eps: &'static [Ep],
    buffers: &EnetMatterUdpBuffers,
    store: &'static SharedStore,
    firmware: Firmware,
    updates: Updates,
) -> Result<(), Error> {
    let mut rand = crypto.weak_rand()?;
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
    let handler = handler.chain(
        |e, c| e == ROOT_ENDPOINT_ID && c == crate::matter::ota::CLUSTER.id,
        crate::matter::ota::requestor(updates, firmware, store, Dataver::new_rand(&mut rand)),
    );

    // Pairings and the rest of rs-matter's state, in the `nvs` partition. If it does not load
    // (another firmware's data, say), it is erased and Matter starts unpaired.
    let range = find_partition(NVS_LABEL).ok_or(Error::from(ErrorCode::NoSpace))?;
    let mut kv_store = SeqMapKvBlobStore::new(BlockingAsync::new(SharedFlash), range.clone());
    if let Err(e) = stack.startup(crypto, &mut kv_store).await {
        log::warn!("matter: stored state unreadable ({e:?}), erasing `nvs`");
        crate::config::esp::with(|flash| flash.erase(range.start, range.end))
            .map_err(|_| Error::from(ErrorCode::StdIoError))?;
        stack.startup(crypto, &mut kv_store).await?;
    }
    let kv = stack.matter().kv(kv_store);
    started(stack.matter());

    let mut mdns = HostMdns { mdns: BuiltinMdns::new(), hostname: net.hostname };
    let matter = pin!(stack.run_preex(
        EnetStack::new(net.stack, buffers),
        EnetNetif::new(net.stack, InterfaceTypeEnum::WiFi),
        &mut mdns,
        crypto,
        (node, Tap(handler)),
        &kv,
        (),
    ));
    let requests = async {
        loop {
            match next_request(stack.matter()).await {
                Request::Pair => match stack.matter().open_basic_comm_window(PAIRING_WINDOW_S, crypto, &()) {
                    Ok(()) => log::info!("matter: pairing window open for {} min", PAIRING_WINDOW_S / 60),
                    Err(e) => log::warn!("matter: cannot open a pairing window: {e:?}"),
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
