//! `cargo iotkit` (`cargo xtask` in the iotkit repository): create (`new`), build, sign, flash and
//! update iotkit firmware. Works on the project in the current directory: the nearest one with a
//! `partitions.csv` next to its `Cargo.toml` (`new` makes one).
//!
//! Install it with `cargo install --path xtask` from an iotkit checkout.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

mod new;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use iotkit_core::image::{Manifest, Version, app, hex_decode, hex_encode, matter, sha256};

/// The chips iotkit builds for: name (cargo feature and `espflash --chip`), ESP Web Tools' chip
/// family, target, image id.
const CHIPS: &[Chip] = &[
    Chip { name: "esp32c6", family: "ESP32-C6", target: "riscv32imac-unknown-none-elf", id: app::ESP32C6 },
    Chip { name: "esp32c3", family: "ESP32-C3", target: "riscv32imc-unknown-none-elf", id: app::ESP32C3 },
    Chip { name: "esp32c5", family: "ESP32-C5", target: "riscv32imac-unknown-none-elf", id: app::ESP32C5 },
    Chip { name: "esp32h2", family: "ESP32-H2", target: "riscv32imac-unknown-none-elf", id: app::ESP32H2 },
    Chip { name: "esp32s3", family: "ESP32-S3", target: "xtensa-esp32s3-none-elf", id: app::ESP32S3 },
    Chip { name: "esp32", family: "ESP32", target: "xtensa-esp32-none-elf", id: app::ESP32 },
];
/// Chip and flash size when neither `--chip` nor `[package.metadata.iotkit]` says.
const DEFAULT_CHIP: &str = "esp32c6";
const DEFAULT_FLASH_SIZE: &str = "8mb";
/// Toolchain for the Xtensa chips (`espup install`); `IOTKIT_ESP_TOOLCHAIN` overrides it.
const ESP_TOOLCHAIN: &str = "esp";

#[derive(Clone, Copy)]
struct Chip {
    name: &'static str,
    family: &'static str,
    target: &'static str,
    id: u16,
}

impl Chip {
    fn find(name: &str) -> Result<Self> {
        let names: Vec<_> = CHIPS.iter().map(|c| c.name).collect();
        CHIPS
            .iter()
            .copied()
            .find(|c| c.name == name)
            .with_context(|| format!("unknown chip {name} (one of {names:?})"))
    }

    fn xtensa(&self) -> bool {
        self.target.starts_with("xtensa")
    }
}
/// Key pair written by `keygen`, relative to the repository root.
const DEFAULT_KEY: &str = "keys/ota";

#[derive(Parser)]
#[command(
    name = "cargo iotkit",
    bin_name = "cargo iotkit",
    about = "iotkit host tooling: signed builds, flashing, OTA"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a device project: asks what it is, its board, power, connectivity and parts.
    New {
        /// The project's folder (default: from the device's name).
        dir: Option<PathBuf>,
        /// Take the answers from a TOML file instead of asking (see `xtask/answers/`).
        #[arg(long)]
        answers: Option<PathBuf>,
        /// Ask nothing: the defaults (a plugged-in ESP32-C6 with its LED).
        #[arg(long, conflicts_with = "answers")]
        defaults: bool,
        /// A local iotkit checkout for the project to use (default: the GitHub repository).
        #[arg(long)]
        iotkit_path: Option<PathBuf>,
    },
    /// Add a part to a project made by `cargo iotkit new`: its module in `src/parts/`, and its
    /// lines in `src/main.rs`, the README and `iotkit.toml`. Without a component: asked.
    Add {
        /// A component's id (`cargo iotkit components` lists them).
        component: Option<String>,
        /// Its name in Home Assistant (default: the component's).
        #[arg(long)]
        name: Option<String>,
        /// A pin: `4` (its first), or `ROLE=4` for a part with several; for the project's first
        /// I²C part, the bus: `sda=6`, `scl=7`. Left out: to choose later.
        #[arg(long = "pin", value_name = "[ROLE=]GPIO")]
        pins: Vec<String>,
        /// An I²C chip's address, if not its default (`0x45`).
        #[arg(long, value_parser = parse_address)]
        address: Option<u8>,
        /// The project (default: here).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// List the parts `new` and `add` know (`components/` in the repository).
    Components,
    /// Create the OTA signing key pair (`<out>.key` secret, `<out>.pub` public).
    Keygen {
        #[arg(long, default_value = DEFAULT_KEY)]
        out: PathBuf,
        /// Replace an existing key. Devices built with the old public key reject new updates.
        #[arg(long)]
        force: bool,
    },
    /// Build a firmware, save its app image and write a signed manifest to `dist/<package>/`.
    Build {
        #[command(flatten)]
        app: App,
        /// Oldest running version allowed to install this image.
        #[arg(long, default_value = "0.0.0")]
        min_version: String,
        #[command(flatten)]
        release: Release,
        /// Secret signing key.
        #[arg(long, default_value = "keys/ota.key")]
        key: PathBuf,
    },
    /// Check a manifest's signature and its image.
    Verify {
        /// Default: `dist/<package>/manifest.json`.
        manifest: Option<PathBuf>,
        /// Firmware package (default: the project's).
        #[arg(short, long)]
        package: Option<String>,
        /// Public key.
        #[arg(long, default_value = "keys/ota.pub")]
        key: PathBuf,
    },
    /// Flash over USB and open the serial monitor (`cargo run`).
    Flash {
        #[command(flatten)]
        app: App,
    },
    /// OTA commands.
    #[command(subcommand)]
    Ota(Ota),
    /// Build, then serve the install page (`dist/<package>/web/`) on this machine: open it in
    /// Chrome or Edge to flash the device over USB and set its Wi-Fi (ESP Web Tools, Improv).
    Web {
        #[command(flatten)]
        app: App,
        /// Serve the last build instead of building.
        #[arg(long)]
        no_build: bool,
        #[arg(long, default_value_t = 8000)]
        port: u16,
        #[arg(long, default_value = "0.0.0")]
        min_version: String,
        #[command(flatten)]
        release: Release,
        #[arg(long, default_value = "keys/ota.key")]
        key: PathBuf,
    },
}

