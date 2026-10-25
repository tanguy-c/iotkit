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

## Over Matter (Thread, Wi-Fi)

With `matter-ota` (on in Thread projects from `cargo iotkit new`; next to `matter-wifi` on Wi-Fi)
the device is a Matter OTA Requestor: a Matter OTA provider offers it updates, and it downloads
them over Matter (BDX), so a Thread device updates without Wi-Fi.

```bash
cargo iotkit ota build --release   # dist/<package>/<package>-<version>.ota, and its .json
```

The `.ota` file is a standard Matter OTA image (`ota_image_tool.py show` reads it). Its payload
starts with the signed manifest, then the app image: the device checks the signature, product and
version before it erases anything, then the header's SHA-256 and the image's. Matter itself only
checks a digest, and any administrator of any fabric (each ecosystem paired with the device) can
point it at a provider: the signature keeps updates to whoever holds `keys/ota.key`.
`--vid` and `--pid` (the test vendor's 0xFFF1/0x8001 by default, as the firmware) must match the
device's.

A provider serves it. Home Assistant's Matter server reads the `.json` files of its
`--ota-provider-dir` ([test-server](https://github.com/tanguy-c/iotkit/tree/main/test-server)
points it at `dist/thread-light`): the device's update entity then offers the version, and
installing announces the provider to the device (`AnnounceOTAProvider`). The device also asks its
default providers (`DefaultOTAProviders`, which a controller writes) once a day. Progress shows as
the requestor's `UpdateState`; once the update runs, the device tells the provider
(`NotifyUpdateApplied`).

Once downloaded and checked, the image waits in the spare slot, unselected, for the provider's
go-ahead (`ApplyUpdateRequest`). Other updates are not blocked meanwhile, and the image survives a
restart: when the provider offers the same file again, the device skips the download. It is
hashed again before the switch, in case another update wrote the slot since. A provider that
cannot be reached, or delays the update past 24 hours, is asked again 10 minutes later.

`matter-ota-unsigned` takes bare app images instead (from ESP-IDF's `ota_image_tool.py`, or
`cargo iotkit ota build --unsigned`), checked by the header's SHA-256, chip, product and version
only: any administrator of any fabric can then install firmware. It replaces the signature
check, it is not a fallback: a signed build never takes a bare image.

## What the device does

1. Checks the manifest's Ed25519 signature; it must be for the same product, and the version
   newer than the running one (and the running one not older than `--min-version`).
2. Streams the image into the spare slot; its header must name the same chip, product and
   version. Over HTTP a dropped connection resumes with a `Range` request; over Matter the
   transfer starts again.
3. Checks the size and SHA-256, selects the slot and restarts.
4. Runs the new firmware on trial until it passes the health check (`iotkit::health`): the boot
   recorded, the checks of the network it uses, and the app's own (`Config::checks`, passed with
   `HEALTH.pass`). Then it keeps it. A battery device stays awake until then, and another install
   is refused: the spare slot holds the firmware to go back to.

   | Network | Network up (`NETWORK`) | Broker (`BROKER`) | Matter (`MATTER`) | ESPHome API (`ESPHOME`) |
   | --- | --- | --- | --- | --- |
   | Wi-Fi | joined, with an address | reached, if one is set | with `matter-wifi` | with `esphome-api` |
   | Thread | attached (child, router or leader) | - | always | - |

   Each is skipped while there is nothing to reach: no Wi-Fi network stored, no broker, no Matter
   pairing (then no Thread network either), no Home Assistant connection yet (`ESPHOME` passes once
   Home Assistant has connected and got the states). `MATTER` passes when a controller talks to the device
   over an operational session, or when the provider of a Matter update hears that it runs.
5. Switches back to the previous firmware by itself if the new one crashes on each of its 3
   trial boots, or is not healthy within 30 minutes of a trial boot, or cannot start its radio
   (Wi-Fi, or Thread) at all (at once: it could not be updated again but over USB). An update
   that crashes before its boots are counted is caught too, after more than twice the trial
   boots, by a counter in RTC memory (lost when the power is cut).

Every app check must be able to pass within the timeout: a check that waits for something rare
(an hourly reading, say) would roll back good updates. Both limits are set at build time, in the
project's `.cargo/config.toml`:

```toml
[env]
IOTKIT_UPDATE_TRIAL_BOOTS = "3"      # 1 to 10
IOTKIT_UPDATE_TIMEOUT     = "1800"   # seconds a trial boot gets to be healthy, 60 to 86400
IOTKIT_UPDATE_TIME        = "02:00"  # deep sleep over the ESPHome API: when updates install by themselves
```

The timeout cannot tell a broken update from an outage: if the access point, the broker, the
Thread border router or the Matter controller is down for longer than the timeout during a trial, a good update is rolled back too (`info` and Home
Assistant report it rolled back; install it again once they are back). A longer timeout keeps a
good update through a longer outage; on a battery device it also means a longer wake in the
worst case (about 50 mAh for 30 minutes of Wi-Fi).

## Keys and security

`cargo iotkit keygen` (run by `new`) writes `keys/ota.key`: keep it secret and backed up. Whoever
has it can update your devices; without it, you cannot. The update endpoint (port 80, `ota-push`) answers
anyone on the network but installs only signed, newer images. Downloads and MQTT are plain HTTP
and TCP unless the firmware has the [`tls` feature](lean.md#tls). [SECURITY.md](https://github.com/tanguy-c/iotkit/blob/main/SECURITY.md) lists
what iotkit protects and what it does not.
