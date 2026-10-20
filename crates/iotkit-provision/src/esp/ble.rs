//! Improv over Bluetooth LE: a GATT server, only while a provisioning window is open.

use bt_hci::{
    cmd::le::{LeConnUpdate, LeReadLocalSupportedFeatures},
    controller::{ControllerCmdAsync, ControllerCmdSync, ExternalController},
};
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_hal::peripherals::BT;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

use super::{IDENTIFY, Provisioner, closed, device_info};
use crate::improv::{
    BleCommand, Error, RPC_MAX, Rpc, SERVICE_DATA_UUID, SERVICE_UUID, State, Transport, capability, command, parse_rpc,
    rpc_result, service_data,
};

const CAPABILITIES: u8 = capability::IDENTIFY | capability::DEVICE_INFO;

#[gatt_server]
struct Server {
    improv: ImprovService,
}

#[gatt_service(uuid = "00467768-6228-2272-4663-277478268000")]
struct ImprovService {
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268001", read, notify, value = State::Authorized as u8)]
    state: u8,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268002", read, notify, value = 0)]
    error: u8,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268003", write, write_without_response)]
    rpc_command: heapless::Vec<u8, RPC_MAX>,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268004", read, notify)]
    rpc_result: heapless::Vec<u8, RPC_MAX>,
    #[characteristic(uuid = "00467768-6228-2272-4663-277478268005", read, value = CAPABILITIES)]
    capabilities: u8,
}

/// Improv over Bluetooth until the window closes (Bluetooth is off afterwards).
pub(super) async fn run_ble(bt: BT<'_>, p: &Provisioner) {
    let connector = match BleConnector::new(bt, Default::default()) {
        Ok(connector) => connector,
        Err(e) => {
            log::error!("provision: Bluetooth init failed: {e:?}");
            return closed().await;
        }
    };
    let controller: ExternalController<_, 20> = ExternalController::new(connector);
    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address()).build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let gap = GapConfig::Peripheral(PeripheralConfig { name: p.net.hostname, appearance: &appearance::UNKNOWN });
    let server = match Server::new_with_config(gap) {
        Ok(server) => server,
        Err(e) => {
            log::error!("provision: GATT server: {e:?}");
            return closed().await;
        }
    };
    log::info!("provision: Improv over Bluetooth as '{}'", p.net.hostname);
    match select(runner.run(), serve(&stack, &mut peripheral, &server, p)).await {
        Either::First(Err(e)) => {
            log::warn!("provision: Bluetooth stopped: {e:?}");
            closed().await;
        }
        _ => log::info!("provision: Bluetooth off"),
    }
}

/// A static random address from the MAC address: clients see the same device across restarts.
fn address() -> Address {
    let mac = esp_hal::efuse::base_mac_address();
    let mut bytes = [0u8; 6];
    for (b, m) in bytes.iter_mut().zip(mac.as_bytes().iter().rev()) {
        *b = *m;
    }
    bytes[5] |= 0xc0;
    Address::random(bytes)
}

