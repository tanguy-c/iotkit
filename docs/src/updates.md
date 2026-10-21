# Updates

## Pushed from your computer

```bash
cargo iotkit build --release                               # dist/<package>/: image + signed manifest
cargo iotkit verify                                        # optional: check it as a device would
cargo iotkit ota push --host iotkit-XXXX.local --release   # build, serve, have the device install it
```

`ota push` serves `dist/` from this machine (port 8000): the device must reach it. If the host
name does not resolve, use the device's address (`net` on the console). Bump `version` in
`Cargo.toml` for each update.

## From Home Assistant

With the `ha-update` feature (on in new projects) the device has a *Firmware* update entity:
Home Assistant shows the installed and latest versions, the release summary and link, and installs
with progress. The device checks a manifest URL at start, then every 6 hours.

```bash
# Build, sign and copy to a folder a web server serves:
cargo iotkit ota publish --to /srv/www --release --base-url http://192.168.1.10:8080 \
    --summary "Faster Wi-Fi" --release-url https://example.com/notes/1.2.0
# Once, on the device's console (or OTA_MANIFEST_URL at build time):
update-url http://192.168.1.10:8080/my-device/manifest.json
```

The summary and link are signed with the manifest. A failed install shows its error as the
summary and keeps the running firmware.

## What the device does

1. Checks the manifest's Ed25519 signature; the version must be newer than the running one (and
   the running one not older than `--min-version`).
2. Streams the image into the spare slot; its header must name the same product and version. A
   dropped connection resumes with an HTTP `Range` request.
3. Checks the size and SHA-256, selects the slot and restarts.
4. Gives the new firmware 3 trial boots to pass the health check: boot recorded, Wi-Fi up, broker
   reached, and the app's own checks (`Config::checks`, passed with `HEALTH.pass`). Then it keeps
   it.
5. Otherwise switches back to the previous firmware by itself. An update that crashes before its
   boots are counted is caught too, after 6 boots, by a counter in RTC memory.

## Keys and security

`cargo iotkit keygen` (run by `new`) writes `keys/ota.key`: keep it secret and backed up. Whoever
has it can update your devices; without it, you cannot. The update endpoint (port 80) answers
anyone on the network but installs only signed, newer images. Downloads and MQTT are plain HTTP
and TCP unless the firmware has the [`tls` feature](lean.md#tls). [SECURITY.md](https://github.com/tanguy-c/iotkit/blob/main/SECURITY.md) lists
what iotkit protects and what it does not.
