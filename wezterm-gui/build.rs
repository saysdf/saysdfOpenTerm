fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        use anyhow::Context as _;
        use std::io::Write;
        use std::path::Path;
        let profile = std::env::var("PROFILE").unwrap();
        let repo_dir = std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.parent().map(|p| p.to_path_buf()))
            .unwrap();
        // saysdfTerminal: honor CARGO_TARGET_DIR (shared build cache for
        // agent worktrees) so conpty.dll lands next to the built exe
        let exe_output_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| repo_dir.join("target"))
            .join(profile);
        let windows_dir = repo_dir.join("assets").join("windows");

        let conhost_dir = windows_dir.join("conhost");
        for name in &["conpty.dll", "OpenConsole.exe"] {
            let dest_name = exe_output_dir.join(name);
            let src_name = conhost_dir.join(name);

            if !dest_name.exists() {
                std::fs::copy(&src_name, &dest_name)
                    .context(format!(
                        "copy {} -> {}",
                        src_name.display(),
                        dest_name.display()
                    ))
                    .unwrap();
            }
        }

        // saysdfTerminal: unsigned prebuilt ANGLE and Mesa DLLs are no
        // longer bundled; only the Microsoft-signed conpty is copied.

        // saysdfTerminal: shell integration scripts next to the exe
        // (<exe_dir>/shell-integration, change shell-integration D2).
        {
            fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
                std::fs::create_dir_all(dest)?;
                for entry in std::fs::read_dir(src)? {
                    let entry = entry?;
                    let to = dest.join(entry.file_name());
                    if entry.file_type()?.is_dir() {
                        copy_tree(&entry.path(), &to)?;
                    } else {
                        std::fs::copy(entry.path(), to)?;
                    }
                }
                Ok(())
            }
            let src = repo_dir.join("saysdf").join("shell-integration");
            let wezterm_sh = repo_dir
                .join("assets")
                .join("shell-integration")
                .join("wezterm.sh");
            println!("cargo:rerun-if-changed={}", src.display());
            println!("cargo:rerun-if-changed={}", wezterm_sh.display());
            let dest = exe_output_dir.join("shell-integration");
            copy_tree(&src, &dest)
                .and_then(|_| std::fs::copy(&wezterm_sh, dest.join("wezterm.sh")).map(|_| ()))
                .context(format!("copy shell integration -> {}", dest.display()))
                .unwrap();
        }

        // If a file named `.tag` is present, we'll take its contents for the
        // version number that we report in wezterm -h.
        let mut ci_tag = String::new();
        if let Ok(tag) = std::fs::read("../.tag") {
            if let Ok(s) = String::from_utf8(tag) {
                ci_tag = s.trim().to_string();
                println!("cargo:rerun-if-changed=../.tag");
            }
        }
        let version = if ci_tag.is_empty() {
            let mut cmd = std::process::Command::new("git");
            cmd.args(&[
                "-c",
                "core.abbrev=8",
                "show",
                "-s",
                "--format=%cd-%h",
                "--date=format:%Y%m%d-%H%M%S",
            ]);
            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    String::from_utf8_lossy(&output.stdout).trim().to_owned()
                } else {
                    "UNKNOWN".to_owned()
                }
            } else {
                "UNKNOWN".to_owned()
            }
        } else {
            ci_tag
        };

        let rcfile_name = Path::new(&std::env::var_os("OUT_DIR").unwrap()).join("resource.rc");
        let mut rcfile = std::fs::File::create(&rcfile_name).unwrap();
        println!("cargo:rerun-if-changed=../assets/windows/terminal.ico");
        write!(
            rcfile,
            r#"
#include <winres.h>
// This ID is coupled with code in window/src/os/windows/window.rs
#define IDI_ICON 0x101
1 RT_MANIFEST "{win}\\manifest.manifest"
IDI_ICON ICON "{win}\\terminal.ico"
VS_VERSION_INFO VERSIONINFO
FILEVERSION     1,0,0,0
PRODUCTVERSION  1,0,0,0
FILEFLAGSMASK   VS_FFI_FILEFLAGSMASK
FILEFLAGS       0
FILEOS          VOS__WINDOWS32
FILETYPE        VFT_APP
FILESUBTYPE     VFT2_UNKNOWN
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904E4"
        BEGIN
            VALUE "CompanyName",      "Wez Furlong\0"
            VALUE "FileDescription",  "WezTerm - Wez's Terminal Emulator\0"
            VALUE "FileVersion",      "{version}\0"
            VALUE "LegalCopyright",   "Wez Furlong, MIT licensed\0"
            VALUE "InternalName",     "\0"
            VALUE "OriginalFilename", "\0"
            VALUE "ProductName",      "WezTerm\0"
            VALUE "ProductVersion",   "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1252
    END
END
"#,
            win = windows_dir.display().to_string().replace("\\", "\\\\"),
            version = version,
        )
        .unwrap();
        drop(rcfile);

        // Obtain MSVC environment so that the rc compiler can find the right headers.
        // https://github.com/nabijaczleweli/rust-embed-resource/issues/11#issuecomment-603655972
        let target = std::env::var("TARGET").unwrap();
        if let Some(tool) = cc::windows_registry::find_tool(target.as_str(), "cl.exe") {
            for (key, value) in tool.env() {
                std::env::set_var(key, value);
            }
        }
        embed_resource::compile(rcfile_name);
    }

    #[cfg(target_os = "macos")]
    {
        use anyhow::Context as _;
        let profile = std::env::var("PROFILE").unwrap();
        let repo_dir = std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.parent().map(|p| p.to_path_buf()))
            .unwrap();

        // We need to copy the plist to avoid the UNUserNotificationCenter asserting
        // due to not finding the application bundle
        let src_plist = repo_dir
            .join("assets")
            .join("macos")
            .join("WezTerm.app")
            .join("Contents")
            .join("Info.plist");
        let build_target_dir = std::env::var("CARGO_TARGET_DIR")
            .and_then(|s| Ok(std::path::PathBuf::from(s)))
            .unwrap_or(repo_dir.join("target").join(profile));
        let dest_plist = build_target_dir.join("Info.plist");
        println!("cargo:rerun-if-changed=assets/macos/WezTerm.app/Contents/Info.plist");

        std::fs::copy(&src_plist, &dest_plist)
            .context(format!(
                "copy {} -> {}",
                src_plist.display(),
                dest_plist.display()
            ))
            .unwrap();
    }
}
