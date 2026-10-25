//! The sleepy Thread driver: rs-matter-embassy's `EmbassyThread` (`wireless/thread.rs` at the
//! pinned rev) with the OpenThread instance made a sleepy end device, its poll period driven by
//! the traffic, and a radio that keeps the CPU awake while it listens or sends. rs-matter-embassy
//! creates the instance inside its driver and has no hook for its link mode, hence the copy.

use core::pin::pin;

use embassy_futures::select::{Either, select, select3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, with_timeout};
use esp_hal::rtc_cntl::WakeLock;
use rs_matter_embassy::{
    ble::{BtpGattContext, BtpGattPeripheral, Controller},
    matter::{
        crypto::{CryptoRng, Rng},
        dm::networks::wireless::Thread,
        error::{Error, ErrorCode},
        persist::KvBlobStoreAccess,
        utils::{
            init::{Init, init},
            select::Coalesce,
            sync::IfMutex,
        },
    },
    ot::{
        OtMatterResources, OtMdns, OtNetCtl, OtNetStack, OtNetif, OtPersist,
        openthread::{self, OpenThread, OtError, PsduMeta, Radio, RadioCaps, SrcMatchConfig},
    },
    stack::{
        network::{Embedding, Network},
        wireless::{self, Gatt, GattTask},
    },
    wireless::{BleDriver, BleDriverTask, EmbassyWirelessMatterStack, ThreadDriver, ThreadDriverTask},
};

use super::{ACTIVE_POLL_MS, POLL_MS};

/// How long polls stay fast after data went either way: the reply, or the acknowledgment, of
/// an exchange waits at the parent. At least the spec's default active threshold (4 s), which
/// peers assume: rs-matter does not advertise ours.
const ACTIVE: Duration = Duration::from_secs(4);

/// The Matter stack over Thread, with the OpenThread resources this driver uses.
pub(crate) type Stack<const B: usize> = EmbassyWirelessMatterStack<'static, B, Thread, IcdContext>;

/// Data went over the radio (either way).
static ACTIVITY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// rs-matter-embassy's `OtNetContext`, for this driver: the OpenThread resources, in the stack.
pub(crate) struct IcdContext {
    resources: IfMutex<OtMatterResources>,
}

impl Embedding for IcdContext {
    const INIT: Self = Self { resources: IfMutex::new(OtMatterResources::new()) };

    fn init() -> impl Init<Self> {
        init!(Self {
            resources <- IfMutex::init(OtMatterResources::init()),
        })
    }
}

fn ot_err(e: OtError) -> Error {
    log::error!("openthread: {e:?}");
    ErrorCode::NoNetworkInterface.into()
}

/// The Thread network (and Bluetooth, for commissioning) of a sleepy end device.
pub(crate) struct SleepyThread<'a, T, K, R> {
    driver: T,
    ieee_eui64: [u8; 8],
    kv: K,
    context: &'a IcdContext,
    ble_context: &'a BtpGattContext,
    rand: R,
}

impl<'a, T, K, R> SleepyThread<'a, T, K, R> {
    pub(crate) fn new<const B: usize>(driver: T, rand: R, ieee_eui64: [u8; 8], kv: K, stack: &'a Stack<B>) -> Self {
        let embedding = stack.network().embedding();
        Self { driver, ieee_eui64, kv, context: embedding.net_context(), ble_context: embedding.ble_context(), rand }
    }
}

impl<T, K, R> wireless::Thread for SleepyThread<'_, T, K, R>
where
    T: ThreadDriver,
    K: KvBlobStoreAccess,
    R: CryptoRng + Copy,
{
    type NetCtl<'a>
        = &'a OtNetCtl<'a>
    where
        Self: 'a;

    async fn run<A>(&mut self, task: A) -> Result<(), Error>
    where
        A: wireless::ThreadTask,
    {
        let task =
            ThreadTask { ieee_eui64: self.ieee_eui64, rand: self.rand, kv: &self.kv, context: self.context, task };
        self.driver.run(task).await
    }
}

impl<T, K, R> Gatt for SleepyThread<'_, T, K, R>
where
    T: BleDriver,
    K: KvBlobStoreAccess,
    R: Rng + Copy,
{
    async fn run<A>(&mut self, task: A) -> Result<(), Error>
    where
        A: GattTask,
    {
        self.driver.run(BleTask { task, rand: self.rand, context: self.ble_context }).await
    }
}

struct BleTask<'a, A, R> {
    task: A,
    rand: R,
    context: &'a BtpGattContext,
}

impl<A, R> BleDriverTask for BleTask<'_, A, R>
where
    A: GattTask,
    R: Rng + Copy,
{
    async fn run<C>(&mut self, controller: C) -> Result<(), Error>
    where
        C: Controller,
    {
        // A random address, as `EmbassyThread::new(.., true)`.
        let mut peripheral = BtpGattPeripheral::new(controller, Some(self.rand), self.context);
        self.task.run(&mut peripheral).await
    }
}

