//! Line console on the USB serial port (`console`): diagnostics, setup, and failures on purpose.
//! Improv frames (`improv-serial`) share the port.
#![cfg_attr(not(feature = "console"), allow(dead_code, unused_imports, unused_variables))]

use crate::config::esp::SharedStore;
use crate::diag::{BootStats, LastCrash};
use crate::net::esp::Net;
use crate::ota::LastUpdate;
use crate::provision::esp::Provisioner;
#[cfg(feature = "improv-serial")]
use crate::provision::esp::Serial;
use embassy_time::Instant;
#[cfg(not(feature = "esp32"))]
use embedded_io_async::Read;
use esp_hal::Async;
#[cfg(feature = "esp32")]
use esp_hal::uart::UartRx as Rx;
#[cfg(not(feature = "esp32"))]
use esp_hal::usb::usb_serial_jtag::UsbSerialJtagRx as Rx;
use esp_println::{print, println};
use heapless::String;

use super::Config;

/// Commands of the app, next to the kit's.
#[derive(Clone, Copy)]
pub struct ConsoleExtension {
    /// Lines added to `help`, e.g. `"  blink          flash the LED"`.
    pub help: &'static str,
    /// Runs a command line; `false` if it is not one of the app's.
    pub run: fn(&str) -> bool,
}

