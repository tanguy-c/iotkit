# Updates

## Pushed from your computer

```bash
cargo iotkit build --release                               # dist/<package>/: image + signed manifest
cargo iotkit verify                                        # optional: check its signature and SHA-256
cargo iotkit ota push --host iotkit-XXXX.local --release   # build, serve, have the device install it
```

`ota push` (feature `ota-push`, on by default) serves `dist/` from this machine (port 8000): the device must reach it. If the host
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

The summary and link are signed with the manifest (64 and 96 bytes at most; the `update-url`
200). A failed install shows its error as the
summary and keeps the running firmware.

## What the device does

1. Checks the manifest's Ed25519 signature; it must be for the same product, and the version
   newer than the running one (and the running one not older than `--min-version`).
2. Streams the image into the spare slot; its header must name the same chip, product and
   version. A dropped connection resumes with an HTTP `Range` request.
3. Checks the size and SHA-256, selects the slot and restarts.
4. Runs the new firmware on trial until it passes the health check: boot recorded, Wi-Fi up,
   broker reached (each skipped when there is no network or broker to reach), and the app's own
   checks (`Config::checks`, passed with `HEALTH.pass`). Then it keeps it. A battery device stays
   awake until then, and another install is refused: the spare slot holds the firmware to go
   back to.
5. Switches back to the previous firmware by itself if the new one crashes on each of its 3
   trial boots, or is not healthy within 30 minutes of a trial boot, or cannot start Wi-Fi at
   all (at once: it could not be updated again but over USB). An update that crashes
   before its boots are counted is caught too, after more than twice the trial boots, by a
   counter in RTC memory (lost when the power is cut).

Every app check must be able to pass within the timeout: a check that waits for something rare
(an hourly reading, say) would roll back good updates. Both limits are set at build time, in the
project's `.cargo/config.toml`:

```toml
[env]
IOTKIT_UPDATE_TRIAL_BOOTS = "3"      # 1 to 10
IOTKIT_UPDATE_TIMEOUT     = "1800"   # seconds a trial boot gets to be healthy, 60 to 86400
```

The timeout cannot tell a broken update from an outage: if the access point or the broker is down
for longer than the timeout during a trial, a good update is rolled back too (`info` and Home
Assistant report it rolled back; install it again once they are back). A longer timeout keeps a
good update through a longer outage; on a battery device it also means a longer wake in the
worst case (about 50 mAh for 30 minutes of Wi-Fi).

## Keys and security

`cargo iotkit keygen` (run by `new`) writes `keys/ota.key`: keep it secret and backed up. Whoever
has it can update your devices; without it, you cannot. The update endpoint (port 80, `ota-push`) answers
anyone on the network but installs only signed, newer images. Downloads and MQTT are plain HTTP
and TCP unless the firmware has the [`tls` feature](lean.md#tls). [SECURITY.md](https://github.com/tanguy-c/iotkit/blob/main/SECURITY.md) lists
what iotkit protects and what it does not.
