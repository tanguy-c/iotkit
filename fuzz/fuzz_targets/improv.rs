//! Improv over serial (bytes mixed with console input) and over Bluetooth (GATT writes).
#![no_main]

use iotkit::provision::improv::{BleCommand, SerialDecoder, Transport, parse_rpc};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut serial = SerialDecoder::new();
    for &byte in data {
        if let Some(Ok((_, frame))) = serial.push(byte) {
            let _ = parse_rpc(Transport::Serial, frame);
        }
    }
    let mut ble = BleCommand::new();
    for chunk in data.chunks(20) {
        if let Some(packet) = ble.push(chunk) {
            let _ = parse_rpc(Transport::Ble, &packet);
        }
    }
});
