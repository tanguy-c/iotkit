//! `cargo iotkit ota publish`: puts a release where devices find it (see GLOSSARY.md): a GitHub
//! release, or a folder a web server serves.
//!
//! A folder keeps every release, flat, under `<folder>/<package>/`: `<package>-<version>.bin`,
//! `-full.bin` (the whole flash, for the install page), `.ota` with `matter-ota`,
//! `.manifest.json`, and `manifest.json`, the latest stable one. A web
//! server can serve it as it is (the manifest's image URL is relative), and the update server reads
//! all the versions from it.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use iotkit_core::image::{Manifest, Version};

use crate::{Build, Metadata, run as run_command};

/// Where releases are published: the release store.
#[derive(Debug, PartialEq)]
pub enum Target {
    /// `github:OWNER/REPO`: one GitHub release per version, tagged after `tag` (`{package}`,
    /// `{version}`).
    GitHub { repo: String, tag: String },
    /// A folder (`dir:PATH`, or a path).
    Dir(PathBuf),
}

pub struct Options {
    pub prerelease: bool,
    pub force: bool,
    pub allow_secrets: bool,
}

impl Target {
    /// `--to`, else `IOTKIT_PUBLISH_TO`, else `[package.metadata.iotkit.publish] to`.
    pub fn find(to: Option<String>, meta: &Metadata, package: &str) -> Result<Self> {
        let publish = meta.iotkit_value(package, "publish");
        let to = to
            .or_else(|| std::env::var("IOTKIT_PUBLISH_TO").ok().filter(|v| !v.is_empty()))
            .or_else(|| publish["to"].as_str().map(str::to_owned))
            .context(
                "where to publish: `--to github:OWNER/REPO` or `--to FOLDER`, or `to` in \
                 `[package.metadata.iotkit.publish]` of Cargo.toml",
            )?;
        Self::parse(&to, publish["tag"].as_str().unwrap_or("v{version}"))
    }

    fn parse(to: &str, tag: &str) -> Result<Self> {
        if let Some(repo) = to.strip_prefix("github:") {
            let parts: Vec<_> = repo.split('/').collect();
            ensure!(parts.len() == 2 && parts.iter().all(|p| !p.is_empty()), "{to}: expected github:OWNER/REPO");
            ensure!(tag.contains("{version}"), "publish tag `{tag}`: it needs `{{version}}`");
            return Ok(Self::GitHub { repo: repo.to_owned(), tag: tag.to_owned() });
        }
        let path = to.strip_prefix("dir:").unwrap_or(to);
        ensure!(!path.is_empty(), "--to: an empty folder");
        Ok(Self::Dir(PathBuf::from(path)))
    }
}

/// Publishes the release in `dist/<package>/` (its manifest and image, and `ota`).
pub fn run(app: &Build, target: &Target, ota: Option<&Path>, o: Options) -> Result<()> {
    let dist = Path::new("dist").join(&app.package);
    let manifest_path = dist.join("manifest.json");
    let json = fs::read(&manifest_path).with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest = Manifest::parse(&json).map_err(|e| anyhow::anyhow!("{}: {e}", manifest_path.display()))?;
    let mut files = vec![dist.join(manifest.url)];
    files.extend(ota.map(Path::to_path_buf));
    // The whole flash (bootloader, partition table, app): the update server's install page.
    let full = dist.join("web").join(format!("{}-{}-full.bin", manifest.product, manifest.version));
    if full.is_file() {
        files.push(full);
    }
    if !app.kept.is_empty() {
        println!(
            "warning: built in, from published-secrets: {}. Anyone who can download the release can read them.",
            app.kept.join(", ")
        );
    }
    if app.matter_unsigned {
        println!(
            "warning: `matter-ota-unsigned`: any administrator of any of the device's fabrics can install firmware on it"
        );
    }
    let release = Release { product: manifest.product, version: manifest.version, manifest: &manifest_path, files };
    match target {
        Target::Dir(root) => publish_dir(root, &release, &o),
        Target::GitHub { repo, tag } => publish_github(repo, tag, &release, !app.kept.is_empty(), &o),
    }
}

struct Release<'a> {
    product: &'a str,
    version: Version,
    /// `dist/<package>/manifest.json`.
    manifest: &'a Path,
    /// The image, the whole flash's, and the `.ota` file.
    files: Vec<PathBuf>,
}

/// Refuses a version already published, or older than one published, unless forced.
fn check_version(version: Version, published: &[Version], force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    ensure!(!published.contains(&version), "{version} is already published (--force publishes it again)");
    if let Some(newer) = published.iter().filter(|v| **v > version).max() {
        bail!("{newer} is already published, newer than {version}: bump `version` in Cargo.toml (or --force)");
    }
    Ok(())
}