#[derive(clap::Args)]
struct App {
    /// Firmware package (default: the project's, or the workspace's default member).
    #[arg(short, long)]
    package: Option<String>,
    #[arg(long)]
    release: bool,
    /// Cargo features of the firmware, e.g. `matter-wifi`.
    #[arg(short = 'F', long)]
    features: Option<String>,
    /// Chip, when not the app's default (`[package.metadata.iotkit] chip`, else esp32c6): builds
    /// with `--no-default-features --features <chip>`.
    #[arg(long)]
    chip: Option<String>,
    /// Flash size: `8mb` (`partitions.csv`) or `4mb` (`partitions-4mb.csv`). Default:
    /// `[package.metadata.iotkit] flash-size`, else 8mb.
    #[arg(long)]
    flash_size: Option<String>,
}

/// What Home Assistant shows about an update (signed with the manifest).
#[derive(clap::Args)]
struct Release {
    /// What the release changes, up to 64 bytes.
    #[arg(long, default_value = "")]
    summary: String,
    /// Where the release notes are, up to 96 bytes.
    #[arg(long, default_value = "")]
    release_url: String,
}

// Built once per run: the size difference between the variants does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Ota {
    /// Build, then have a device install it: serves `dist/` and calls the device's endpoint.
    Push {
        /// Device host name or address, e.g. `iotkit-aefc.local`.
        #[arg(long)]
        host: String,
        #[command(flatten)]
        app: App,
        /// Push the last build instead of building.
        #[arg(long)]
        no_build: bool,
        /// Port of the file server on this machine.
        #[arg(long, default_value_t = 8000)]
        port: u16,
        #[arg(long, default_value = "0.0.0")]
        min_version: String,
        #[command(flatten)]
        release: Release,
        #[arg(long, default_value = "keys/ota.key")]
        key: PathBuf,
    },
    /// Build, then copy the signed manifest and image to `DIR/<package>/`, a folder a web server
    /// serves: devices that poll its manifest (`update-url` on their console) offer the update
    /// in Home Assistant.
    Publish {
        /// The served folder (e.g. a web server's root, or a mounted share).
        #[arg(long)]
        to: PathBuf,
        /// Where `DIR` is served from, to print the manifest URL to give the devices.
        #[arg(long)]
        base_url: Option<String>,
        #[command(flatten)]
        app: App,
        /// Publish the last build instead of building.
        #[arg(long)]
        no_build: bool,
        #[arg(long, default_value = "0.0.0")]
        min_version: String,
        #[command(flatten)]
        release: Release,
        #[arg(long, default_value = "keys/ota.key")]
        key: PathBuf,
    },
    /// Build, then write the Matter OTA file `dist/<package>/<package>-<version>.ota` (its payload:
    /// the signed manifest, then the image) and its description for a Matter OTA provider
    /// (`<package>-<version>.json`: python-matter-server's `--ota-provider-dir` format).
    Build {
        #[command(flatten)]
        app: App,
        /// Use the last build instead of building.
        #[arg(long)]
        no_build: bool,
        #[arg(long, default_value = "0.0.0")]
        min_version: String,
        #[command(flatten)]
        release: Release,
        #[arg(long, default_value = "keys/ota.key")]
        key: PathBuf,
        /// The device's Matter vendor id (the test vendor's by default, as the firmware).
        #[arg(long, default_value = "0xFFF1", value_parser = parse_u16)]
        vid: u16,
        /// The device's Matter product id.
        #[arg(long, default_value = "0x8001", value_parser = parse_u16)]
        pid: u16,
        /// The bare image as the payload, for firmware built with `matter-ota-unsigned`.
        #[arg(long)]
        unsigned: bool,
    },
}

