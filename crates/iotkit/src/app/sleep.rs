//! Deep-sleeping devices (feature `deep-sleep`, MQTT only): wake up, report, sleep.
//!
//! ```ignore
//! static SLEEP: SleepConfig = SleepConfig { interval: Duration::from_secs(300), max_awake: Duration::from_secs(30) };
//! let app = iotkit::start_sleepy(spawner, board, p.LPWR, &CONFIG, &SLEEP).await;
//! TEMPERATURE.set(read());
//! app.sleep().await // sends the states, disconnects, sleeps `interval`
//! ```
//!
//! The full, compiled version: `apps/sleepy/src/main.rs`.
//!
//! What a wake-up does differently from [`start`](super::start): the access point and the
//! address of the last wake-up are reused (no scan, no DHCP; kept in RTC memory, lost on power
//! loss); boots are not written to flash; MQTT has no last will (asleep is not offline),
//! commands are retained (Home Assistant's command waits for the next wake-up), readings expire
//! after twice the interval, and the discovery is sent only when it changed. If the device cannot
//! reach the broker within `max_awake`, it sleeps again, longer after each failure, and forgets
//! the access point and address after two. It stays awake while a setup window is open or an
//! update is installing.

use core::cell::RefCell;

use crate::config::esp::SharedStore;
use crate::net::esp::{FastConnect, Net};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::{
    gpio::{Input, InputConfig, Pull},
    peripherals::LPWR,
    rtc_cntl::{
        SocResetReason, WakeupSource,
        sleep::{LowPower, RtcSleepConfig},
    },
    timer::timg::TimerGroup,
};
use log::{error, info, warn};
use static_cell::StaticCell;

use super::{
    App, BROKER, Board, Config, HEALTH, NETWORK, RECORDED, broker_check_task, network_check_task, spawn,
    supervisor_task,
};

/// How a deep-sleeping device sleeps.
pub struct SleepConfig {
    /// Time asleep between two wake-ups.
    pub interval: Duration,
    /// Longest time awake without reaching the broker before sleeping again.
    pub max_awake: Duration,
}

/// Why the chip is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    /// A boot: power-on, reset, update, crash.
    PowerOn,
    /// The end of a timed sleep.
    Timer,
    /// A wake-up pin.
    Pin,
    /// Anything else.
    Other,
}

/// Why the chip is running (usable before [`start_sleepy`]).
pub fn wake_reason() -> WakeReason {
    if esp_hal::system::reset_reason() != Some(SocResetReason::CoreDeepSleep) {
        return WakeReason::PowerOn;
    }
    let cause = esp_hal::system::wakeup_cause();
    // The C3 has no EXT1: its wake-up pins report as GPIO.
    #[cfg(not(feature = "esp32c3"))]
    let ext1 = cause.contains(WakeupSource::Ext1);
    #[cfg(feature = "esp32c3")]
    let ext1 = false;
    if cause.contains(WakeupSource::Timer) {
        WakeReason::Timer
    } else if cause.contains(WakeupSource::Gpio) || ext1 {
        WakeReason::Pin
    } else {
        WakeReason::Other
    }
}

// The words kept in RTC memory across sleeps.
const MAGIC: usize = 0;
const BOOTS: usize = 1;
const DISCOVERY_HASH: usize = 2;
const AP_CHANNEL: usize = 3;
const AP_BSSID: usize = 4; // 2 words
const IP: usize = 6;
const PREFIX: usize = 7;
const GATEWAY: usize = 8;
const DNS: usize = 9;
const FAILURES: usize = 10;
const LEASE_AGE: usize = 11;
const WORDS: usize = 12;
const MAGIC_VALUE: u32 = 0x5130_0001;
/// A lease older than this is asked again (most routers lease for an hour or more).
const LEASE_REUSE_S: u32 = 30 * 60;

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC: [u32; WORDS] = [0; WORDS];

fn rtc<R>(f: impl FnOnce(&mut [u32; WORDS]) -> R) -> R {
    let rtc: *mut [u32; WORDS] = &raw mut RTC;
    // SAFETY: core 0 only, from tasks (never an interrupt), never across an await.
    f(unsafe { &mut *rtc })
}

/// Validates the block (cleared after a power loss or any boot that is not a wake-up), counts
/// the boot.
fn rtc_init(woke: bool) -> u32 {
    rtc(|r| {
        if r[MAGIC] != MAGIC_VALUE || !woke {
            let boots = if r[MAGIC] == MAGIC_VALUE { r[BOOTS] } else { 0 };
            *r = [0; WORDS];
            r[MAGIC] = MAGIC_VALUE;
            r[BOOTS] = boots;
        }
        r[BOOTS] = r[BOOTS].wrapping_add(1);
        r[BOOTS]
    })
}

