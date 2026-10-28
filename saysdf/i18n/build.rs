//! Generates `embedded.rs`: `EMBEDDED` = `(language, domain, source)` for every
//! `saysdf/locales/<lang>/<domain>.toml`, embedded with `include_str!` (change `i18n`, design D3).
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let locales = manifest.parent().unwrap().join("locales");
    println!("cargo:rerun-if-changed={}", locales.display());

    let mut files: Vec<(String, String, PathBuf)> = Vec::new();
    if locales.is_dir() {
        for lang in std::fs::read_dir(&locales).expect("read saysdf/locales") {
            let lang = lang.expect("dir entry").path();
            if !lang.is_dir() {
                continue;
            }
            println!("cargo:rerun-if-changed={}", lang.display());
            let lang_name = lang.file_name().unwrap().to_str().expect("utf-8 lang").to_string();
            for file in std::fs::read_dir(&lang).expect("read locale dir") {
                let path = file.expect("dir entry").path();
                if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                println!("cargo:rerun-if-changed={}", path.display());
                let domain = path.file_stem().unwrap().to_str().expect("utf-8 domain").to_string();
                files.push((lang_name.clone(), domain, path));
            }
        }
    }
    files.sort();

    let mut out = String::from("pub static EMBEDDED: &[(&str, &str, &str)] = &[\n");
    for (lang, domain, path) in &files {
        writeln!(
            out,
            "    ({lang:?}, {domain:?}, include_str!({:?})),",
            path.to_str().expect("utf-8 path")
        )
        .unwrap();
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("embedded.rs");
    std::fs::write(dest, out).expect("write embedded.rs");
}