/// `0x8001` or `32769`.
fn parse_u16(s: &str) -> Result<u16, String> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => s.parse(),
    }
    .map_err(|e| format!("{s}: {e}"))
}

fn main() -> Result<()> {
    // As a cargo subcommand (`cargo iotkit ...`), cargo passes the subcommand name first.
    let args = std::env::args_os().enumerate().filter(|(i, a)| !(*i == 1 && a == "iotkit")).map(|(_, a)| a);
    let cli = Cli::parse_from(args);
    // Not in a project's root: a new project, or the given one.
    match cli.command {
        Cmd::New { dir, answers, defaults, iotkit_path } => {
            return new::run(new::NewArgs { dir, answers, defaults, iotkit_path });
        }
        Cmd::Add { component, name, pins, address, dir } => {
            return new::add::run(new::add::AddArgs { component, name, pins, address, dir });
        }
        Cmd::Components => {
            new::add::list();
            return Ok(());
        }
        _ => {}
    }
    let root = project_root()?;
    std::env::set_current_dir(&root)?;
    match cli.command {
        Cmd::New { .. } | Cmd::Add { .. } | Cmd::Components => unreachable!("handled above"),
        Cmd::Keygen { out, force } => keygen(&out, force),
        Cmd::Build { app, min_version, release, key } => build(&Build::new(app)?, &min_version, &release, &key),
        Cmd::Verify { manifest, package, key } => {
            let manifest = match manifest {
                Some(m) => m,
                None => Path::new("dist").join(Metadata::read()?.package(package)?).join("manifest.json"),
            };
            verify(&manifest, &key)
        }
        Cmd::Flash { app } => flash(&Build::new(app)?),
        Cmd::Ota(Ota::Push { host, app, no_build, port, min_version, release, key }) => {
            let app = Build::new(app)?;
            if !no_build {
                build(&app, &min_version, &release, &key)?;
            }
            push(&host, &app.package, port)
        }
        Cmd::Web { app, no_build, port, min_version, release, key } => {
            let app = Build::new(app)?;
            if !no_build {
                build(&app, &min_version, &release, &key)?;
            }
            let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("port {port}"))?;
            println!("open http://localhost:{port}/{}/web/ in Chrome or Edge (Ctrl-C stops)", app.package);
            serve_dist(listener);
            Ok(())
        }
        Cmd::Ota(Ota::Build { app, no_build, min_version, release, key, vid, pid, unsigned }) => {
            let app = Build::new(app)?;
            if !no_build {
                build(&app, &min_version, &release, &key)?;
            }
            matter_ota(&Path::new("dist").join(&app.package), vid, pid, unsigned).map(|_| ())
        }
        Cmd::Ota(Ota::Publish { to, base_url, app, no_build, min_version, release, key }) => {
            let app = Build::new(app)?;
            if !no_build {
                build(&app, &min_version, &release, &key)?;
            }
            publish(&app.package, &to, base_url.as_deref())
        }
    }
}

/// The nearest directory, from the current one up, with a `Cargo.toml` and a `partitions.csv`.
fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    cwd.ancestors()
        .find(|d| d.join("Cargo.toml").is_file() && d.join("partitions.csv").is_file())
        .map(Path::to_path_buf)
        .with_context(|| {
            format!("no iotkit project here: no `Cargo.toml` with a `partitions.csv` in {} or above", cwd.display())
        })
}

