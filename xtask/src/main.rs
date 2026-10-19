//! `cargo iotkit` (`cargo xtask` in the iotkit repository): build, sign, flash and update iotkit
//! firmware. Works on the project in the current directory: the nearest one with a
//! `partitions.csv` next to its `Cargo.toml`.
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

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use iotkit_image::{Manifest, Version, app, hex_decode, hex_encode, sha256};

/// The chips iotkit builds for: name (cargo feature and `espflash --chip`), target, image id.
const CHIPS: &[Chip] = &[
    Chip { name: "esp32c6", target: "riscv32imac-unknown-none-elf", id: app::ESP32C6 },
    Chip { name: "esp32c3", target: "riscv32imc-unknown-none-elf", id: app::ESP32C3 },
    Chip { name: "esp32c5", target: "riscv32imac-unknown-none-elf", id: app::ESP32C5 },
    Chip { name: "esp32h2", target: "riscv32imac-unknown-none-elf", id: app::ESP32H2 },
    Chip { name: "esp32s3", target: "xtensa-esp32s3-none-elf", id: app::ESP32S3 },
];
/// Chip and flash size when neither `--chip` nor `[package.metadata.iotkit]` says.
const DEFAULT_CHIP: &str = "esp32c6";
const DEFAULT_FLASH_SIZE: &str = "8mb";
/// Toolchain for the Xtensa chips (`espup install`); `IOTKIT_ESP_TOOLCHAIN` overrides it.
const ESP_TOOLCHAIN: &str = "esp";

#[derive(Clone, Copy)]
struct Chip {
    name: &'static str,
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
        #[arg(long, default_value = "keys/ota.key")]
        key: PathBuf,
    },
    /// Upload a build to an update server that devices poll.
    Publish {
        #[arg(long)]
        server: String,
    },
}

fn main() -> Result<()> {
    // As a cargo subcommand (`cargo iotkit ...`), cargo passes the subcommand name first.
    let args = std::env::args_os().enumerate().filter(|(i, a)| !(*i == 1 && a == "iotkit")).map(|(_, a)| a);
    let cli = Cli::parse_from(args);
    let root = project_root()?;
    std::env::set_current_dir(&root)?;
    match cli.command {
        Cmd::Keygen { out, force } => keygen(&out, force),
        Cmd::Build { app, min_version, key } => build(&Build::new(app)?, &min_version, &key),
        Cmd::Verify { manifest, package, key } => {
            let manifest = match manifest {
                Some(m) => m,
                None => Path::new("dist").join(Metadata::read()?.package(package)?).join("manifest.json"),
            };
            verify(&manifest, &key)
        }
        Cmd::Flash { app } => flash(&Build::new(app)?),
        Cmd::Ota(Ota::Push { host, app, no_build, port, min_version, key }) => {
            let app = Build::new(app)?;
            if !no_build {
                build(&app, &min_version, &key)?;
            }
            push(&host, &app.package, port)
        }
        Cmd::Ota(Ota::Publish { server }) => bail!("ota publish to {server}: not implemented yet (M2)"),
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

fn build(app: &Build, min_version: &str, key: &Path) -> Result<()> {
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
    Ok(())
}

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
    let relative = path.trim_start_matches('/');
    let file = Path::new("dist").join(relative);
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
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
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
