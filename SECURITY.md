# Security

## Reporting a vulnerability

Please report it privately with GitHub's **Report a vulnerability** button (the repository's
*Security* tab), not in a public issue. iotkit is at 0.1: fixes go to `main`, and the latest release is the one supported.

## What iotkit protects, and what it does not

**Firmware updates are signed.** A device installs an update only if its manifest carries a
valid Ed25519 signature from the key pair made by `cargo iotkit keygen`, names the same product,
and is newer than the running firmware (which must not be older than the manifest's
`min_version`). The image's size and SHA-256 must match the manifest. Keep `keys/ota.key` secret and backed up: whoever has it can
update your devices, and without it you cannot.

**The OTA endpoint is not authenticated.** Port 80 answers anyone on the network. A request can
make the device download a manifest, but it installs only a signed, newer image; the cost is a
download and a restart.

**Without the `tls` feature, downloads are plain HTTP and MQTT is plain TCP.** Someone on the
network can read the states and commands, and send commands. The signature still protects
updates. With `tls`, MQTT on port 8883 and `https://` downloads use TLS 1.3, and the server's
certificate must chain to the CA built into the firmware (`IOTKIT_TLS_CA`) and carry the name
or address the device connects to. The device has no clock yet: an expired certificate is still
accepted. The OTA push endpoint (port 80) stays plain: it only takes signed images.

**The ESPHome API (`esphome-api`) is encrypted with one key per device.** Whoever has it, or the
device's console, can read the states and send commands. The key is made at first start from the
hardware random generator and kept in flash with the other settings; `api-key` on the console
shows it.

**The setup window is open.** While it is open (first boot, BOOT held 3 s, or `provision` on the
console), anyone in range can give the device a Wi-Fi network: the setup access point has no
password, and Improv asks for no button press. Open it only when you need it; opened by hand,
it closes after 10 minutes.

**Settings are not encrypted in flash.** Someone with the device in hand can read the Wi-Fi and
MQTT passwords and the Matter pairings. Flash encryption and secure boot are not set up by iotkit.

**Matter uses the test attestation credentials.** Fine for development. A product needs its own
credentials.