/// What `cargo metadata` says about the project.
struct Metadata(serde_json::Value);

impl Metadata {
    fn read() -> Result<Self> {
        let out = cargo().args(["metadata", "--no-deps", "--format-version", "1"]).output()?;
        ensure!(out.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
        Ok(Self(serde_json::from_slice(&out.stdout)?))
    }

    /// `package`, or the only default member of the workspace.
    fn package(&self, package: Option<String>) -> Result<String> {
        if let Some(p) = package {
            return Ok(p);
        }
        let ids = self.0["workspace_default_members"].as_array().context("cargo metadata: no default members")?;
        let [id] = ids.as_slice() else { bail!("several packages here: choose one with `-p`") };
        let found = self.packages()?.iter().find(|p| p["id"] == *id).context("default member not found")?;
        Ok(found["name"].as_str().context("package without name")?.to_owned())
    }

    fn version(&self, package: &str) -> Result<String> {
        let found =
            self.packages()?.iter().find(|p| p["name"] == package).with_context(|| format!("no package {package}"))?;
        Ok(found["version"].as_str().context("package without version")?.to_owned())
    }

    fn packages(&self) -> Result<&Vec<serde_json::Value>> {
        self.0["packages"].as_array().context("cargo metadata: no packages")
    }

    /// `[package.metadata.iotkit] <key>` of `package`.
    fn iotkit(&self, package: &str, key: &str) -> Option<String> {
        let found = self.packages().ok()?.iter().find(|p| p["name"] == package)?;
        found["metadata"]["iotkit"][key].as_str().map(str::to_owned)
    }

    fn target_dir(&self) -> PathBuf {
        self.0["target_directory"].as_str().map_or_else(|| PathBuf::from("target"), PathBuf::from)
    }
}

/// A firmware to build: the package resolved, and where cargo puts it.
struct Build {
    package: String,
    release: bool,
    features: Option<String>,
    version: String,
    target_dir: PathBuf,
    chip: Chip,
    /// Not the app's default chip: build with `--no-default-features`.
    other_chip: bool,
    flash_size: String,
}

impl Build {
    fn new(app: App) -> Result<Self> {
        let meta = Metadata::read()?;
        let package = meta.package(app.package)?;
        let version = meta.version(&package)?;
        let default_chip = meta.iotkit(&package, "chip").unwrap_or_else(|| DEFAULT_CHIP.to_owned());
        let chip = Chip::find(app.chip.as_deref().unwrap_or(&default_chip))?;
        let other_chip = chip.name != default_chip;
        let features = match (other_chip, app.features) {
            (false, features) => features,
            (true, None) => Some(chip.name.to_owned()),
            (true, Some(f)) => Some(format!("{},{f}", chip.name)),
        };
        let flash_size = app
            .flash_size
            .or_else(|| meta.iotkit(&package, "flash-size"))
            .unwrap_or_else(|| DEFAULT_FLASH_SIZE.to_owned());
        ensure!(matches!(flash_size.as_str(), "4mb" | "8mb"), "--flash-size: 4mb or 8mb");
        Ok(Self {
            version,
            target_dir: meta.target_dir(),
            package,
            release: app.release,
            features,
            chip,
            other_chip,
            flash_size,
        })
    }

    /// `cargo` for this chip: the Xtensa ones use Espressif's toolchain.
    fn cargo(&self) -> Command {
        if self.chip.xtensa() {
            let toolchain = std::env::var("IOTKIT_ESP_TOOLCHAIN").unwrap_or_else(|_| ESP_TOOLCHAIN.to_owned());
            let mut cmd = Command::new("rustup");
            cmd.args(["run", &toolchain, "cargo"]);
            cmd
        } else {
            cargo()
        }
    }

    fn cargo_args(&self) -> Vec<&str> {
        let mut args = vec!["-p", &self.package, "--target", self.chip.target];
        if self.chip.xtensa() {
            args.push("-Zbuild-std=core,alloc");
        }
        if self.release {
            args.push("--release");
        }
        if self.other_chip {
            args.push("--no-default-features");
        }
        if let Some(features) = &self.features {
            args.extend(["--features", features]);
        }
        args
    }

    fn elf(&self) -> PathBuf {
        let profile = if self.release { "release" } else { "debug" };
        self.target_dir.join(self.chip.target).join(profile).join(&self.package)
    }

