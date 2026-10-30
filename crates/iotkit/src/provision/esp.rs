//! Provisioning on the chip: the windows, Improv's serial front-end (fed by the app's console),
//! and while a window is open the Bluetooth LE GATT server and the setup access point.

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "improv-ble")]
use embassy_futures::select::select;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

use crate::config::esp::SharedStore;
use crate::net::{WifiCredentials, esp::Net};
use embassy_time::{Duration, Timer};
#[cfg(feature = "improv-ble")]
use esp_hal::peripherals::BT;
use esp_println::Printer;
use heapless::String;

use crate::provision::improv::{
    Error, FRAME_MAX, RPC_MAX, Released, Rpc, SerialDecoder, State, Transport, command, frame, parse_rpc, rpc_result,
    serial_frame,
};

#[cfg(feature = "softap")]
mod ap;
#[cfg(feature = "softap")]
pub(crate) use ap::ApBuffers;
#[cfg(feature = "softap")]
pub use ap::portal_job_task;
#[cfg(feature = "improv-ble")]
mod ble;

/// How long a requested window stays open (none is needed while no network is stored).
pub const WINDOW: Duration = Duration::from_secs(600);

/// What the device tells clients about itself.
#[derive(Clone, Copy)]
pub struct DeviceInfo {
    /// The firmware name (package name).
    pub firmware: &'static str,
    /// The firmware version.
    pub version: &'static str,
    /// The chip (`ESP32-C6`, ...).
    pub chip: &'static str,
}

/// Everything provisioning needs.
#[derive(Clone, Copy)]
pub struct Provisioner {
    /// The network to configure.
    pub net: Net,
    /// Where the network is stored once it works.
    pub store: &'static SharedStore,
    /// What Improv clients are told.
    pub info: DeviceInfo,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Set by an Improv "identify": the app can blink a LED.
pub static IDENTIFY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Opens a provisioning window: the setup access point opens and Bluetooth advertises.
pub fn open_window() {
    ACTIVE.store(true, Ordering::Relaxed);
    CHANGED.signal(());
    crate::status::notify();
}

/// Whether a provisioning window is open.
pub fn window_open() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub(crate) fn close_window() {
    ACTIVE.store(false, Ordering::Relaxed);
    CHANGED.signal(());
    crate::status::notify();
}

impl Provisioner {
    fn url(&self) -> String<48> {
        let mut url = String::new();
        let _ = write!(url, "http://{}.local/", self.net.hostname);
        url
    }

    fn state(&self) -> State {
        if self.net.is_online() { State::Provisioned } else { State::Authorized }
    }

    /// Tries the network; stores it and closes the window when it works.
    async fn provision(&self, ssid: &str, password: &str) -> bool {
        let Some(credentials) = WifiCredentials::new(ssid, password) else {
            log::warn!("provision: refused Wi-Fi '{ssid}': the password must be empty or 8 to 63 characters");
            return false;
        };
        log::info!("provision: received Wi-Fi '{ssid}'");
        if !crate::net::esp::join(credentials.clone()).await {
            return false;
        }
        if let Err(e) = self.store.lock().await.save(&credentials).await {
            log::error!("provision: could not store the Wi-Fi network: {e}");
        }
        log::info!("provision: Wi-Fi '{ssid}' stored");
        close_window();
        true
    }
}

/// Improv over the serial port shared with the console.
#[derive(Default)]
pub struct Serial {
    decoder: SerialDecoder,
}

impl Serial {
    /// A decoder with nothing received yet.
    pub const fn new() -> Self {
        Self { decoder: SerialDecoder::new() }
    }

