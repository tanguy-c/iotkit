//! Embeds the repository's `boards/*.toml` and `components/*.toml` (what `cargo iotkit new` knows
//! about each board and part), so an installed CLI carries them.

use std::{env, fs, path::Path};

fn main() {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    for folder in ["boards", "components"] {
        let dir = root.join(folder);
        println!("cargo::rerun-if-changed={}", dir.display());
        let mut files: Vec<_> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("the repository's {folder}/ folder: {e}"))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        let mut out = format!("/// `(id, TOML)` of each file in `{folder}/`.\npub const FILES: &[(&str, &str)] = &[\n");
        for f in &files {
            let id = f.file_stem().unwrap().to_str().unwrap();
            let path = fs::canonicalize(f).unwrap();
            out += &format!("    ({id:?}, include_str!({:?})),\n", path.display().to_string());
        }
        out += "];\n";
        fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join(format!("{folder}.rs")), out).unwrap();
    }
}