fn bssid_words(b: [u8; 6]) -> [u32; 2] {
    [u32::from_le_bytes([b[0], b[1], b[2], b[3]]), u32::from_le_bytes([b[4], b[5], 0, 0])]
}

fn bssid(w: [u32; 2]) -> [u8; 6] {
    let [a, b, c, d] = w[0].to_le_bytes();
    let [e, f, ..] = w[1].to_le_bytes();
    [a, b, c, d, e, f]
}

/// What the last wake-up remembered.
fn fast_connect() -> FastConnect {
    rtc(|r| {
        let ap = (r[AP_CHANNEL] != 0).then(|| (r[AP_CHANNEL] as u8, bssid([r[AP_BSSID], r[AP_BSSID + 1]])));
        let ipv4 = (r[IP] != 0 && r[LEASE_AGE] < LEASE_REUSE_S).then(|| {
            let ip = |w: u32| core::net::Ipv4Addr::from(w);
            let mut dns = heapless::Vec::new();
            if r[DNS] != 0 {
                let _ = dns.push(ip(r[DNS]));
            }
            embassy_net::StaticConfigV4 {
                address: embassy_net::Ipv4Cidr::new(ip(r[IP]), r[PREFIX] as u8),
                gateway: (r[GATEWAY] != 0).then(|| ip(r[GATEWAY])),
                dns_servers: dns,
            }
        });
        FastConnect { ap, ipv4 }
    })
}

/// Keeps the access point and the address of a wake-up that reached the broker.
fn remember(net: Net) {
    let ap = crate::net::esp::connected_ap();
    let lease = net.stack.config_v4();
    rtc(|r| {
        if let Some((channel, b)) = ap {
            r[AP_CHANNEL] = u32::from(channel);
            [r[AP_BSSID], r[AP_BSSID + 1]] = bssid_words(b);
        }
        if let Some(v4) = lease
            && r[IP] != u32::from(v4.address.address())
        {
            r[IP] = v4.address.address().into();
            r[PREFIX] = u32::from(v4.address.prefix_len());
            r[GATEWAY] = v4.gateway.map_or(0, u32::from);
            r[DNS] = v4.dns_servers.first().map_or(0, |d| u32::from(*d));
            r[LEASE_AGE] = 0;
        }
        r[FAILURES] = 0;
    });
}

fn forget_network() {
    rtc(|r| {
        r[AP_CHANNEL] = 0;
        r[IP] = 0;
    });
}

static LPWR_PERIPHERAL: embassy_sync::blocking_mutex::Mutex<CriticalSectionRawMutex, RefCell<Option<LPWR<'static>>>> =
    embassy_sync::blocking_mutex::Mutex::new(RefCell::new(None));

/// An update is asked for (Home Assistant's Install may come in with the retained commands,
/// while the update task still checks the manifest) or installs.
fn installing() -> bool {
    #[cfg(feature = "ha-update")]
    if super::update::FIRMWARE.install_requested() {
        return true;
    }
    crate::ota::esp::installing()
}

/// Sleeps `duration` (wake-up pins armed with esp-hal's `Input::wakeup_enable` stay armed).
fn deep_sleep(duration: Duration) -> ! {
    let lpwr = LPWR_PERIPHERAL.lock(|l| l.borrow_mut().take());
    let Some(lpwr) = lpwr else {
        error!("sleep: no LPWR peripheral; restarting instead");
        esp_hal::system::software_reset()
    };
    let awake = Instant::now().as_secs().saturating_add(duration.as_secs());
    rtc(|r| r[LEASE_AGE] = r[LEASE_AGE].saturating_add(awake.min(u64::from(u32::MAX)) as u32));
    let mut low_power = LowPower::new(lpwr);
    let micros = duration.as_micros().max(10_000);
    low_power.set_wakeup_deadline(esp_hal::time::Instant::now() + esp_hal::time::Duration::from_micros(micros));
    info!("sleep: {} s", duration.as_secs());
    if usb_host_attached() {
        // A serial monitor loses what it has not read when the port goes away.
        busy_wait(esp_hal::time::Duration::from_millis(200));
    }
    low_power.sleep_deep(RtcSleepConfig::deep())
}

