//! Embeds the repository's `boards/*.toml` (what `cargo iotkit new` knows about each board), so an
//! installed CLI carries them.

use std::{env, fs, path::Path};

fn main() {
    let dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../boards");
    println!("cargo::rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = fs::read_dir(&dir)
        .expect("the repository's boards/ folder")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    let mut out = String::from("/// `(id, TOML)` of each board file.\npub const FILES: &[(&str, &str)] = &[\n");
    for f in &files {
        let id = f.file_stem().unwrap().to_str().unwrap();
        let path = fs::canonicalize(f).unwrap();
        out += &format!("    ({id:?}, include_str!({:?})),\n", path.display().to_string());
    }
    out += "];\n";
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("boards.rs"), out).unwrap();
}
