//! The ESP32-C6's temperature in BTHome v2 advertisements (https://bthome.io): Home Assistant's
//! Bluetooth integration finds it, no pairing or network. A small app that assembles iotkit's
//! parts by hand instead of `iotkit::start`: the crash record, the boot counters and the health
//! check, which here passes once Bluetooth advertises. No Wi-Fi, Matter or updates (no
//! `ota` engine is linked: `tools/link-check.sh` checks it).

#![no_std]
#![no_main]

use bt_hci::controller::ExternalController;
use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use esp_hal::{
    clock::CpuClock,
    timer::timg::TimerGroup,
    tsens::{self, TemperatureSensor},
};
use esp_radio::ble::controller::BleConnector;
use iotkit::config::esp::SharedStore;
use iotkit::health::{APP_CHECKS, HEALTH, RECORDED, supervisor_task};
use iotkit::{Check, image::Version};
use log::{error, info, warn};
use static_cell::StaticCell;
use trouble_host::prelude::*;

esp_bootloader_esp_idf::esp_app_desc!();

/// Bluetooth advertises.
const ADVERTISING: Check = Check(APP_CHECKS);
/// BTHome's service data UUID.
const BTHOME: u16 = 0xfcd2;
/// Between two readings.
const EVERY: Duration = Duration::from_secs(30);

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // The crash record first: a crash before this does not overwrite it.
    let boot = iotkit::diag::esp::boot();
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    iotkit::config::esp::init(p.FLASH);
    // The Bluetooth controller allocates.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("{} {} booted, reset cause: {:?}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), boot.cause);
    if let Some(crash) = &boot.crash {
        error!("previous run crashed: {crash}");
    }

    HEALTH.require(ADVERTISING);
    static STORE: StaticCell<SharedStore> = StaticCell::new();
    let store = match iotkit::config::esp::open().await {
        Ok(store) => &*STORE.init(Mutex::new(store)),
        Err(e) => panic!("config: {e}"),
    };
    match iotkit::diag::record_boot(&mut *store.lock().await, &boot).await {
        Ok((stats, _)) => {
            info!("boot #{}, {} crashes ({} in a row)", stats.boots, stats.crashes, stats.crashes_in_a_row);
            HEALTH.pass(RECORDED);
        }
        Err(e) => warn!("could not record the boot: {e}"),
    }
    let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_default();
    spawner.spawn(supervisor_task(store, version).unwrap());

    let tsens = TemperatureSensor::new(p.TSENS, tsens::Config::default()).unwrap();
    let connector = match BleConnector::new(p.BT, Default::default()) {
        Ok(connector) => connector,
        Err(e) => panic!("Bluetooth: {e:?}"),
    };
    let controller: ExternalController<_, 20> = ExternalController::new(connector);
    let mut resources: HostResources<DefaultPacketPool, 0, 0> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let beacon = async {
        let mut packet: u8 = 0;
        loop {
            let celsius = tsens.get_temperature().to_celsius();
            let mut adv = [0u8; 31];
            let Some(len) = encode(packet, celsius, &mut adv) else { return };
            // Non-connectable: the advertisement is the whole interface.
            let advertised = peripheral
                .advertise(
                    &Default::default(),
                    Advertisement::NonconnectableNonscannableUndirected { adv_data: &adv[..len] },
                )
                .await;
            match advertised {
                Ok(_advertiser) => {
                    if !HEALTH.has_passed(ADVERTISING) {
                        info!("advertising as BTHome");
                        HEALTH.pass(ADVERTISING);
                    }
                    // Advertises until dropped.
                    Timer::after(EVERY).await;
                }
                Err(e) => {
                    warn!("cannot advertise: {e:?}");
                    Timer::after(EVERY).await;
                }
            }
            packet = packet.wrapping_add(1);
        }
    };
    if let embassy_futures::select::Either::First(Err(e)) = embassy_futures::select::select(runner.run(), beacon).await
    {
        error!("Bluetooth stopped: {e:?}");
    }
    loop {
        Timer::after_secs(3600).await;
    }
}

/// The BTHome v2 advertisement: unencrypted, a packet id (receivers drop repeats) and the
/// temperature in 0.01 °C.
fn encode(packet: u8, celsius: f32, out: &mut [u8; 31]) -> Option<usize> {
    let centi = (celsius * 100.0) as i16;
    let [lo, hi] = centi.to_le_bytes();
    let data = [0x40, 0x00, packet, 0x02, lo, hi];
    AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceData16 { uuid: BTHOME.to_le_bytes(), data: &data },
            AdStructure::CompleteLocalName(b"iotkit-beacon"),
        ],
        out,
    )
    .ok()
}
