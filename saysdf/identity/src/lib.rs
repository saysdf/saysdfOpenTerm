//! saysdfTerm identity: every user-visible name, config/data path component
//! and binary name of the fork lives here, so upstream files only carry
//! one-line references (change rebrand-saysdfterm, design D2).

use std::path::{Path, PathBuf};

/// Application name; also the subdirectory name for config/data/cache/runtime dirs.
pub const APP_NAME: &str = "saysdfTerm";
/// Config file name inside the config dirs and next to the exe (Windows).
pub const CONFIG_FILE_NAME: &str = "saysdfTerm.lua";
/// Config file name directly in the home dir.
pub const HOME_CONFIG_FILE: &str = ".saysdfTerm.lua";
/// Lua module dir directly in the home dir.
pub const HOME_MODULE_DIR: &str = ".saysdfTerm";
/// Portable Lua module dir next to the exe (Windows).
pub const EXE_MODULE_DIR: &str = "saysdfTerm_modules";
/// Env var overriding the config file; exported after load.
pub const CONFIG_FILE_ENV: &str = "SAYSDFTERM_CONFIG_FILE";
/// Env var exported after load with the config file's dir.
pub const CONFIG_DIR_ENV: &str = "SAYSDFTERM_CONFIG_DIR";
/// Window class / app id / AppUserModelID.
pub const WINDOW_CLASS: &str = "com.saysdf.saysdfTerm";
/// GUI binary base name.
pub const GUI_EXE: &str = "saysdfTerm";
/// CLI binary base name.
pub const CLI_EXE: &str = "saysdfTerm-cli";
/// Mux server binary base name.
pub const MUX_EXE: &str = "saysdfTerm-mux-server";

/// File name of a binary on the current OS (adds `.exe` on Windows).
pub fn exe_file_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Config file candidates in lookup order: `(path, required)`.
///
/// Order (spec app-identity, "Config file location"): `env_file`
/// (from `SAYSDFTERM_CONFIG_FILE`, required); on Windows `exe_dir/saysdfTerm.lua`;
/// `home/.saysdfTerm.lua`; `<dir>/saysdfTerm.lua` for each of `config_dirs`.
/// `--config-file` is prepended by the caller; `WEZTERM_CONFIG_FILE` is never used.
pub fn config_file_candidates(
    home: &Path,
    config_dirs: &[PathBuf],
    exe_dir: Option<&Path>,
    env_file: Option<PathBuf>,
) -> Vec<(PathBuf, bool)> {
    candidates_for(cfg!(windows), home, config_dirs, exe_dir, env_file)
}

fn candidates_for(
    windows: bool,
    home: &Path,
    config_dirs: &[PathBuf],
    exe_dir: Option<&Path>,
    env_file: Option<PathBuf>,
) -> Vec<(PathBuf, bool)> {
    let mut paths = Vec::new();
    if let Some(path) = env_file {
        paths.push((path, true));
    }
    if windows {
        if let Some(dir) = exe_dir {
            paths.push((dir.join(CONFIG_FILE_NAME), false));
        }
    }
    paths.push((home.join(HOME_CONFIG_FILE), false));
    for dir in config_dirs {
        paths.push((dir.join(CONFIG_FILE_NAME), false));
    }
    paths
}

/// User config dir of the fork's tools: `$SAYSDFTERM_CONFIG_DIR` (exported by the GUI after the
/// config is loaded, so child processes agree with it), else `~/.config/saysdfTerm`.
pub fn user_config_dir() -> Option<PathBuf> {
    std::env::var_os(CONFIG_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config").join(APP_NAME)))
}

/// Home dir from the environment: `USERPROFILE`, then `HOME` on Windows; `HOME` elsewhere.
pub fn home_dir() -> Option<PathBuf> {
    let vars: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    vars.iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, Vec<PathBuf>, PathBuf) {
        let home = PathBuf::from("home");
        let dirs = vec![home.join(".config").join(APP_NAME), PathBuf::from("xdg").join(APP_NAME)];
        (home, dirs, PathBuf::from("exe"))
    }

    #[test]
    fn order_without_env() {
        let (home, dirs, exe) = fixture();
        let got = candidates_for(true, &home, &dirs, Some(&exe), None);
        let want = vec![
            (exe.join("saysdfTerm.lua"), false),
            (home.join(".saysdfTerm.lua"), false),
            (dirs[0].join("saysdfTerm.lua"), false),
            (dirs[1].join("saysdfTerm.lua"), false),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn env_file_first_and_required() {
        let (home, dirs, exe) = fixture();
        let env = PathBuf::from("custom.lua");
        let got = candidates_for(true, &home, &dirs, Some(&exe), Some(env.clone()));
        assert_eq!(got[0], (env, true));
        assert_eq!(got.len(), 5);
        assert!(got[1..].iter().all(|(_, required)| !required));
    }

    #[test]
    fn exe_dir_only_on_windows() {
        let (home, dirs, exe) = fixture();
        let got = candidates_for(false, &home, &dirs, Some(&exe), None);
        assert_eq!(got[0], (home.join(".saysdfTerm.lua"), false));
        assert!(got.iter().all(|(p, _)| !p.starts_with(&exe)));

        let public = config_file_candidates(&home, &dirs, Some(&exe), None);
        assert_eq!(public.iter().any(|(p, _)| p.starts_with(&exe)), cfg!(windows));
    }

    #[test]
    fn no_wezterm_names() {
        let (home, dirs, exe) = fixture();
        let got = candidates_for(true, &home, &dirs, Some(&exe), None);
        assert!(got
            .iter()
            .all(|(p, _)| !p.to_string_lossy().to_lowercase().contains("wezterm")));
    }

    #[test]
    fn exe_suffix() {
        let name = exe_file_name(GUI_EXE);
        if cfg!(windows) {
            assert_eq!(name, "saysdfTerm.exe");
        } else {
            assert_eq!(name, "saysdfTerm");
        }
    }
}
