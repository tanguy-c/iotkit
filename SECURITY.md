# Security

## Reporting a vulnerability

Please report it privately with GitHub's **Report a vulnerability** button (the repository's
*Security* tab), not in a public issue. iotkit is not released yet: fixes go to `main`.

## What iotkit protects, and what it does not

**Firmware updates are signed.** A device installs an update only if its manifest carries a
valid Ed25519 signature from the key pair made by `cargo iotkit keygen`, names the same product,
and is newer than the running firmware (which must not be older than the manifest's
`min_version`). The image's size and SHA-256 must match the manifest. Keep `keys/ota.key` secret and backed up: whoever has it can
update your devices, and without it you cannot.

**The OTA endpoint is not authenticated.** Port 80 answers anyone on the network. A request can
make the device download a manifest, but it installs only a signed, newer image; the cost is a
download and a restart.

**Downloads are plain HTTP**, and so is MQTT: no TLS yet. Someone on the network can read the
states and commands, and send commands. The signature still protects updates. Run devices on a
network you trust.

**The setup window is open.** While it is open (first boot, BOOT held 3 s, or `provision` on the
console), anyone in range can give the device a Wi-Fi network: the setup access point has no
password, and Improv asks for no button press. Open it only when you need it; opened by hand,
it closes after 10 minutes.

**Settings are not encrypted in flash.** Someone with the device in hand can read the Wi-Fi and
MQTT passwords and the Matter pairings. Flash encryption and secure boot are not set up by iotkit.

**Matter uses the test attestation credentials.** Fine for development. A product needs its own
credentials.