    /// Feeds a byte read from the port; returns the bytes for the console, in order: none while
    /// they could be an Improv frame, the held ones once they are not (an `I` typed, then not
    /// `MPROV`). Answers complete frames (which can take the time of a Wi-Fi join).
    pub async fn feed(&mut self, p: &Provisioner, byte: u8) -> Released {
        let rpc = match self.decoder.push(byte) {
            None => {
                let mut console = self.decoder.take_released();
                if !self.decoder.in_frame() {
                    let _ = console.push(byte);
                }
                return console;
            }
            // Read in place: nothing else uses the decoder until the answer (a join can take a
            // while, and a copy would stay in the console's future meanwhile).
            Some(Ok((frame::RPC, data))) => data,
            Some(Ok(_)) => return Released::new(),
            Some(Err(e)) => {
                send(frame::ERROR, &[e as u8]);
                return Released::new();
            }
        };
        match parse_rpc(Transport::Serial, rpc) {
            Ok(Rpc::GetState) => {
                let state = p.state();
                send(frame::STATE, &[state as u8]);
                if state == State::Provisioned {
                    send_result(command::IDENTIFY_OR_STATE, &[&p.url()]);
                }
            }
            Ok(Rpc::DeviceInfo) => send_result(command::DEVICE_INFO, &device_info(p)),
            Ok(Rpc::WifiSettings { ssid, password }) => {
                send(frame::STATE, &[State::Provisioning as u8]);
                if p.provision(ssid, password).await {
                    send(frame::STATE, &[State::Provisioned as u8]);
                    send_result(command::WIFI_SETTINGS, &[&p.url()]);
                } else {
                    send(frame::ERROR, &[Error::UnableToConnect as u8]);
                    send(frame::STATE, &[State::Authorized as u8]);
                }
            }
            Ok(Rpc::Identify) | Err(Error::UnknownRpc) => send(frame::ERROR, &[Error::UnknownRpc as u8]),
            Err(e) => send(frame::ERROR, &[e as u8]),
        }
        Released::new()
    }
}

fn device_info(p: &Provisioner) -> [&'static str; 4] {
    [p.info.firmware, p.info.version, p.info.chip, p.net.hostname]
}

fn send(kind: u8, data: &[u8]) {
    let mut out = [0; FRAME_MAX];
    Printer::write_bytes(serial_frame(kind, data, &mut out));
}

fn send_result(id: u8, strings: &[&str]) {
    let mut out = [0; RPC_MAX];
    let result = rpc_result(Transport::Serial, id, strings, &mut out);
    send(frame::RPC_RESULT, result);
}

/// Runs the provisioning windows: the setup access point (`softap`) and Improv over Bluetooth LE
/// (`ble`) while a window is open, both off otherwise. The access point's servers work in the
/// kit's work area ([`crate::scratch`]), held only while the window is open.
#[embassy_executor::task]
pub async fn window_task(#[cfg(feature = "improv-ble")] mut bt: BT<'static>, p: Provisioner) {
    // Once: its attribute values are static cells, which cannot be made twice.
    #[cfg(feature = "improv-ble")]
    let server = ble::server(&p);
    loop {
        while !window_open() {
            CHANGED.wait().await;
        }
        #[cfg(feature = "softap")]
        let mut area = match crate::scratch::try_lease() {
            Some(area) => area,
            None => {
                log::info!("provision: the window opens once the update or download running is over");
                crate::scratch::lease().await
            }
        };
        #[cfg(feature = "softap")]
        crate::net::esp::access_point(true).await;
        let window = async {
            // With no network stored the window stays open until provisioned.
            if p.net.is_online() || has_network(&p).await {
                Timer::after(WINDOW).await;
                log::info!("provision: window closed");
                close_window();
            } else {
                core::future::pending::<()>().await;
            }
        };
        let open = async {
            #[cfg(feature = "improv-ble")]
            select(ble::run_ble(bt.reborrow(), &p, server.as_ref()), window).await;
            #[cfg(not(feature = "improv-ble"))]
            window.await;
            closed().await;
            // Leave the portal's client time to see the outcome.
            #[cfg(feature = "softap")]
            Timer::after_secs(10).await;
        };
        #[cfg(feature = "softap")]
        {
            embassy_futures::select::select(ap::servers(&p, area.access_point()), open).await;
            crate::net::esp::access_point(false).await;
        }
        #[cfg(not(feature = "softap"))]
        open.await;
    }
}

async fn has_network(p: &Provisioner) -> bool {
    crate::net::esp::has_credentials(p.store).await
}

async fn closed() {
    while window_open() {
        CHANGED.wait().await;
    }
}

pub use crate::provision::gesture::{HELD, wait_press};

/// BOOT button (active low): runs the [gestures](crate::provision::gesture); a factory reset forgets every
/// setting and Matter pairing. A short press closes an open window (a setup window once a network
/// is stored, a Matter pairing window once paired); otherwise it is the app's.
#[embassy_executor::task]
pub async fn button_task(mut button: esp_hal::gpio::Input<'static>, store: &'static SharedStore) {
    use crate::provision::gesture::Gesture;
    loop {
        match crate::provision::gesture::next(&mut button, None).await {
            #[cfg(feature = "matter-wifi")]
            Gesture::Press if crate::matter::common::pairing() => crate::matter::common::close_pairing_window(),
            Gesture::Press if window_open() => {
                // Without a network, the window is the way to get one: it stays.
                if crate::net::esp::has_credentials(store).await {
                    log::info!("provision: button pressed, window closed");
                    close_window();
                } else {
                    log::info!("provision: no Wi-Fi network yet, the window stays open");
                }
            }
            Gesture::Press => crate::provision::gesture::pressed(),
            Gesture::Provision => {
                log::info!("provision: button held, window open");
                open_window();
            }
            Gesture::FactoryReset => factory_reset(store).await,
        }
    }
}

/// The button held 10 s: forgets every setting and Matter pairing, and restarts.
pub(crate) async fn factory_reset(store: &SharedStore) -> ! {
    log::warn!("provision: button held 10 s, erasing all settings");
    if let Err(e) = crate::config::esp::factory_reset(store).await {
        log::error!("provision: factory reset failed: {e}");
    }
    crate::diag::esp::restart().await
}
