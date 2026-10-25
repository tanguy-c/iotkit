//! The kit in one call: [`start`], the [`Board`] and [`Config`], the console, deep sleep and
//! updates from Home Assistant. Re-exported at the crate root. `start` runs the kit over Wi-Fi
//! (`wifi`), or Matter over Thread (`matter-thread`).

pub mod battery;
#[cfg(esp_wifi)]
mod console;
#[cfg(feature = "deep-sleep")]
pub(crate) mod sleep;
#[cfg(feature = "matter-thread")]
mod thread;
#[cfg(feature = "ha-update")]
mod update;
#[cfg(esp_wifi)]
mod wifi;

#[cfg(feature = "deep-sleep")]
pub use sleep::{SleepConfig, SleepyApp, WakeReason, start_sleepy, wake_reason};
#[cfg(feature = "matter-thread")]
pub use thread::{Board, start};
#[cfg(esp_wifi)]
pub use wifi::{Board, start};

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant};
use esp_hal::peripherals::FLASH;
use log::warn;

#[cfg(esp_wifi)]
use crate::Net;
use crate::{BootStats, Check, Entity, Firmware, Setting, SharedStore, TaskWatch};

pub use crate::health::{APP_CHECKS, BROKER, ESPHOME, HEALTH, MATTER, NETWORK, RECORDED};
#[cfg(esp_wifi)]
pub use console::ConsoleExtension;

/// What the device is: `Config::new(iotkit::firmware!(&OTA_KEY))`, then the builders.
///
/// ```ignore
/// static CONFIG: Config = Config::new(iotkit::firmware!(&OTA_KEY))
///     .manufacturer("Me")
///     .entities(&[&LED, &TEMPERATURE]);
/// ```
#[non_exhaustive]
pub struct Config {
    /// `iotkit::firmware!(&OTA_KEY)`: package name, version, update key.
    pub firmware: Firmware,
    /// Shown in Home Assistant (default `"iotkit"`).
    pub manufacturer: &'static str,
    /// Shown in Home Assistant (default: the chip, [`CHIP`]).
    pub model: &'static str,
    /// The product name over Matter (default: the package name).
    pub product_name: &'static str,
    /// The device model: every entity, in order (Matter endpoints follow it).
    pub entities: &'static [&'static dyn Entity],
    /// The app's task watches: a task that stops feeding its watch restarts the device.
    pub watches: &'static [&'static TaskWatch],
    /// The app's own health checks (numbers from [`APP_CHECKS`]), passed with `HEALTH.pass`.
    pub checks: &'static [Check],
    /// Extra console commands.
    #[cfg(esp_wifi)]
    pub console: Option<ConsoleExtension>,
    /// [`CHIP`]; read through the config, which is smaller code than the constant.
    #[cfg(esp_wifi)]
    chip: &'static str,
}

impl Config {
    /// The defaults: no entities, watches, checks or console commands.
    pub const fn new(firmware: Firmware) -> Self {
        Self {
            firmware,
            manufacturer: "iotkit",
            model: CHIP,
            product_name: firmware.product,
            entities: &[],
            watches: &[],
            checks: &[],
            #[cfg(esp_wifi)]
            console: None,
            #[cfg(esp_wifi)]
            chip: CHIP,
        }
    }
    /// See [`Config::manufacturer`](Self#structfield.manufacturer).
    pub const fn manufacturer(mut self, manufacturer: &'static str) -> Self {
        self.manufacturer = manufacturer;
        self
    }
    /// See [`Config::model`](Self#structfield.model).
    pub const fn model(mut self, model: &'static str) -> Self {
        self.model = model;
        self
    }
    /// See [`Config::product_name`](Self#structfield.product_name).
    pub const fn product_name(mut self, name: &'static str) -> Self {
        self.product_name = name;
        self
    }
    /// See [`Config::entities`](Self#structfield.entities).
    pub const fn entities(mut self, entities: &'static [&'static dyn Entity]) -> Self {
        self.entities = entities;
        self
    }
    /// See [`Config::watches`](Self#structfield.watches).
    pub const fn watches(mut self, watches: &'static [&'static TaskWatch]) -> Self {
        self.watches = watches;
        self
    }
    /// See [`Config::checks`](Self#structfield.checks).
    pub const fn checks(mut self, checks: &'static [Check]) -> Self {
        self.checks = checks;
        self
    }
    /// See [`Config::console`](Self#structfield.console).
    #[cfg(esp_wifi)]
    pub const fn console(mut self, console: ConsoleExtension) -> Self {
        self.console = Some(console);
        self
    }
}