/// The USB host sends a start-of-frame every millisecond; on battery nothing counts them.
fn usb_host_attached() -> bool {
    let frame = || esp_hal::peripherals::USB_DEVICE::regs().fram_num().read().sof_frame_index().bits();
    let before = frame();
    busy_wait(esp_hal::time::Duration::from_millis(3));
    frame() != before
}

fn busy_wait(duration: esp_hal::time::Duration) {
    let start = esp_hal::time::Instant::now();
    while start.elapsed() < duration {}
}

/// When the broker was reached in this wake-up.
static CONNECTED_AT: embassy_sync::blocking_mutex::Mutex<CriticalSectionRawMutex, core::cell::Cell<Option<Instant>>> =
    embassy_sync::blocking_mutex::Mutex::new(core::cell::Cell::new(None));

/// A started deep-sleeping device.
#[derive(Clone, Copy)]
pub struct SleepyApp {
    /// The running kit.
    pub app: App,
    /// Why the chip is running.
    pub wake_reason: WakeReason,
    /// Boots and wake-ups since the last power loss.
    pub boots: u32,
    sleep: &'static SleepConfig,
}

impl SleepyApp {
    /// Waits until the broker is reached and the retained commands had time to arrive (they
    /// come right after the connection): then the app knows what Home Assistant asked.
    /// `false` if not reached within [`SleepConfig::max_awake`].
    pub async fn settled(&self) -> bool {
        if with_timeout(self.sleep.max_awake, crate::mqtt::esp::wait_connected()).await.is_err() {
            return false;
        }
        let at = CONNECTED_AT.lock(core::cell::Cell::get).unwrap_or_else(Instant::now);
        Timer::at(at + Duration::from_millis(300)).await;
        true
    }

    /// Sends the states, disconnects and sleeps [`SleepConfig::interval`].
    pub async fn sleep(&self) -> ! {
        self.sleep_for(self.sleep.interval).await
    }

    /// The same, for `duration`.
    pub async fn sleep_for(&self, duration: Duration) -> ! {
        // Not while an update installs, or a setup window is open.
        while installing() || crate::provision::esp::window_open() {
            Timer::after_secs(1).await;
        }
        if self.settled().await {
            rtc(|r| r[DISCOVERY_HASH] = crate::mqtt::esp::discovery_hash());
        }
        // MQTT sends what is pending and disconnects.
        let _ = with_timeout(Duration::from_secs(5), crate::runtime::shutdown::shutdown()).await;
        deep_sleep(duration)
    }
}

/// [`start`](super::start) for a deep-sleeping device (see the [module](self) docs). `lpwr`:
/// the low-power peripheral, which sleeps the chip.
pub async fn start_sleepy(
    spawner: Spawner,
    board: Board,
    lpwr: LPWR<'static>,
    config: &'static Config,
    sleep: &'static SleepConfig,
) -> SleepyApp {
    let wake_reason = wake_reason();
    let woke = wake_reason != WakeReason::PowerOn;
    let boots = rtc_init(woke);
    LPWR_PERIPHERAL.lock(|l| l.replace(Some(lpwr)));
    let boot = crate::diag::esp::boot();
    esp_println::logger::init_logger_from_env();
    let fw = config.firmware;
    info!("{} {} booted ({wake_reason:?}, #{boots}), reset cause: {:?}", fw.product, fw.version, boot.cause);
    if let Some(crash) = &boot.crash {
        error!("previous run crashed: {crash}");
    }
    for &check in config.checks {
        HEALTH.require(check);
    }

    crate::config::esp::init(board.flash);
    crate::ota::esp::early_guard();
    static STORE: StaticCell<SharedStore> = StaticCell::new();
    let store = match crate::config::esp::open().await {
        Ok(store) => &*STORE.init(Mutex::new(store)),
        Err(e) => panic!("config: {e}"),
    };
    // A wake-up is not a boot: no flash write each time.
    let stats = if woke {
        HEALTH.pass(RECORDED);
        None
    } else {
        match crate::diag::record_boot(&mut *store.lock().await, &boot).await {
            Ok((stats, _)) => {
                HEALTH.pass(RECORDED);
                Some(stats)
            }
            Err(e) => {
                warn!("could not record the boot: {e}");
                None
            }
        }
    };
    crate::ota::esp::boot(store).await;
    spawn(spawner, crate::diag::esp::watchdog_task(TimerGroup::new(board.timg1).wdt, config.watches));

    let credentials = crate::net::esp::credentials(store).await;
    let unprovisioned = credentials.is_none();
    if unprovisioned {
        warn!("no Wi-Fi network stored: join the setup access point, or use Improv (Bluetooth, USB serial)");
        HEALTH.pass(NETWORK);
    }
    #[cfg(feature = "improv-ble")]
    let bt = board.bt;
    #[cfg(not(feature = "improv-ble"))]
    let bt = ();
    let net = match crate::net::esp::start_fast(spawner, board.wifi, credentials, fast_connect()) {
        Ok(net) => {
            start_services(spawner, net, store, bt, config, sleep, unprovisioned).await;
            Some(net)
        }
        Err(e) => {
            error!("Wi-Fi: {e:?}");
            HEALTH.pass(NETWORK);
            HEALTH.pass(BROKER);
            None
        }
    };
    spawn(spawner, supervisor_task(store));
    spawn(spawner, awake_guard_task(sleep));
    if let Some(pin) = board.boot_button {
        let button = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
        spawn(spawner, crate::provision::esp::button_task(button, store));
    }
    #[cfg(any(feature = "console", feature = "improv-serial"))]
    super::spawn_console(spawner, board.usb, store, net.map(|net| super::provisioner(net, store, config)), config);

    SleepyApp { app: App { store, net, stats }, wake_reason, boots, sleep }
}