fn publish_dir(root: &Path, r: &Release, o: &Options) -> Result<()> {
    let dir = root.join(r.product);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let suffix = ".manifest.json";
    let prefix = format!("{}-", r.product);
    let mut published = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(v) = name.strip_prefix(&prefix).and_then(|n| n.strip_suffix(suffix))
            && let Ok(v) = Version::parse(v)
        {
            published.push(v);
        }
    }
    check_version(r.version, &published, o.force)?;

    // The files first, the manifests last: a device never sees a manifest whose image is missing.
    for file in &r.files {
        let name = file.file_name().context("a file without a name")?;
        fs::copy(file, dir.join(name)).with_context(|| format!("copying {}", file.display()))?;
    }
    let json = fs::read(r.manifest)?;
    write_atomic(&dir.join(format!("{prefix}{}{suffix}", r.version)), &json)?;
    // `manifest.json` names the latest stable release: not a pre-release, nor an older version
    // published again.
    let current = fs::read(dir.join("manifest.json")).ok();
    let current = current.as_deref().and_then(|j| Manifest::parse(j).ok()).map(|m| m.version);
    let latest = !o.prerelease && current.is_none_or(|c| c <= r.version);
    if latest {
        write_atomic(&dir.join("manifest.json"), &json)?;
    }
    println!(
        "published {} {} to {}{}",
        r.product,
        r.version,
        dir.display(),
        if latest { "" } else { " (manifest.json unchanged: not the latest stable release)" }
    );
    Ok(())
}

/// Written next to it, then renamed: never half-written.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn publish_github(repo: &str, tag_format: &str, r: &Release, secrets: bool, o: &Options) -> Result<()> {
    if secrets && !o.allow_secrets {
        let visibility = gh_output(&["repo", "view", repo, "--json", "visibility", "--jq", ".visibility"])?;
        ensure!(
            visibility.trim() != "PUBLIC",
            "{repo} is public, and the build has published secrets: anyone could read them in the image \
             (--allow-secrets publishes anyway)"
        );
    }
    let format = tag_format.replace("{package}", r.product);
    let tag = format.replace("{version}", &r.version.to_string());
    let tags: Vec<String> = serde_json::from_str(&gh_output(&[
        "release",
        "list",
        "--repo",
        repo,
        "--limit",
        "1000",
        "--json",
        "tagName",
        "--jq",
        "[.[].tagName]",
    ])?)
    .context("gh release list: unexpected output")?;
    let published: Vec<Version> = tags.iter().filter_map(|t| tag_version(&format, t)).collect();
    check_version(r.version, &published, o.force)?;

    let mut cmd = Command::new("gh");
    if published.contains(&r.version) {
        cmd.args(["release", "upload", &tag, "--clobber", "--repo", repo]);
    } else {
        let notes = r.manifest.with_file_name("release-notes.md");
        let text = fs::read_to_string("CHANGELOG.md").ok().and_then(|c| changelog_section(&c, r.version));
        fs::write(&notes, text.unwrap_or_default())?;
        cmd.args(["release", "create", &tag, "--repo", repo, "--title", &format!("{} {}", r.product, r.version)])
            .arg("--notes-file")
            .arg(&notes);
        if o.prerelease {
            cmd.args(["--prerelease", "--latest=false"]);
        }
    }
    // The manifest's asset is `manifest.json`: the update server finds the release by it.
    cmd.args(&r.files).arg(r.manifest);
    run_command(&mut cmd)?;
    println!("published {} {} as the GitHub release {tag} of {repo}", r.product, r.version);
    Ok(())
}