    fn partition_table(&self) -> &'static str {
        if self.flash_size == "4mb" { "partitions-4mb.csv" } else { "partitions.csv" }
    }
}

/// Flashes over USB, then opens the serial monitor.
fn flash(app: &Build) -> Result<()> {
    run(app.cargo().arg("build").args(app.cargo_args()))?;
    run(Command::new("espflash")
        .args(["flash", "--monitor", "--chip", app.chip.name, "--flash-size", &app.flash_size])
        .args(["--partition-table", app.partition_table(), "--erase-parts", "otadata"])
        .arg(app.elf()))
}

fn keygen(out: &Path, force: bool) -> Result<()> {
    let (secret, public) = (out.with_extension("key"), out.with_extension("pub"));
    ensure!(force || !secret.exists(), "{} exists; pass --force to replace it", secret.display());
    let pair = ed25519_compact::KeyPair::generate();
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir)?;
    }
    write_secret(&secret, &hex(&pair.sk[..]))?;
    fs::write(&public, hex(&pair.pk[..]) + "\n")?;
    println!("wrote {} (keep it secret, it is git-ignored) and {}", secret.display(), public.display());
    Ok(())
}

#[cfg(unix)]
fn write_secret(path: &Path, contents: &str) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    writeln!(f, "{contents}")?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret(path: &Path, contents: &str) -> Result<()> {
    Ok(fs::write(path, format!("{contents}\n"))?)
}

fn build(app: &Build, min_version: &str, release: &Release, key: &Path) -> Result<()> {
    let secret: [u8; 64] = read_hex(key).context("no signing key: run `cargo iotkit keygen`")?;
    let version_str = &app.version;
    let version =
        Version::parse(version_str).map_err(|e| anyhow::anyhow!("version {version_str} of {}: {e}", app.package))?;
    let min_version = Version::parse(min_version).map_err(|e| anyhow::anyhow!("--min-version: {e}"))?;
    ensure!(min_version <= version, "--min-version {min_version} is newer than the build ({version})");

    run(app.cargo().arg("build").args(app.cargo_args()))?;

    let dir = Path::new("dist").join(&app.package);
    fs::create_dir_all(&dir)?;
    let file = format!("{}-{version}.bin", app.package);
    let image = dir.join(&file);
    run(Command::new("espflash")
        .args(["save-image", "--chip", app.chip.name, "--flash-size", &app.flash_size])
        .args(["--partition-table", app.partition_table()])
        .arg(app.elf())
        .arg(&image))?;

    let data = fs::read(&image)?;
    let mut manifest = Manifest {
        product: &app.package,
        version,
        min_version,
        size: data.len().try_into().context("image over 4 GiB")?,
        sha256: sha256(&data),
        url: &file,
        summary: &release.summary,
        release_url: &release.release_url,
        signature: [0; 64],
    };
    // The device checks the same: the app descriptor must name this package and version.
    manifest.check_app(&data, app.chip.id).map_err(|e| anyhow::anyhow!("{}: {e}", image.display()))?;
    manifest.sign(&secret).map_err(|e| anyhow::anyhow!("signing: {e}"))?;

    let mut buf = [0; 1024];
    let n = manifest.to_json(&mut buf).map_err(|e| anyhow::anyhow!("manifest: {e}"))?;
    let path = dir.join("manifest.json");
    fs::write(&path, &buf[..n])?;
    println!("{}: {} {version}, {} bytes, signed", path.display(), app.package, data.len());
    web(app, &dir.join("web"), version_str)
}

