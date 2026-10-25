//! The template's files, built into the CLI (it works away from the checkout), rendered with
//! Liquid as `cargo generate` does.

use anyhow::{Context, Result};

/// The files `cargo generate` copies (`template/cargo-generate.toml` ignores the rest).
pub const FILES: &[(&str, &str)] = &[
    ("build.rs", include_str!("../../../template/build.rs")),
    ("Cargo.toml", include_str!("../../../template/Cargo.toml.liquid")),
    (".cargo/config.toml", include_str!("../../../template/.cargo/config.toml")),
    (".cargo/secrets.toml.example", include_str!("../../../template/.cargo/secrets.toml.example")),
    (".clippy.toml", include_str!("../../../template/.clippy.toml")),
    (".gitignore", include_str!("../../../template/.gitignore")),
    ("partitions.csv", include_str!("../../../template/partitions.csv")),
    ("partitions-4mb.csv", include_str!("../../../template/partitions-4mb.csv")),
    ("rustfmt.toml", include_str!("../../../template/rustfmt.toml")),
    ("rust-toolchain.toml", include_str!("../../../template/rust-toolchain.toml")),
    ("src/main.rs", include_str!("../../../template/src/main.rs")),
    (".vscode/extensions.json", include_str!("../../../template/.vscode/extensions.json")),
    (".vscode/launch.json", include_str!("../../../template/.vscode/launch.json")),
    (".vscode/settings.json", include_str!("../../../template/.vscode/settings.json")),
    (".vscode/tasks.json", include_str!("../../../template/.vscode/tasks.json")),
];

/// Renders a template file with `cargo generate`'s variables.
pub fn render(path: &str, source: &str, package: &str, chip: &str, iotkit_path: &str) -> Result<String> {
    let parser = liquid::ParserBuilder::with_stdlib().build()?;
    let template = parser.parse(source).with_context(|| format!("template/{path}"))?;
    let globals = liquid::object!({
        "project-name": package,
        "chip": chip,
        "iotkit_path": iotkit_path,
    });
    template.render(&globals).with_context(|| format!("template/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chip_renders() {
        for chip in ["esp32c6", "esp32c3", "esp32c5", "esp32s3", "esp32", "esp32h2"] {
            for (path, source) in FILES {
                let out = render(path, source, "my-device", chip, "/opt/iotkit").unwrap();
                assert!(!out.contains("{%") && !out.contains("{{project"), "{chip} {path}");
            }
        }
        let cargo = render("Cargo.toml", FILES[1].1, "my-device", "esp32c3", "/opt/iotkit").unwrap();
        assert!(cargo.contains("name         = \"my-device\"") && cargo.contains("flash-size = \"4mb\""));
        assert!(cargo.contains("iotkit = { path = \"/opt/iotkit/crates/iotkit\""));
        // No checkout: the git repository.
        let cargo = render("Cargo.toml", FILES[1].1, "my-device", "esp32c6", "").unwrap();
        let git = format!("git = \"{}\"", crate::new::IOTKIT_GIT);
        assert!(cargo.matches(&git).count() == 2 && !cargo.contains("path ="), "{cargo}");
    }
}
