# Publishing updates

To update your devices without a USB cable, you publish each new version once. Your devices then
find it by themselves, and Home Assistant offers it. This page shows where to publish, and how to
run the server your devices ask. [Updates](updates.md) explains what a device does with an update.

## How it works

1. You bump `version` in `Cargo.toml`, then run `cargo iotkit ota publish --release`.
2. The CLI makes a *published build*, signs it with `keys/ota.key`, and puts the *release* in a
   *release store*: GitHub releases, or a folder.
3. Your devices find it through their *update source*:
   - Over Wi-Fi, the device asks an *update server* for its manifest, at start and every 6 hours.
     The URL is built in: `OTA_MANIFEST_URL`.
   - Over Matter (Thread, or Wi-Fi with `matter-wifi`), a *Matter OTA provider* offers the `.ota`
     file. Home Assistant's Matter Server is one.

[GLOSSARY.md](https://github.com/tanguy-c/iotkit/blob/main/GLOSSARY.md) defines these words.

Wi-Fi and Matter use two protocols, so two services serve them: an update server over HTTP, and a
Matter server over Matter. They can run on one machine, and one command publishes for both:
[iotkit-server](#run-iotkit-server) hands each new `.ota` file to the Matter server.

```text
cargo iotkit ota publish ──▶ GitHub releases or a folder
                                     │
                               iotkit-server ── checks the signature, keeps the images
                                │          │
                   HTTP, Wi-Fi  ▼          ▼  hands the .ota over
                     Wi-Fi devices      Matter server ──Matter──▶ Thread and Matter devices
```

The device checks every release's signature, whoever serves it. A server can make an update
fail, but it cannot install firmware you did not sign.

## Dev builds and published builds

`.cargo/secrets.toml` holds your Wi-Fi network, logins and keys. A published release can be read
by anyone who can download it, so the CLI keeps these values out of it:

| Command | Build | `.cargo/secrets.toml` |
| --- | --- | --- |
| `cargo run`, `cargo iotkit flash`, `cargo iotkit ota push` | dev build | built in |
| `cargo iotkit ota publish`, `cargo iotkit ota build`, `cargo iotkit build`, `cargo iotkit web` | published build | left out, but `published-secrets` |

To keep a value in published builds, list it in `Cargo.toml`:

```toml
[package.metadata.iotkit]
published-secrets = ["MQTT_USER", "MQTT_PASS"]
```

`cargo iotkit new` lists the values every device of the project needs: the broker's login, the
project's Matter pairing code, the ESPHome API key. The Wi-Fi network stays out: give it on the
device, from the setup window ([getting started](getting-started.md#join-wi-fi)).

`cargo iotkit ota publish` prints the published secrets it builds in. It refuses to publish them
to a public GitHub repository, unless you add `--allow-secrets`.

A device that joined Wi-Fi only from a dev build's `WIFI_SSID` has no network after a published
update. To store the network on the device first, plug it in over USB, then run:

```bash
cargo iotkit wifi
```

It sends the network of `.cargo/secrets.toml` over Improv (`improv-serial`, a default feature).
The device joins it, then stores it. You can also hold **BOOT** 3 s, then use the setup window.

`cargo iotkit build --dev` makes a dev build. `--no-build` publishes the last build, and
refuses it if it was a dev build.

## Choose where to publish

`cargo iotkit ota publish` reads where to publish from `Cargo.toml`, which `cargo iotkit new`
fills in:

```toml
[package.metadata.iotkit.publish]
to = "github:me/kitchen-hub"   # or a folder: "/srv/iotkit", "dir:/mnt/updates"
# tag = "v{version}"           # the GitHub tag; {package} and {version} are replaced
```

`--to` and the `IOTKIT_PUBLISH_TO` variable override it, for example in CI.

Each release holds the manifest, the app image, the whole flash for the install page, and the
`.ota` file if the firmware has `matter-ota`. The CLI refuses a version already published, or older
than one published: bump `version` in `Cargo.toml`. `--force` publishes it anyway.

To publish a test version first, add `--prerelease`. Only the update server's `beta/` channel
offers it.

### On GitHub

You need the [GitHub CLI](https://cli.github.com), logged in (`gh auth login`).

```bash
cargo iotkit ota publish --release
```

It creates the release `v<version>`, with the files as its assets. Its notes come from your
`CHANGELOG.md`, if it has a section for this version (`## [1.2.0]`, as in
[Keep a Changelog](https://keepachangelog.com)). `--summary` sets the text Home Assistant shows,
64 bytes at most.

To publish several firmwares to one repository, give each firmware its own tags. Otherwise the
CLI takes the other firmware's releases for its own, and refuses an older version:

```toml
[package.metadata.iotkit.publish]
to  = "github:me/home-devices"
tag = "{package}-v{version}"
```

To publish from GitHub Actions instead, `cargo iotkit new` writes
`.github/workflows/release.yml`. It publishes each `v*` tag you push. It needs your signing key as
the repository's `IOTKIT_OTA_KEY` secret:

```bash
gh secret set IOTKIT_OTA_KEY < keys/ota.key
git tag v1.2.0 && git push --tags
```

Whoever can run your repository's workflows can then sign updates for your devices. If that is
too much, publish from your computer.

### In a folder

```bash
cargo iotkit ota publish --release --to /srv/iotkit
```

The folder gets one subfolder per firmware, and keeps every release:

```text
/srv/iotkit/kitchen-hub/
  manifest.json                     the latest stable release (what devices ask for)
  kitchen-hub-1.2.0.manifest.json   each release's manifest
  kitchen-hub-1.2.0.bin             its app image
  kitchen-hub-1.2.0-full.bin        the whole flash, for the install page
  kitchen-hub-1.2.0.ota             with matter-ota
```

The files go first and `manifest.json` last, so a device never sees a manifest without its
image. Any web server can serve this folder as it is.

## Run iotkit-server

[iotkit-server](https://github.com/tanguy-c/iotkit-server) is the update server made for iotkit.
It reads your releases from GitHub or a folder, and serves the latest to your devices. It holds
public keys only, and takes no uploads: you can put it on the internet with little risk.

It does this for each firmware:

- It serves a release only if it is signed with that firmware's key.
- It keeps the images, so devices download from your network, over plain HTTP, even when the
  releases are on GitHub.
- It serves two channels: `/<package>/manifest.json`, the latest stable release, and
  `/<package>/beta/manifest.json`, the latest of all.
- It serves an install page for new devices: `/<package>/install/`
  ([Install from the browser](install-from-browser.md)).
- With `matter` set, it hands each new `.ota` file to the Matter server.

This `compose.yaml` runs it, and a Matter server next to it:

```yaml
services:
  iotkit-server:
    image: ghcr.io/tanguy-c/iotkit-server:0.1
    restart: unless-stopped
    ports: ["8080:8080"]
    extra_hosts: ["host.docker.internal:host-gateway"]  # this machine, for `matter`
    environment:
      # GITHUB_TOKEN: ...          # to read private repositories
      # IOTKIT_SERVER_TOKEN: ...   # allows `POST /refresh`, to read the stores at once
    volumes:
      - ./config:/config:ro        # iotkit-server.toml and the public keys
      - iotkit-data:/data          # the images it keeps
      # - /srv/iotkit:/releases:ro # a folder store

  # Without Home Assistant's Matter Server. Leave it out if you have that one.
  matter-server:
    image: ghcr.io/matter-js/matterjs-server:stable
    restart: unless-stopped
    network_mode: host             # Matter needs IPv6 multicast
    environment:
      ENABLE_TEST_NET_DCL: "true"  # offers updates of the test vendor (0xFFF1), iotkit's default
    volumes:
      - matter-data:/data          # its pairings

volumes:
  iotkit-data:
  matter-data:
```

Then write `config/iotkit-server.toml`, with one `[[product]]` per firmware:

```toml
# The Matter server, to hand it the .ota files. On this machine, as in the compose.yaml above:
# matter = "ws://host.docker.internal:5580/ws"
# On another one, such as Home Assistant's: matter = "ws://192.168.1.10:5580/ws"

[[product]]
name  = "kitchen-hub"                    # the package name
store = "github:me/kitchen-hub"          # or "dir:/releases"
key   = "kitchen-hub.pub"                # the project's keys/ota.pub
```

To read a private GitHub repository, give the server a token in `GITHUB_TOKEN`. Make it a
[fine-grained token](https://github.com/settings/personal-access-tokens/new) with only these
rights:

- *Repository access*: the repositories of your releases.
- *Permissions*: *Contents*, read-only. It covers releases and their files.

The server needs nothing more. Devices never see the token: they download from the server.

Start it with `docker compose up -d`, then open `http://<server>:8080/`: it lists each firmware's
versions. It reads the stores every 10 minutes. To read them right after publishing, call
`POST /refresh` with the token:

```bash
curl -X POST -H "Authorization: Bearer $IOTKIT_SERVER_TOKEN" http://192.168.1.10:8080/refresh
```

Build your firmware with the server's address. `cargo iotkit new` asks for it and writes it in
`.cargo/config.toml`:

```toml
[env]
OTA_MANIFEST_URL = "http://192.168.1.10:8080/kitchen-hub/manifest.json"
# The beta channel, for test devices: http://192.168.1.10:8080/kitchen-hub/beta/manifest.json
```

The install page needs Web Serial, which browsers allow only over HTTPS or from `localhost`. To use
it from another computer, put the server behind HTTPS ([Over the internet](#over-the-internet)).

## Without iotkit-server

Any web server can serve a folder release store, since the manifest names its image by a relative
path. You give up the signature check on the server, the channels, the install page and the Matter
hand-over.

### On Home Assistant

Home Assistant serves the files of its `www/` folder at `/local/`, without a login:

1. Create the folder `www/iotkit/` in Home Assistant's configuration folder. To reach it from
   your computer, use the Samba share add-on, for example.
2. Restart Home Assistant: it serves `/local/` only if `www/` existed when it started.
3. Publish there: `cargo iotkit ota publish --release --to /mnt/homeassistant/www/iotkit`.
4. Build the firmware with
   `OTA_MANIFEST_URL = "http://homeassistant.local:8123/local/iotkit/kitchen-hub/manifest.json"`.

Home Assistant tells browsers to cache these files for 31 days. Devices do not cache, so they
still see each new release.

### Any web server

A static web server works too: Caddy, nginx, or an S3-compatible store such as
[Garage](https://garagehq.deuxfleurs.fr). For a public project, GitHub Pages or Codeberg Pages can
serve the folder. Pages are HTTPS only, so the firmware needs the `tls` feature.

Do not point `OTA_MANIFEST_URL` at GitHub release assets: they redirect to a signed URL, and the
device does not follow redirects. Put iotkit-server in front.

### Over the internet

To update devices outside your network, serve the releases over HTTPS:

- Put the update server behind a reverse proxy that gets a certificate, such as Caddy.
- Build the firmware with the [`tls` feature](lean.md#tls) and an `https://` manifest URL.
- Keep secrets out of your published builds: anyone can download the release.

## Over Matter

A Matter device updates from a Matter OTA provider. With Home Assistant, that is its Matter Server
add-on (9.0 and later). Set it up once:

1. In the add-on's configuration, turn on `enable_test_net_dcl`. Without it, the provider does not
   offer images of the test vendor, 0xFFF1, which iotkit uses until you have your own vendor ID.
2. Give the provider your `.ota` files, in one of these ways:
   - With iotkit-server: set `matter` to the Matter Server's address (port 5580). It hands over
     each new stable release. The port must be reachable from iotkit-server.
   - By hand: open the Matter Server's dashboard (port 5580), then upload the `.ota` file from the
     device's page. `cargo iotkit ota build --release` writes it to `dist/<package>/`.

Home Assistant then shows the update on the device's update entity. Pre-releases stay on the beta
channel: iotkit-server hands only stable releases to the Matter server, which offers each update
to every device it matches.

### Bare images (`matter-ota-unsigned`)

With `matter-ota-unsigned`, the device takes `.ota` files without iotkit's signature, such as the
ones ESP-IDF's `ota_image_tool.py` makes. It checks only that the file is intact, and that its
chip, product and version fit.

This feature exists to work with Matter tools outside iotkit. Avoid it for devices you deploy:

- In Matter, any administrator of any fabric can point the device at a provider. A fabric is
  each ecosystem you paired the device with: Home Assistant, Google Home, Apple Home...
- With `matter-ota-unsigned`, any of them can then install any firmware on it.
- With the signed default, only whoever holds `keys/ota.key` can.

`cargo iotkit ota build` makes standard `.ota` files that any provider serves. You don't need
`matter-ota-unsigned` to work with other Matter controllers.