/// Writes the Matter OTA file of the build in `dir` (from its manifest and image) and the
/// provider's description of it; returns the file's path.
fn matter_ota(dir: &Path, vid: u16, pid: u16, unsigned: bool) -> Result<PathBuf> {
    let json = fs::read(dir.join("manifest.json")).with_context(|| format!("reading {}", dir.display()))?;
    let manifest = Manifest::parse(&json).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
    let image = fs::read(dir.join(manifest.url)).context("reading the image")?;
    let version = matter::version(manifest.version)
        .with_context(|| format!("version {}: Matter's needs minor and patch up to 255", manifest.version))?;
    let max = version.checked_sub(1).context("version 0.0.0: no older firmware to update")?;
    let mut payload = Vec::new();
    if !unsigned {
        payload.extend(matter::bundle::prefix(json.len()).map_err(|e| anyhow::anyhow!("manifest: {e}"))?);
        payload.extend(&json);
    }
    payload.extend(&image);
    let version_text = manifest.version.to_string();
    let min = matter::version(manifest.min_version)
        .with_context(|| format!("min_version {}: Matter's needs minor and patch up to 255", manifest.min_version))?;
    let header = matter::Header {
        vendor_id: vid,
        product_id: pid,
        version,
        version_text: &version_text,
        payload_size: payload.len() as u64,
        min_applicable: (min > 0).then_some(min),
        max_applicable: Some(max),
        release_notes_url: manifest.release_url,
        digest: sha256(&payload),
    };
    let mut file = vec![0; matter::PREFIX_LEN + matter::MAX_HEADER_LEN];
    let n = header.encode(&mut file).map_err(|e| anyhow::anyhow!("Matter OTA header: {e}"))?;
    file.truncate(n);
    file.extend(&payload);

    let name = format!("{}-{version_text}", manifest.product);
    let path = dir.join(format!("{name}.ota"));
    fs::write(&path, &file)?;
    // The provider reads `file:///<name>` from its own folder, and checks the whole file's SHA-256.
    let description = serde_json::json!({
        "modelVersion": {
            "vid": vid,
            "pid": pid,
            "softwareVersion": version,
            "softwareVersionString": version_text,
            "otaUrl": format!("file:///{name}.ota"),
            "otaChecksum": new::base64(&sha256(&file)),
            "otaChecksumType": 1,
            "minApplicableSoftwareVersion": min,
            "maxApplicableSoftwareVersion": max,
            "releaseNotesUrl": manifest.release_url,
        }
    });
    fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(&description)? + "\n")?;
    let kind = if unsigned { "bare image" } else { "signed manifest and image" };
    println!("{}: Matter OTA file, {vid:#06x}/{pid:#06x} version {version:#x} ({kind})", path.display());
    Ok(path)
}

/// The browser install (ESP Web Tools): a merged image to flash at 0 (bootloader, partition
/// table, app; otadata stays erased, so it boots `ota_0`), its manifest and a page.
fn web(app: &Build, dir: &Path, version: &str) -> Result<()> {
    fs::create_dir_all(dir)?;
    let file = format!("{}-{version}-full.bin", app.package);
    run(Command::new("espflash")
        .args(["save-image", "--merge", "--skip-padding", "--chip", app.chip.name, "--flash-size", &app.flash_size])
        .args(["--partition-table", app.partition_table()])
        .arg(app.elf())
        .arg(dir.join(&file)))?;
    let manifest = serde_json::json!({
        "name": app.package,
        "version": version,
        "new_install_prompt_erase": true,
        // Firmware with `improv-serial` takes the Wi-Fi right after the install.
        "new_install_improv_wait_time": 15,
        "builds": [{ "chipFamily": app.chip.family, "parts": [{ "path": file, "offset": 0 }] }],
    });
    fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&manifest)? + "\n")?;
    fs::write(dir.join("index.html"), INSTALL_PAGE.replace("{name}", &app.package).replace("{version}", version))?;
    println!("{}: install page (cargo iotkit web)", dir.display());
    Ok(())
}

/// `dist/<package>/web/index.html`: serve the folder over HTTPS (or localhost) for Web Serial.
const INSTALL_PAGE: &str = r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Install {name}</title>
<script type="module" src="https://unpkg.com/esp-web-tools@10/dist/web/install-button.js?module"></script>
<body style="font-family: system-ui, sans-serif; max-width: 36rem; margin: 3rem auto; padding: 0 1rem">
<h1>{name} {version}</h1>
<p>Plug the device in over USB, then install. Chrome or Edge on a computer; the device asks for
your Wi-Fi at the end.</p>
<esp-web-install-button manifest="manifest.json"></esp-web-install-button>
<p><small>If it cannot start: hold BOOT, tap RESET, release BOOT, then connect again. After the
install, tap RESET, then connect again to set the Wi-Fi.</small></p>
</body>
</html>
"#;

fn verify(path: &Path, key: &Path) -> Result<()> {
    let key: [u8; 32] = read_hex(key)?;
    let json = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let manifest = Manifest::parse(&json).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    manifest.verify(&key).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let image = path.parent().unwrap_or(Path::new(".")).join(manifest.url);
    let data = fs::read(&image).with_context(|| format!("reading {}", image.display()))?;
    manifest
        .check_image(data.len().try_into()?, &sha256(&data))
        .map_err(|e| anyhow::anyhow!("{}: {e}", image.display()))?;
    println!("OK: {} {} ({} bytes), signature and SHA-256 match", manifest.product, manifest.version, manifest.size);
    Ok(())
}