fn gh_output(args: &[&str]) -> Result<String> {
    let out = Command::new("gh").args(args).output().context("running gh (GitHub CLI: https://cli.github.com)")?;
    ensure!(out.status.success(), "gh {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    Ok(String::from_utf8(out.stdout)?)
}

/// The version in `tag`, if it follows `format` (`{version}` in it, `{package}` replaced).
fn tag_version(format: &str, tag: &str) -> Option<Version> {
    let (before, after) = format.split_once("{version}")?;
    Version::parse(tag.strip_prefix(before)?.strip_suffix(after)?).ok()
}

/// The section of `version` in a Keep a Changelog file (`## [1.2.0] - 2026-11-11`, or
/// `## 1.2.0`), without its heading.
fn changelog_section(changelog: &str, version: Version) -> Option<String> {
    let version = version.to_string();
    let mut lines = changelog.lines();
    lines.find(|l| {
        l.strip_prefix("## ").is_some_and(|h| {
            let h = h.trim_start_matches('[');
            h.strip_prefix(version.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with([']', ' ']))
        })
    })?;
    let body: Vec<_> = lines.take_while(|l| !l.starts_with("## ")).collect();
    let body = body.join("\n").trim().to_owned();
    (!body.is_empty()).then_some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(
            Target::parse("github:me/hub", "v{version}").unwrap(),
            Target::GitHub { repo: "me/hub".into(), tag: "v{version}".into() }
        );
        assert!(Target::parse("github:me", "v{version}").is_err());
        assert!(Target::parse("github:me/hub", "v").is_err());
        assert_eq!(Target::parse("dir:/srv/x", "").unwrap(), Target::Dir("/srv/x".into()));
        assert_eq!(Target::parse("published", "").unwrap(), Target::Dir("published".into()));
    }

    #[test]
    fn versions() {
        let v = |s| Version::parse(s).unwrap();
        assert!(check_version(v("1.2.0"), &[v("1.1.0")], false).is_ok());
        assert!(check_version(v("1.2.0"), &[v("1.2.0")], false).is_err());
        assert!(check_version(v("1.2.0"), &[v("1.3.0")], false).is_err());
        assert!(check_version(v("1.2.0"), &[v("1.3.0")], true).is_ok());
        assert_eq!(tag_version("v{version}", "v1.2.3"), Some(v("1.2.3")));
        assert_eq!(tag_version("hub-v{version}", "v1.2.3"), None);
        assert_eq!(tag_version("v{version}", "nightly"), None);
    }

    #[test]
    fn changelog() {
        let text = "# Changelog\n\n## [Unreleased]\n\n## [1.2.0] - 2026-11-11\n\n### Fixed\n\n- A bug.\n\n## [1.1.0]\n\n- Old.\n";
        let v = |s| Version::parse(s).unwrap();
        assert_eq!(changelog_section(text, v("1.2.0")).as_deref(), Some("### Fixed\n\n- A bug."));
        assert_eq!(changelog_section(text, v("1.1.0")).as_deref(), Some("- Old."));
        assert_eq!(changelog_section(text, v("1.1.1")), None);
        assert_eq!(changelog_section("## 1.2.0\n\nNotes.\n", v("1.2.0")).as_deref(), Some("Notes."));
    }

    #[test]
    fn folder() {
        let root = std::env::temp_dir().join(format!("iotkit-publish-{}", std::process::id()));
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        let pair = ed25519_compact::KeyPair::generate();
        let release = |version: &str| {
            let image = src.join(format!("hub-{version}.bin"));
            fs::write(&image, version).unwrap();
            let url = format!("hub-{version}.bin");
            let mut m = Manifest {
                product: "hub",
                version: Version::parse(version).unwrap(),
                min_version: Version::new(0, 0, 0),
                size: version.len() as u32,
                sha256: iotkit_core::image::sha256(version.as_bytes()),
                url: &url,
                summary: "",
                release_url: "",
                signature: [0; 64],
            };
            m.sign(&pair.sk).unwrap();
            let mut buf = [0; 1024];
            let n = m.to_json(&mut buf).unwrap();
            let manifest = src.join(format!("{version}.json"));
            fs::write(&manifest, &buf[..n]).unwrap();
            (manifest, image)
        };
        let out = root.join("out");
        let publish = |version: &str, prerelease: bool, force: bool| {
            let (manifest, image) = release(version);
            let r = Release {
                product: "hub",
                version: Version::parse(version).unwrap(),
                manifest: &manifest,
                files: vec![image],
            };
            publish_dir(&out, &r, &Options { prerelease, force, allow_secrets: false })
        };
        let latest = || Manifest::parse(&fs::read(out.join("hub/manifest.json")).unwrap()).unwrap().version.to_string();

        publish("1.0.0", false, false).unwrap();
        assert_eq!(latest(), "1.0.0");
        publish("1.1.0", true, false).unwrap();
        assert_eq!(latest(), "1.0.0", "a pre-release leaves manifest.json");
        assert!(out.join("hub/hub-1.1.0.manifest.json").is_file() && out.join("hub/hub-1.1.0.bin").is_file());
        assert!(publish("1.0.0", false, false).is_err(), "already published");
        assert!(publish("1.0.5", false, false).is_err(), "older than 1.1.0");
        publish("1.2.0", false, false).unwrap();
        assert_eq!(latest(), "1.2.0");
        publish("1.0.0", false, true).unwrap();
        assert_eq!(latest(), "1.2.0", "an older version published again is not the latest");
        fs::remove_dir_all(&root).unwrap();
    }
}