/// The chip, as told to Improv clients (`"ESP32-C6"`, ...).
pub const CHIP: &str = if cfg!(feature = "esp32c6") {
    "ESP32-C6"
} else if cfg!(feature = "esp32c3") {
    "ESP32-C3"
} else if cfg!(feature = "esp32c5") {
    "ESP32-C5"
} else if cfg!(feature = "esp32") {
    "ESP32"
} else if cfg!(feature = "esp32h2") {
    "ESP32-H2"
} else {
    "ESP32-S3"
};

/// [`init!`]'s first step, before the heap, the RTOS and the app: sets up the shared flash
/// ([`config::esp::with`](crate::config::esp::with)), and rolls back an update that keeps crashing
/// this early.
#[doc(hidden)]
pub fn __early_boot(flash: FLASH<'static>) {
    crate::config::esp::init(flash);
    #[cfg(esp_ota)]
    crate::ota::esp::early_guard();
}

/// Sets up the chip for the kit, in `main` before [`start`]: the app descriptor, the clocks at
/// full speed, the flash, the rollback of an update that keeps crashing
/// ([`early_guard`](crate::ota::esp::early_guard)), the heap (64 KB reclaimed from the bootloader
/// plus `heap`, 96 KB by default; the Wi-Fi and Bluetooth drivers allocate) and the RTOS timer.
/// Declares `$p`, the peripherals (`FLASH`, `TIMG0` and `FROM_CPU_INTR0` are taken; `LPWR` too
/// on a Thread device on battery, `matter-icd`, whose CPU light-sleeps when idle). The app
/// reaches the flash through [`config::esp::with`](crate::config::esp::with) (its own partition:
/// [`config::esp::find_partition`](crate::config::esp::find_partition)), or keeps settings with
/// [`App::save`].
///
/// ```ignore
/// iotkit::init!(p);
/// iotkit::init!(mut p, heap: 128 * 1024);   // `mut`: to reborrow peripherals
/// ```
#[cfg(not(feature = "esp32"))]
#[macro_export]
macro_rules! init {
    (mut $p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [mut] $p, $($heap)?);
    };
    ($p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [] $p, $($heap)?);
    };
    (@go [$($mut:tt)?] $p:ident,) => {
        $crate::init!(@go [$($mut)?] $p, 96 * 1024);
    };
    (@go [$($mut:tt)?] $p:ident, $heap:expr) => {
        $crate::__private::esp_bootloader_esp_idf::esp_app_desc!();
        let $($mut)? $p = $crate::__private::esp_hal::init(
            $crate::__private::esp_hal::Config::default()
                .with_cpu_clock($crate::__private::esp_hal::clock::CpuClock::max()),
        );
        $crate::__early_boot($p.FLASH);
        $crate::__private::esp_alloc::heap_allocator!(#[$crate::__private::esp_hal::ram(reclaimed)] size: 64 * 1024);
        $crate::__private::esp_alloc::heap_allocator!(size: $heap);
        let timg0 = $crate::__private::esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        $crate::__start_rtos!($p, timg0.timer0);
    };
}

/// Starts the RTOS for [`init!`].
#[cfg(not(all(feature = "matter-thread", feature = "matter-icd")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __start_rtos {
    ($p:ident, $timer:expr) => {
        $crate::__private::esp_rtos::start($timer, $p.FROM_CPU_INTR0);
    };
}