/// Serves `dist/` and asks the device at `host` to install `dist/<package>/manifest.json`.
fn push(host: &str, package: &str, port: u16) -> Result<()> {
    let manifest = Path::new("dist").join(package).join("manifest.json");
    let json = fs::read(&manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let version = Manifest::parse(&json).map_err(|e| anyhow::anyhow!("{}: {e}", manifest.display()))?.version;

    let device =
        (host, 80).to_socket_addrs().with_context(|| format!("resolving {host}"))?.next().context("no address")?;
    // The address this machine uses to reach the device: the one to give it.
    let probe = UdpSocket::bind("0.0.0.0:0")?;
    probe.connect(device)?;
    let local = probe.local_addr()?.ip();

    let listener = TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("port {port}"))?;
    thread::spawn(move || serve_dist(listener));
    let url = format!("http://{local}:{port}/{package}/manifest.json");
    println!("{host} ({device}): install {package} {version} from {url}");

    let reply = http_get(device, &format!("/ota?manifest={url}"), Duration::from_secs(180))?;
    println!("device: {}", reply.trim());
    ensure!(reply.starts_with("installed"), "the device refused the update");

    // It restarts, then answers with its version once on the network again.
    let deadline = Instant::now() + Duration::from_secs(90);
    let expected = format!("{package} {version}");
    loop {
        thread::sleep(Duration::from_secs(2));
        if let Ok(answer) = http_get(device, "/", Duration::from_secs(3)) {
            let answer = answer.trim();
            ensure!(answer == expected, "device back with `{answer}`, expected `{expected}` (rolled back?)");
            println!("device runs {answer}; it confirms the update once healthy");
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "the device did not come back within 90 s");
    }
}

/// Copies `dist/<package>/manifest.json` and its image to `to/<package>/`. The image goes
/// first: a device never sees a manifest whose image is not there yet.
fn publish(package: &str, to: &Path, base_url: Option<&str>) -> Result<()> {
    let from = Path::new("dist").join(package);
    let json = fs::read(from.join("manifest.json")).with_context(|| format!("reading {}", from.display()))?;
    let manifest = Manifest::parse(&json).map_err(|e| anyhow::anyhow!("{}: {e}", from.display()))?;
    let dir = to.join(package);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::copy(from.join(manifest.url), dir.join(manifest.url)).context("copying the image")?;
    // Written next to it, then renamed: a manifest is never half-written.
    let tmp = dir.join("manifest.json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, dir.join("manifest.json"))?;
    println!("published {package} {} to {}", manifest.version, dir.display());
    if let Some(base) = base_url {
        println!("devices: update-url {}/{package}/manifest.json", base.trim_end_matches('/'));
    }
    Ok(())
}

/// A GET over plain HTTP/1.1; returns the body of a 2xx response, or an error with it.
fn http_get(addr: SocketAddr, path: &str, timeout: Duration) -> Result<String> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(timeout))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", addr.ip())?;
    // Read the head, then exactly Content-Length bytes: do not wait for the connection to close.
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    let mut length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let mut body = vec![0; length.context("response without Content-Length")?];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).into_owned();
    let status = status_line.split(' ').nth(1).unwrap_or("");
    ensure!(status.starts_with('2'), "HTTP {status}: {}", body.trim());
    Ok(body)
}

/// Minimal static file server for `dist/`, one thread per request.
fn serve_dist(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        thread::spawn(move || {
            if let Err(e) = serve_file(stream) {
                eprintln!("file server: {e}");
            }
        });
    }
}

