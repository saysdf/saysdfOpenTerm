//! Generates `bundled.rs`: `MODULES` (module name -> source) and `DEFAULT_CONFIG` via `include_str!`.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let lua_dir = manifest.parent().unwrap().join("lua");
    let mod_dir = lua_dir.join("saysdf");
    let default_config = lua_dir.join("default_config.lua");

    println!("cargo:rerun-if-changed={}", mod_dir.display());
    println!("cargo:rerun-if-changed={}", default_config.display());

    let mut modules: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&mod_dir).expect("read saysdf/lua/saysdf") {
        let path = entry.expect("dir entry").path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("lua") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("utf-8 module name");
        let name = if stem == "init" {
            "saysdf".to_string()
        } else {
            format!("saysdf.{stem}")
        };
        println!("cargo:rerun-if-changed={}", path.display());
        modules.push((name, path));
    }
    modules.sort();

    let mut out = String::from("pub static MODULES: &[(&str, &str)] = &[\n");
    for (name, path) in &modules {
        writeln!(out, "    ({name:?}, include_str!({:?})),", path_str(path)).unwrap();
    }
    out.push_str("];\n");
    writeln!(
        out,
        "pub static DEFAULT_CONFIG: &str = include_str!({:?});",
        path_str(&default_config)
    )
    .unwrap();

    let dest = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("bundled.rs");
    std::fs::write(dest, out).expect("write bundled.rs");
}

fn path_str(p: &Path) -> &str {
    p.to_str().expect("utf-8 path")
}