/// Starts the RTOS for [`init!`], the CPU in light sleep whenever no task runs and nothing holds
/// it awake (the Bluetooth stack during pairing, the 802.15.4 radio while it listens:
/// [`matter::icd`](crate::matter::icd)). Takes `LPWR`.
#[cfg(all(feature = "matter-thread", feature = "matter-icd"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __start_rtos {
    ($p:ident, $timer:expr) => {
        let sleep = $crate::__private::esp_rtos::sleep::configure($p.LPWR);
        $crate::__private::esp_rtos::start_with_idle_hook($timer, $p.FROM_CPU_INTR0, sleep.light_sleep_hook);
    };
}

/// [`init!`] for the original ESP32: the heap is the 96 KB the bootloader leaves (`heap` sets
/// it, at most 96 KB).
#[cfg(feature = "esp32")]
#[macro_export]
macro_rules! init {
    (mut $p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [mut] $p, $($heap)?);
    };
    ($p:ident $(, heap: $heap:expr)?) => {
        $crate::init!(@go [] $p, $($heap)?);
    };
    (@go [$($mut:tt)?] $p:ident,) => {
        $crate::init!(@go [$($mut)?] $p, 96 * 1024);
    };
    (@go [$($mut:tt)?] $p:ident, $heap:expr) => {
        $crate::__private::esp_bootloader_esp_idf::esp_app_desc!();
        let $($mut)? $p = $crate::__private::esp_hal::init(
            $crate::__private::esp_hal::Config::default()
                .with_cpu_clock($crate::__private::esp_hal::clock::CpuClock::max()),
        );
        $crate::__early_boot($p.FLASH);
        // The ESP32's data RAM is small: the heap is the RAM the bootloader leaves (`dram2`).
        $crate::__private::esp_alloc::heap_allocator!(#[$crate::__private::esp_hal::ram(reclaimed)] size: $heap);
        let timg0 = $crate::__private::esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        $crate::__private::esp_rtos::start(timg0.timer0, $p.FROM_CPU_INTR0);
    };
}

/// The running kit.
#[derive(Clone, Copy)]
pub struct App {
    /// The settings in flash, shared with the kit (load and save the app's own).
    pub store: &'static SharedStore,
    /// `None` if the radio did not start.
    #[cfg(esp_wifi)]
    pub net: Option<Net>,
    /// Boot and crash counters (`crashes_in_a_row` lets an app stop a boot loop).
    pub stats: Option<BootStats>,
}

impl App {
    /// A setting from flash; `None` if never saved or unreadable.
    pub async fn load<S: Setting>(&self) -> Option<S> {
        match self.store.lock().await.load::<S>().await {
            Ok(value) => value,
            Err(e) => {
                warn!("config: setting {:#06x}: {e}", S::KEY);
                None
            }
        }
    }

    /// Saves a setting (only written when it changed).
    pub async fn save<S: Setting>(&self, value: &S) {
        if let Err(e) = self.store.lock().await.save(value).await {
            warn!("config: could not save setting {:#06x}: {e}", S::KEY);
        }
    }
}

/// Spawns a kit task; each runs once, so a failure is a bug.
pub(crate) fn spawn<S>(spawner: Spawner, token: Result<embassy_executor::SpawnToken<S>, embassy_executor::SpawnError>) {
    match token {
        Ok(token) => spawner.spawn(token),
        Err(e) => panic!("iotkit-app: task spawned twice: {e:?}"),
    }
}

/// Saves a setting a while after its last change, so a dragged slider writes the flash once.
///
/// ```ignore
/// let mut later = SaveLater::new(Duration::from_secs(3));
/// // on each change:
/// later.changed();
/// // now and then (e.g. on a ticker):
/// if later.due() { app.save(&state).await }
/// ```
///
/// In use: `apps/devkit/src/main.rs`.
pub struct SaveLater {
    delay: Duration,
    at: Option<Instant>,
}

impl SaveLater {
    /// Saves `delay` after the last change.
    pub const fn new(delay: Duration) -> Self {
        Self { delay, at: None }
    }

    /// The value changed: save it after the delay (counted from the last change).
    pub fn changed(&mut self) {
        self.at = Some(Instant::now() + self.delay);
    }

    /// Whether to save now; `true` once per series of changes.
    pub fn due(&mut self) -> bool {
        let due = self.at.is_some_and(|at| Instant::now() >= at);
        if due {
            self.at = None;
        }
        due
    }
}