/// Advertises and serves clients until the window closes.
async fn serve<C: Controller + ControllerCmdAsync<LeConnUpdate> + ControllerCmdSync<LeReadLocalSupportedFeatures>>(
    stack: &Stack<'_, C, DefaultPacketPool>,
    peripheral: &mut Peripheral<'_, C, DefaultPacketPool>,
    server: &Server<'_>,
    p: &Provisioner,
) {
    loop {
        let advertiser = match advertise(peripheral, p.net.hostname).await {
            Ok(advertiser) => advertiser,
            Err(e) => {
                log::warn!("provision: cannot advertise: {e:?}");
                Timer::after_secs(1).await;
                continue;
            }
        };
        log::info!("provision: advertising");
        let conn = match select(advertiser.accept(), closed()).await {
            Either::First(Ok(conn)) => conn,
            Either::First(Err(e)) => {
                log::warn!("provision: Bluetooth connection failed: {e:?}");
                continue;
            }
            Either::Second(()) => return,
        };
        // Joining a Wi-Fi network starves Bluetooth for a while (the radio is shared): ask for a
        // supervision timeout long enough to keep the link through it.
        let params = RequestedConnParams {
            min_connection_interval: Duration::from_millis(30),
            max_connection_interval: Duration::from_millis(50),
            max_latency: 0,
            supervision_timeout: Duration::from_secs(6),
            ..Default::default()
        };
        if let Err(e) = conn.update_connection_params(stack, &params).await {
            log::warn!("provision: connection parameters refused: {e:?}");
        }
        let Ok(conn) = conn.with_attribute_server(server) else { continue };
        log::info!("provision: Bluetooth client connected");
        let _ = server.improv.state.set(server, &(State::Authorized as u8));
        let _ = server.improv.error.set(server, &(Error::None as u8));
        let mut command = BleCommand::new();
        loop {
            let event = match select(conn.next(), closed()).await {
                Either::First(event) => event,
                Either::Second(()) => {
                    // Leave the client time to read the result.
                    Timer::after_secs(2).await;
                    return;
                }
            };
            let event = match event {
                GattConnectionEvent::Disconnected { reason } => {
                    log::info!("provision: Bluetooth client left ({reason:?})");
                    break;
                }
                GattConnectionEvent::Gatt { event } => event,
                _ => continue,
            };
            let packet = match &event {
                GattEvent::Write(write) if write.handle() == server.improv.rpc_command.handle => {
                    write.with_data(|_, data| command.push(data))
                }
                _ => None,
            };
            if let Ok(reply) = event.accept() {
                reply.send().await;
            }
            if let Some(packet) = packet {
                handle(server, &conn, p, &packet).await;
            }
        }
    }
}

async fn advertise<'p, C: Controller>(
    peripheral: &mut Peripheral<'p, C, DefaultPacketPool>,
    name: &str,
) -> Result<Advertiser<'p, C, DefaultPacketPool>, BleHostError<C::Error>> {
    let uuid = SERVICE_UUID.to_le_bytes();
    let data = service_data(State::Authorized, CAPABILITIES);
    let mut adv = [0u8; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids128(&[uuid]),
            AdStructure::ServiceData16 { uuid: SERVICE_DATA_UUID.to_le_bytes(), data: &data },
        ],
        &mut adv,
    )?;
    let mut scan = [0u8; 31];
    let scan_len = AdStructure::encode_slice(&[AdStructure::CompleteLocalName(name.as_bytes())], &mut scan)?;
    peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected { adv_data: &adv[..adv_len], scan_data: &scan[..scan_len] },
        )
        .await
}

async fn handle(server: &Server<'_>, conn: &GattConnection<'_, '_, DefaultPacketPool>, p: &Provisioner, packet: &[u8]) {
    let notify_state = async |state: State| {
        let _ = server.improv.state.notify(conn, &(state as u8), true).await;
    };
    let notify_error = async |error: Error| {
        let _ = server.improv.error.notify(conn, &(error as u8), true).await;
    };
    let notify_result = async |id: u8, strings: &[&str]| {
        let mut out = [0; RPC_MAX];
        let bytes = rpc_result(Transport::Ble, id, strings, &mut out);
        let value = heapless::Vec::from_slice(bytes).unwrap_or_default();
        let _ = server.improv.rpc_result.notify(conn, &value, true).await;
    };
    notify_error(Error::None).await;
    match parse_rpc(Transport::Ble, packet) {
        Ok(Rpc::WifiSettings { ssid, password }) => {
            notify_state(State::Provisioning).await;
            if p.provision(ssid, password).await {
                notify_result(command::WIFI_SETTINGS, &[&p.url()]).await;
                notify_state(State::Provisioned).await;
            } else {
                notify_state(State::Authorized).await;
                notify_error(Error::UnableToConnect).await;
            }
        }
        Ok(Rpc::Identify) => IDENTIFY.signal(()),
        Ok(Rpc::DeviceInfo) => notify_result(command::DEVICE_INFO, &device_info(p)).await,
        Ok(Rpc::GetState) => notify_error(Error::UnknownRpc).await,
        Err(e) => notify_error(e).await,
    }
}
