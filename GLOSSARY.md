# iotkit

iotkit builds firmware for ESP32 devices and updates it over the air. These are the words its
code, CLI and guide use for builds and updates.

## Builds

**Dev build**:
A build for the developer's own devices, with the values of `.cargo/secrets.toml` built in.
_Avoid_: debug build, local build

**Published build**:
A build meant to be published: none of `.cargo/secrets.toml` is built in, except the published
secrets.
_Avoid_: release build (clashes with cargo's `--release` profile), production build

**Published secret**:
A secret the project chooses to build into its published builds as well.

## Updates

**Release**:
One version of a firmware, signed: its manifest, its image and, for Matter, its `.ota` file.
_Avoid_: update package, OTA

**Publish**:
To put a release where devices find it, through the update server or a Matter OTA provider.
_Avoid_: deploy, upload

**Push**:
To have one device install a build straight from the developer's machine.
_Avoid_: flash (that is over USB)

**Release store**:
Where releases are published and kept, such as GitHub Releases or a folder.
_Avoid_: source, repository, bucket

**Update source**:
What a device asks for its next release: a manifest URL, or a Matter OTA provider.
_Avoid_: OTA source, update URL

**Update server**:
The service devices ask for the latest release over HTTP. It only reads releases from a release
store; it never builds or signs them.
_Avoid_: OTA server (ambiguous with the OTA provider)

**OTA provider**:
The Matter node that offers releases to devices over Matter.
_Avoid_: OTA server, update server