struct ThreadTask<'a, A, K, C> {
    ieee_eui64: [u8; 8],
    rand: C,
    kv: K,
    context: &'a IcdContext,
    task: A,
}

impl<A, K, C> ThreadDriverTask for ThreadTask<'_, A, K, C>
where
    A: wireless::ThreadTask,
    K: KvBlobStoreAccess,
    C: CryptoRng + Copy,
{
    async fn run<R>(&mut self, radio: R) -> Result<(), Error>
    where
        R: Radio,
    {
        let mut resources = self.context.resources.lock().await;
        let resources = &mut *resources;

        let persister = OtPersist::new(&mut resources.settings_buf, &self.kv);
        persister.load()?;
        let mut settings = persister.settings();
        let mut rand = self.rand;
        let ot = OpenThread::new_with_udp_srp(
            self.ieee_eui64,
            &mut rand,
            &mut settings,
            &mut resources.ot,
            &mut resources.udp,
            &mut resources.srp,
        )
        .map_err(ot_err)?;
        // A sleepy end device: the receiver off when idle, the parent keeps its frames until it polls.
        ot.set_link_mode(false, false, false).map_err(ot_err)?;
        ot.set_poll_period(POLL_MS).map_err(ot_err)?;

        let net_ctl = OtNetCtl::new(ot.clone());
        let net_stack = OtNetStack::new(ot.clone());
        let netif = OtNetif::new(ot.clone());
        let mut mdns = OtMdns::new(ot.clone(), &mut resources.mdns_buf);

        let mut main = pin!(self.task.run(&net_stack, &netif, &net_ctl, &mut mdns));
        let mut radio = pin!(async {
            ot.run(SleepyRadio { radio, listening: None }).await;
            #[allow(unreachable_code)]
            Ok(())
        });
        let mut persist = pin!(persister.run());
        ot.enable_ipv6(true).map_err(ot_err)?;
        ot.srp_autostart().map_err(ot_err)?;

        let stack = select3(&mut main, &mut radio, &mut persist).coalesce();
        let result = match select(stack, polls(&ot)).await {
            Either::First(result) => result,
            Either::Second(()) => Ok(()),
        };

        let _ = ot.enable_thread(false);
        let _ = ot.srp_stop();
        let _ = ot.enable_ipv6(false);
        result
    }
}

/// Polls fast while data comes and goes, at [`POLL_MS`] otherwise.
async fn polls(ot: &OpenThread<'_>) {
    loop {
        ACTIVITY.wait().await;
        let _ = ot.set_poll_period(ACTIVE_POLL_MS);
        while with_timeout(ACTIVE, ACTIVITY.wait()).await.is_ok() {}
        let _ = ot.set_poll_period(POLL_MS);
    }
}

/// A data frame (not an acknowledgment, a beacon or a MAC command such as a data poll).
fn is_data(psdu: &[u8]) -> bool {
    psdu.first().is_some_and(|fc| fc & 0b111 == 1)
}

/// The radio, holding the CPU out of light sleep while it listens or sends (a sleeping CPU would
/// miss its interrupts), and reporting data to [`polls`].
struct SleepyRadio<R> {
    radio: R,
    listening: Option<WakeLock>,
}

impl<R: Radio> Radio for SleepyRadio<R> {
    type Error = R::Error;

    async fn init(&mut self) -> Result<RadioCaps, Self::Error> {
        self.radio.init().await
    }

    async fn set_config(&mut self, config: &openthread::Config) -> Result<(), Self::Error> {
        self.radio.set_config(config).await
    }

    async fn set_src_match_config(&mut self, config: &SrcMatchConfig) -> Result<(), Self::Error> {
        self.radio.set_src_match_config(config).await
    }

    async fn energy_scan(&mut self, channel: u8, duration_millis: u16) -> Result<i8, Self::Error> {
        let _awake = WakeLock::new();
        self.radio.energy_scan(channel, duration_millis).await
    }

    async fn set_receive(&mut self, channel: u8) -> Result<(), Self::Error> {
        self.listening.get_or_insert_with(WakeLock::new);
        self.radio.set_receive(channel).await
    }

    async fn set_sleep(&mut self) -> Result<(), Self::Error> {
        let result = self.radio.set_sleep().await;
        self.listening = None;
        result
    }

    async fn transmit(
        &mut self,
        psdu: &[u8],
        channel: u8,
        power: i8,
        cca_threshold: Option<i8>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> Result<Option<PsduMeta>, Self::Error> {
        let _awake = WakeLock::new();
        if is_data(psdu) {
            ACTIVITY.signal(());
        }
        self.radio.transmit(psdu, channel, power, cca_threshold, ack_psdu_buf).await
    }

    async fn receive(&mut self, psdu_buf: &mut [u8]) -> Result<PsduMeta, Self::Error> {
        let meta = self.radio.receive(psdu_buf).await?;
        if is_data(&psdu_buf[..meta.len]) {
            ACTIVITY.signal(());
        }
        Ok(meta)
    }
}
