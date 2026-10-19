//! Line console on the USB serial port: diagnostics, setup, and failures on purpose. Improv
//! frames share the port.

use embassy_time::Instant;
use embedded_io_async::Read;
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtagRx};
use esp_println::{print, println};
use heapless::String;
use iotkit_config::esp::SharedStore;
use iotkit_diag::{BootStats, LastCrash};
use iotkit_net::esp::Net;
use iotkit_ota::LastUpdate;
use iotkit_provision::esp::{Provisioner, Serial};

use crate::Config;

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
  provision      open a 10-minute setup window: access point + Bluetooth (Improv)
  ota URL        install the update whose manifest is at URL (http://...)
  mqtt           MQTT broker and connection state
  mqtt HOST[:PORT] [USER PASS]   set the broker (then restarts)";

#[cfg(feature = "matter-wifi")]
const MATTER_HELP: &str = "
  matter         Matter pairing codes
  matter pair    let a(nother) controller pair for 15 minutes
  matter reset   forget every Matter pairing (restarts)";
#[cfg(not(feature = "matter-wifi"))]
const MATTER_HELP: &str = "";

#[cfg(feature = "ha-update")]
const UPDATE_HELP: &str = "
  update-url [URL]  the manifest Home Assistant's updates come from (set: checks it now)";
#[cfg(not(feature = "ha-update"))]
const UPDATE_HELP: &str = "";

const TEST_HELP: &str = "
  panic          panic (recorded, then restart)
  hang           block the executor (hardware watchdog reset after 5 s)
  reboot         software reset
  factory-reset  erase all settings and Matter pairings, restart";

#[embassy_executor::task]
pub(crate) async fn console_task(
    mut rx: UsbSerialJtagRx<'static, Async>,
    store: &'static SharedStore,
    provisioner: Option<Provisioner>,
    config: &'static Config,
) {
    let net = provisioner.map(|p| p.net);
    let mut improv = Serial::new();
    let mut line: String<64> = String::new();
    let mut buf = [0u8; 64];
    let mut last = 0u8;
    println!("Type 'help' and press Enter.");
    loop {
        let Ok(n) = rx.read(&mut buf).await;
        for &byte in &buf[..n] {
            // Improv frames (ESP Web Tools, improv-wifi.com) share the port.
            if let Some(p) = &provisioner
                && improv.feed(p, byte).await
            {
                continue;
            }
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

async fn run(cmd: &str, store: &SharedStore, net: Option<Net>, config: &Config) {
    match cmd {
        "" => {}
        "help" => {
            println!("{HELP}{UPDATE_HELP}{MATTER_HELP}{TEST_HELP}");
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
        "provision" => {
            iotkit_provision::esp::open_window();
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
        "matter" => match iotkit_link_matter::wifi::pairing_codes() {
            Some(c) => println!("pairing code {}\nQR code text {}", c.manual, c.qr),
            None => println!("Matter not started"),
        },
        #[cfg(feature = "matter-wifi")]
        "matter pair" => {
            iotkit_link_matter::wifi::open_pairing_window();
            println!("pairing window open for 15 minutes");
        }
        #[cfg(feature = "matter-wifi")]
        "matter reset" => {
            println!("forgetting every Matter pairing, restarting");
            iotkit_link_matter::wifi::forget_pairings();
        }
        "mqtt" => {
            match store.lock().await.load::<iotkit_link_mqtt::Broker>().await {
                Ok(Some(b)) => println!("broker {}:{}", b.host, b.port),
                _ => println!("no broker set"),
            }
            println!("connected: {}", iotkit_link_mqtt::esp::is_connected());
        }
        _ if cmd.starts_with("mqtt ") => {
            let mut args = cmd["mqtt ".len()..].split_whitespace();
            let address = args.next().unwrap_or("");
            let (user, password) = (args.next().unwrap_or(""), args.next().unwrap_or(""));
            match iotkit_link_mqtt::Broker::parse(address, user, password) {
                Some(broker) => match store.lock().await.save(&broker).await {
                    Ok(()) => {
                        println!("broker {}:{} saved, restarting", broker.host, broker.port);
                        iotkit_diag::esp::restart().await;
                    }
                    Err(e) => println!("could not save: {e}"),
                },
                None => println!("usage: mqtt HOST[:PORT] [USER PASSWORD]"),
            }
        }
        #[cfg(feature = "ha-update")]
        "update-url" => match crate::update::source(store).await {
            Some(url) => println!("updates from {url}"),
            None => println!("no manifest URL: `update-url http://server/<package>/manifest.json`"),
        },
        #[cfg(feature = "ha-update")]
        _ if cmd.starts_with("update-url ") => {
            let url = cmd["update-url ".len()..].trim();
            let valid = iotkit_net::http::Url::parse(url).is_ok();
            match heapless::String::try_from(url) {
                Ok(manifest_url) if valid => {
                    let source = iotkit_ota::UpdateSource { manifest_url };
                    match store.lock().await.save(&source).await {
                        Ok(()) => {
                            println!("updates from {url}, checking");
                            crate::update::SOURCE_CHANGED.signal(());
                        }
                        Err(e) => println!("could not save: {e}"),
                    }
                }
                _ => println!("usage: update-url http://server/<package>/manifest.json (200 bytes at most)"),
            }
        }
        _ if cmd.starts_with("ota ") => {
            if net.is_none() {
                println!("Wi-Fi not started");
                return;
            }
            match iotkit_ota::esp::request_install(cmd["ota ".len()..].trim()).await {
                Ok(v) => {
                    println!("installed {v}, restarting");
                    iotkit_diag::esp::restart().await;
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
        "reboot" => iotkit_diag::esp::restart().await,
        "factory-reset" => {
            match iotkit_config::esp::factory_reset(store).await {
                Ok(()) => println!("settings and Matter pairings erased, restarting"),
                Err(e) => println!("factory reset failed: {e}"),
            }
            iotkit_diag::esp::restart().await
        }
        other => {
            if !config.console.is_some_and(|ext| (ext.run)(other)) {
                println!("unknown command `{other}`, try `help`");
            }
        }
    }
}