fn serve_file(mut stream: TcpStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    // Headers: only `Range: bytes=N-` matters (the device resumes interrupted downloads).
    let mut from = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? <= 2 {
            break;
        }
        if let Some((name, value)) = header.trim_end().split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            from = value
                .trim()
                .strip_prefix("bytes=")
                .and_then(|r| r.strip_suffix('-'))
                .and_then(|n| n.parse().ok())
                .context("unsupported Range")?;
        }
    }
    let path = line.split(' ').nth(1).unwrap_or("/");
    // A folder (the install page's URL) serves its `index.html`.
    let relative = match path.trim_start_matches('/') {
        r if r.is_empty() || r.ends_with('/') => format!("{r}index.html"),
        r => r.to_string(),
    };
    let file = Path::new("dist").join(&relative);
    let body = if relative.split('/').any(|p| p == ".." || p.is_empty()) { None } else { fs::read(&file).ok() };
    match body {
        Some(body) if from <= body.len() => {
            let rest = &body[from..];
            if from > 0 {
                println!("served {path} from byte {from} ({} bytes)", rest.len());
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len().max(1) - 1,
                    body.len(),
                    rest.len()
                )?;
            } else {
                println!("served {path} ({} bytes)", body.len());
                let kind = match file.extension().and_then(|e| e.to_str()) {
                    Some("html") => "text/html; charset=utf-8",
                    Some("json") => "application/json",
                    _ => "application/octet-stream",
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )?;
            }
            stream.write_all(rest)?;
            // Let the client close first: closing here could reset the connection before the
            // last bytes are read.
            stream.shutdown(std::net::Shutdown::Write)?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let _ = stream.read(&mut [0; 16]);
        }
        _ => {
            println!("not found: {path}");
            write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        }
    }
    Ok(())
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {:?}", cmd.get_program()))?;
    ensure!(status.success(), "{:?} failed ({status})", cmd.get_program());
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = vec![0; 2 * bytes.len()];
    hex_encode(bytes, &mut out).to_owned()
}

fn read_hex<const N: usize>(path: &Path) -> Result<[u8; N]> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    hex_decode(text.trim()).map_err(|_| anyhow::anyhow!("{}: expected {} hex bytes", path.display(), N))
}

/// `0x45` or `69`.
fn parse_address(s: &str) -> Result<u8, String> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u8::from_str_radix(hex, 16),
        None => s.parse(),
    }
    .map_err(|_| format!("`{s}`: an I²C address, such as 0x45"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Matter OTA file reads back as the firmware reads it: the header, then the signed
    /// manifest and the image; its description has the provider's checksum.
    #[test]
    fn matter_ota_file() {
        let dir = std::env::temp_dir().join(format!("iotkit-matter-ota-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let image = b"\xe9 an app image".repeat(100);
        let pair = ed25519_compact::KeyPair::generate();
        let mut m = Manifest {
            product: "light",
            version: Version::new(1, 2, 3),
            min_version: Version::new(1, 0, 0),
            size: image.len() as u32,
            sha256: sha256(&image),
            url: "light-1.2.3.bin",
            summary: "",
            release_url: "",
            signature: [0; 64],
        };
        m.sign(&pair.sk).unwrap();
        let mut buf = [0; 1024];
        let n = m.to_json(&mut buf).unwrap();
        fs::write(dir.join("manifest.json"), &buf[..n]).unwrap();
        fs::write(dir.join(m.url), &image).unwrap();

        let file = fs::read(matter_ota(&dir, 0xfff1, 0x8001, false).unwrap()).unwrap();
        let prefix = matter::Prefix::parse(file[..matter::PREFIX_LEN].try_into().unwrap()).unwrap();
        let body = matter::PREFIX_LEN + prefix.header_size as usize;
        let header = matter::Header::parse(&file[matter::PREFIX_LEN..body]).unwrap();
        header.check_prefix(prefix).unwrap();
        assert_eq!(
            (header.version, header.min_applicable, header.max_applicable),
            (0x01_02_03, Some(0x01_00_00), Some(0x01_02_02))
        );
        header.check_update(0xfff1, 0x8001, 0x01_00_00).unwrap();
        let payload = &file[body..];
        assert_eq!(sha256(payload), header.digest);
        let len = matter::bundle::manifest_len(payload[..6].try_into().unwrap()).unwrap();
        let bundled = Manifest::parse(&payload[6..6 + len]).unwrap();
        bundled.verify(&pair.pk).unwrap();
        assert_eq!(&payload[6 + len..], &image[..]);

        let description: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("light-1.2.3.json")).unwrap()).unwrap();
        let model = &description["modelVersion"];
        assert_eq!(model["softwareVersion"], 0x01_02_03);
        assert_eq!(model["otaUrl"], "file:///light-1.2.3.ota");
        assert_eq!(model["otaChecksum"], new::base64(&sha256(&file)));

        let bare = fs::read(matter_ota(&dir, 0xfff1, 0x8001, true).unwrap()).unwrap();
        assert!(bare.ends_with(&image) && bare.len() < file.len());
        fs::remove_dir_all(&dir).unwrap();
    }
}