const HELP: &str = "\
Commands:
  help           this text
  info           version, uptime, boot counters, last crash, last update
  net            Wi-Fi state, host name, IP address
  mem            heap and stack use, now and at their fullest
  provision      open a 10-minute setup window: access point + Bluetooth (Improv)
  ota URL        install the update whose manifest is at URL (http://...)";

#[cfg(feature = "mqtt")]
const MQTT_HELP: &str = "
  mqtt           MQTT broker and connection state
  mqtt HOST[:PORT] [USER PASS]   set the broker (then restarts)";
#[cfg(not(feature = "mqtt"))]
const MQTT_HELP: &str = "";

#[cfg(feature = "matter-wifi")]
const MATTER_HELP: &str = "
  matter         Matter pairing codes
  matter pair    let a(nother) controller pair for 15 minutes
  matter reset   forget every Matter pairing (restarts)";
#[cfg(not(feature = "matter-wifi"))]
const MATTER_HELP: &str = "";

#[cfg(feature = "esphome-api")]
const ESPHOME_HELP: &str = "  api-key        the ESPHome API's encryption key, for Home Assistant
  api-key KEY    set it (base64, 32 bytes; then restarts)";

const TEST_HELP: &str = "
  panic          panic (recorded, then restart)
  hang           block the executor (hardware watchdog reset after 5 s)
  reboot         software reset
  factory-reset  erase all settings and Matter pairings, restart";

#[embassy_executor::task]
pub(crate) async fn console_task(
    mut rx: Rx<'static, Async>,
    store: &'static SharedStore,
    provisioner: Option<Provisioner>,
    config: &'static Config,
) {
    let net = provisioner.map(|p| p.net);
    #[cfg(feature = "improv-serial")]
    let mut improv = Serial::new();
    let mut buf = [0u8; 64];
    #[cfg(feature = "console")]
    let (mut line, mut last) = (String::<64>::new(), 0u8);
    #[cfg(feature = "console")]
    println!("Type 'help' and press Enter.");
    loop {
        #[cfg(not(feature = "esp32"))]
        let Ok(n) = rx.read(&mut buf).await;
        // A UART can report framing or overrun errors: skip them.
        #[cfg(feature = "esp32")]
        let Ok(n) = rx.read_async(&mut buf).await else { continue };
        for &byte in &buf[..n] {
            // Improv frames (ESP Web Tools, improv-wifi.com) share the port: the console gets the
            // rest, including what looked like the start of a frame.
            #[cfg(feature = "improv-serial")]
            let console = match &provisioner {
                Some(p) => improv.feed(p, byte).await,
                None => crate::provision::improv::Released::from_slice(&[byte]).unwrap_or_default(),
            };
            #[cfg(not(feature = "improv-serial"))]
            let console = [byte];
            #[cfg(feature = "console")]
            for &byte in console.iter() {
                match byte {
                    // Terminals send \r, \n or \r\n: one Enter.
                    b'\n' if last == b'\r' => {}
                    b'\r' | b'\n' => {
                        println!();
                        run(line.trim(), store, net, config).await;
                        line.clear();
                    }
                    0x08 | 0x7f if line.pop().is_some() => print!("\x08 \x08"),
                    b' '..=b'~' if line.push(byte as char).is_ok() => print!("{}", byte as char),
                    _ => {}
                }
                last = byte;
            }
        }
    }
}

async fn run(cmd: &str, store: &SharedStore, net: Option<Net>, config: &Config) {
    match cmd {
        "" => {}
        "help" => {
            println!("{HELP}{MQTT_HELP}{MATTER_HELP}{TEST_HELP}");
            // Its own line: builds without it print as before.
            #[cfg(feature = "esphome-api")]
            println!("{ESPHOME_HELP}");
            if let Some(ext) = config.console {
                println!("{}", ext.help);
            }
        }
        "info" => {
            let fw = config.firmware;
            println!("{} {}, up {} s", fw.product, fw.version, Instant::now().as_secs());
            let mut store = store.lock().await;
            match store.load::<BootStats>().await {
                Ok(Some(s)) => println!("boots {}, crashes {} ({} in a row)", s.boots, s.crashes, s.crashes_in_a_row),
                Ok(None) => println!("no boot counters"),
                Err(e) => println!("boot counters: {e}"),
            }
            match store.load::<LastCrash>().await {
                Ok(Some(LastCrash(c))) => println!("last crash: {c}"),
                Ok(None) => println!("no crash recorded"),
                Err(e) => println!("last crash: {e}"),
            }
            if let Ok(Some(u)) = store.load::<LastUpdate>().await {
                let (a, b, c) = u.version;
                println!("last update: {a}.{b}.{c} {:?}", u.outcome);
            }
        }
        #[cfg(feature = "console")]
        "mem" => {
            let m = crate::diag::esp::memory();
            println!("heap {} of {} bytes used, {} at most", m.heap_used, m.heap_size, m.heap_peak);
            println!("stack {} of {} bytes used at most", m.stack_peak, m.stack_size);
        }
        "provision" => {
            crate::provision::esp::open_window();
            println!("setup window open for 10 minutes: access point and Bluetooth");
        }
        "net" => match net {
            Some(net) => match net.stack.config_v4() {
                Some(c) => println!("{}.local at {}", net.hostname, c.address.address()),
                None => println!("{}.local, no address yet", net.hostname),
            },
            None => println!("Wi-Fi not started"),
        },
        #[cfg(feature = "matter-wifi")]
        "matter" => match crate::matter::wifi::pairing_codes() {
            Some(c) => println!("pairing code {}\nQR code text {}", c.manual, c.qr),
            None => println!("Matter not started"),
        },
        #[cfg(feature = "matter-wifi")]
        "matter pair" => {
            crate::matter::wifi::open_pairing_window();
            println!("pairing window open for 15 minutes");
        }
        #[cfg(feature = "matter-wifi")]
        "matter reset" => {
            println!("forgetting every Matter pairing, restarting");
            crate::matter::wifi::forget_pairings();
        }
        #[cfg(feature = "mqtt")]
        "mqtt" => {
            // The one in use: set on the device (stored), else built in.
            let set = matches!(store.lock().await.load::<crate::mqtt::Broker>().await, Ok(Some(_)));
            match crate::mqtt::esp::broker(store).await {
                Some(b) => {
                    println!("broker {}:{} ({})", b.host, b.port, if set { "set on the device" } else { "built in" })
                }
                None => println!("no broker"),
            }
            println!("connected: {}", crate::mqtt::esp::is_connected());
        }
        #[cfg(feature = "mqtt")]
        _ if cmd.starts_with("mqtt ") => {
            let mut args = cmd["mqtt ".len()..].split_whitespace();
            let address = args.next().unwrap_or("");
            let (user, password) = (args.next().unwrap_or(""), args.next().unwrap_or(""));
            match crate::mqtt::Broker::parse(address, user, password) {
                Some(broker) => {
                    println!("broker {}:{} saved, restarting", broker.host, broker.port);
                    if let Err(e) = crate::mqtt::esp::set_broker(store, &broker).await {
                        println!("could not save: {e}");
                    }
                }
                None => println!("usage: mqtt HOST[:PORT] [USER PASSWORD]"),
            }
        }
        #[cfg(feature = "esphome-api")]
        "api-key" => match crate::esphome::api_key() {
            Some(key) => println!("{}", key.to_text()),
            None => println!("ESPHome API not started"),
        },
        #[cfg(feature = "esphome-api")]
        _ if cmd.starts_with("api-key ") => match crate::esphome::ApiKey::parse(cmd["api-key ".len()..].trim()) {
            Some(key) => match store.lock().await.save(&key).await {
                Ok(()) => {
                    println!("key saved, restarting");
                    crate::diag::esp::restart().await;
                }
                Err(e) => println!("could not save: {e}"),
            },
            None => println!("usage: api-key KEY (base64 of 32 bytes, as Home Assistant shows it)"),
        },
        _ if cmd.starts_with("ota ") => {
            if net.is_none() {
                println!("Wi-Fi not started");
                return;
            }
            // No task would run the install.
            #[cfg(not(any(feature = "ota-push", feature = "ha-update")))]
            println!("updates not built in (features `ota-push`, `ha-update`)");
            #[cfg(any(feature = "ota-push", feature = "ha-update"))]
            match crate::ota::esp::request_install(cmd["ota ".len()..].trim()).await {
                Ok(v) => {
                    println!("installed {v}, restarting");
                    crate::diag::esp::restart().await;
                }
                Err(e) => println!("update failed: {e}"),
            }
        }
        "panic" => panic!("panic requested on the console"),
        "hang" => {
            println!("blocking the executor");
            #[allow(clippy::empty_loop)]
            loop {}
        }
        "reboot" => crate::diag::esp::restart().await,
        "factory-reset" => {
            match crate::config::esp::factory_reset(store).await {
                Ok(()) => println!("settings and Matter pairings erased, restarting"),
                Err(e) => println!("factory reset failed: {e}"),
            }
            crate::diag::esp::restart().await
        }
        other => {
            if !config.console.is_some_and(|ext| (ext.run)(other)) {
                println!("unknown command `{other}`, try `help`");
            }
        }
    }
}