async fn start_services(
    spawner: Spawner,
    net: Net,
    store: &'static SharedStore,
    bt: super::Ble,
    config: &'static Config,
    sleep: &'static SleepConfig,
    unprovisioned: bool,
) {
    spawn(spawner, network_check_task(net));
    #[cfg(feature = "ha-update")]
    let entities = super::update::with_update(config.entities);
    #[cfg(not(feature = "ha-update"))]
    let entities = config.entities;
    static DEVICE: StaticCell<crate::model::Device> = StaticCell::new();
    let device = &*DEVICE.init(crate::model::Device {
        name: net.hostname,
        manufacturer: config.manufacturer,
        model: config.model,
        sw_version: config.firmware.version_text,
        entities,
    });
    match crate::mqtt::esp::broker(store).await {
        Some(broker) => {
            let sleepy = crate::mqtt::esp::Sleepy {
                expire_after_s: (2 * sleep.interval.as_secs() + 60).min(u64::from(u32::MAX)) as u32,
                discovery_hash: rtc(|r| r[DISCOVERY_HASH]),
            };
            spawn(spawner, crate::mqtt::esp::mqtt_sleepy_task(net, device, broker, sleepy));
            spawn(spawner, broker_check_task());
            spawn(spawner, remember_task(net));
        }
        None => {
            warn!("no MQTT broker: set one with `mqtt HOST[:PORT]` on the console");
            HEALTH.pass(BROKER);
        }
    }
    #[cfg(feature = "ota-push")]
    spawn(spawner, crate::ota::esp::serve_task(net, store, config.firmware));
    #[cfg(feature = "ha-update")]
    spawn(spawner, super::update::update_task(net, store, config.firmware));
    super::spawn_provisioning(spawner, bt, super::provisioner(net, store, config), unprovisioned);
}

/// Once the broker is reached: remember how, for the next wake-up.
#[embassy_executor::task]
async fn remember_task(net: Net) {
    crate::mqtt::esp::wait_connected().await;
    CONNECTED_AT.lock(|c| c.set(Some(Instant::now())));
    remember(net);
    info!("sleep: broker reached {} ms after the start", Instant::now().as_millis());
}

/// Sleeps again when the broker is not reached within `max_awake` (not while a setup window is
/// open or an update installs): 2, 4, then 8 times longer after each failure; after two, the
/// remembered access point and address are dropped (a fresh scan and DHCP next time).
#[embassy_executor::task]
async fn awake_guard_task(sleep: &'static SleepConfig) {
    loop {
        let connected = crate::mqtt::esp::wait_connected();
        if let Either::First(()) = select(connected, Timer::after(sleep.max_awake)).await {
            return;
        }
        if installing() || crate::provision::esp::window_open() {
            continue;
        }
        let failures = rtc(|r| {
            r[FAILURES] = r[FAILURES].saturating_add(1);
            r[FAILURES]
        });
        if failures >= 2 {
            forget_network();
        }
        let factor = 1u32 << failures.min(3);
        warn!("sleep: broker not reached in {} s ({failures} in a row)", sleep.max_awake.as_secs());
        deep_sleep(sleep.interval * factor)
    }
}
