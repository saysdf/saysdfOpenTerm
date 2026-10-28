//! File browser operations (file-browser D4–D5): create/rename/delete without overwriting,
//! symlinks are treated as links (the target is not touched), a non-empty folder is deleted only
//! after its name is typed; the "path lies under the root" check before an action is sent to the GUI.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-files");

/// Name of a new/renamed item: a single name without separators, not `.`/`..`.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.trim().is_empty() {
        return Err(I18N.t("empty name").into_owned());
    }
    if name == "." || name == ".." {
        return Err(I18N.t("invalid name").into_owned());
    }
    if name.chars().any(|c| c == '/' || c == '\\' || c.is_control()) {
        return Err(I18N.t("the name must not contain / \\ or control characters").into_owned());
    }
    #[cfg(windows)]
    if name.chars().any(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(I18N.t("invalid character in the name").into_owned());
    }
    Ok(())
}

fn exists_entry(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// Create a file or folder `name` in `dir`; an existing one is never overwritten.
pub fn create(dir: &Path, name: &str, is_dir: bool) -> Result<PathBuf, String> {
    check_name(name)?;
    let p = dir.join(name);
    let res = if is_dir {
        fs::create_dir(&p)
    } else {
        fs::OpenOptions::new().write(true).create_new(true).open(&p).map(|_| ())
    };
    match res {
        Ok(()) => Ok(p),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(I18N.f("“{name}” already exists", &[("name", &name)])),
        Err(e) => Err(e.to_string()),
    }
}

/// Rename within the folder; an existing name is an error. A case-only change
/// (Windows/macOS) goes through a temporary name.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    check_name(new_name)?;
    let parent = path.parent().ok_or_else(|| I18N.t("no parent folder").into_owned())?;
    let old_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if old_name == new_name {
        return Ok(path.to_path_buf());
    }
    let target = parent.join(new_name);
    let case_only = old_name.to_lowercase() == new_name.to_lowercase();
    if case_only {
        let tmp = parent.join(format!(".{old_name}.saysdf-rename"));
        if exists_entry(&tmp) {
            return Err(I18N.t("the temporary name is taken").into_owned());
        }
        fs::rename(path, &tmp).map_err(|e| e.to_string())?;
        if let Err(e) = fs::rename(&tmp, &target) {
            let _ = fs::rename(&tmp, path);
            return Err(e.to_string());
        }
        return Ok(target);
    }
    if exists_entry(&target) {
        return Err(I18N.f("“{name}” already exists", &[("name", &new_name)]));
    }
    fs::rename(path, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// What exactly is being deleted — the confirmation depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteKind {
    /// A link: the link itself is deleted.
    Link,
    File,
    EmptyDir,
    /// A non-empty folder: its name has to be typed.
    NonEmptyDir,
}

pub fn delete_kind(path: &Path) -> io::Result<DeleteKind> {
    let m = fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() {
        return Ok(DeleteKind::Link);
    }
    if m.is_dir() {
        let empty = fs::read_dir(path)?.next().is_none();
        return Ok(if empty { DeleteKind::EmptyDir } else { DeleteKind::NonEmptyDir });
    }
    Ok(DeleteKind::File)
}

/// Irreversible deletion after confirmation. For a non-empty folder `typed` must match its
/// name exactly, otherwise nothing is deleted.
pub fn delete(path: &Path, kind: DeleteKind, typed: Option<&str>) -> Result<(), String> {
    // The kind may have changed between the confirmation and the deletion — check again.
    let now = delete_kind(path).map_err(|e| e.to_string())?;
    if now != kind {
        return Err(if now == DeleteKind::NonEmptyDir {
            I18N.t("the folder is no longer empty — repeat the deletion").into_owned()
        } else {
            I18N.t("the item has changed — repeat the deletion").into_owned()
        });
    }
    match now {
        DeleteKind::Link => {
            // remove_file removes a link to a file (and any link on unix); a link to a folder and a
            // junction on Windows — remove_dir of the link itself. The target is not touched.
            fs::remove_file(path).or_else(|_| fs::remove_dir(path)).map_err(|e| e.to_string())
        }
        DeleteKind::File => fs::remove_file(path).map_err(|e| e.to_string()),
        DeleteKind::EmptyDir => fs::remove_dir(path).map_err(|e| e.to_string()),
        DeleteKind::NonEmptyDir => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if typed != Some(name.as_str()) {
                return Err(I18N.t("the name does not match — deletion cancelled").into_owned());
            }
            // remove_dir_all does not follow links inside the folder
            fs::remove_dir_all(path).map_err(|e| e.to_string())
        }
    }
}

/// Canonical path without the `\\?\` prefix (Windows), so that comparison does not depend on the form.
pub fn canonical(p: &Path) -> io::Result<PathBuf> {
    Ok(strip_verbatim(fs::canonicalize(p)?))
}

pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if rest.as_bytes().get(1) == Some(&b':') {
            return PathBuf::from(rest);
        }
    }
    p
}

/// Path for a GUI action: it exists and, once canonicalized (links resolved), lies under one of
/// the roots. Returns the path without resolving the last link — as the user sees it.
pub fn under_root(roots: &[PathBuf], path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return Err(I18N.t("the path must be absolute and without ..").into_owned());
    }
    let real = canonical(path).map_err(|e| format!("{}: {e}", path.display()))?;
    for r in roots {
        if let Ok(root) = canonical(r) {
            if real.starts_with(&root) {
                return Ok(path.to_path_buf());
            }
        }
    }
    Err(I18N.t("outside the browser root").into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let f = create(d.path(), "a.txt", false).unwrap();
        fs::write(&f, "data").unwrap();
        assert!(create(d.path(), "a.txt", false).unwrap_err().contains("already exists"));
        assert!(create(d.path(), "a.txt", true).is_err());
        assert_eq!(fs::read_to_string(&f).unwrap(), "data");
        assert!(create(d.path(), "../x", false).is_err());
        assert!(create(d.path(), "..", true).is_err());
        assert!(create(d.path(), "sub", true).unwrap().is_dir());
    }

    #[test]
    fn rename_onto_existing_fails() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a"), "A").unwrap();
        fs::write(d.path().join("b"), "B").unwrap();
        assert!(rename(&d.path().join("a"), "b").unwrap_err().contains("already exists"));
        assert_eq!(fs::read_to_string(d.path().join("b")).unwrap(), "B");
        let c = rename(&d.path().join("a"), "c").unwrap();
        assert_eq!(fs::read_to_string(c).unwrap(), "A");
        // case only
        let up = rename(&d.path().join("c"), "C").unwrap();
        assert!(up.exists());
        assert!(fs::read_dir(d.path()).unwrap().any(|e| e.unwrap().file_name() == "C"));
    }

    #[test]
    fn delete_nonempty_dir_requires_exact_name() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path().join("build");
        fs::create_dir_all(b.join("x")).unwrap();
        fs::write(b.join("x/f"), "").unwrap();
        let k = delete_kind(&b).unwrap();
        assert_eq!(k, DeleteKind::NonEmptyDir);
        assert!(delete(&b, k, None).is_err());
        assert!(delete(&b, k, Some("Build")).is_err());
        assert!(delete(&b, k, Some("buil")).is_err());
        assert!(b.exists());
        delete(&b, k, Some("build")).unwrap();
        assert!(!b.exists());
        // an empty folder and a file need no typed name
        let e = d.path().join("empty");
        fs::create_dir(&e).unwrap();
        assert_eq!(delete_kind(&e).unwrap(), DeleteKind::EmptyDir);
        delete(&e, DeleteKind::EmptyDir, None).unwrap();
        let f = d.path().join("f");
        fs::write(&f, "").unwrap();
        delete(&f, DeleteKind::File, None).unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn delete_rechecks_kind() {
        let d = tempfile::tempdir().unwrap();
        let e = d.path().join("e");
        fs::create_dir(&e).unwrap();
        fs::write(e.join("late"), "").unwrap(); // appeared after the "empty" confirmation
        assert!(delete(&e, DeleteKind::EmptyDir, None).is_err());
        assert!(e.join("late").exists());
    }

    /// Link to a folder (on Windows — if the OS allows creating links).
    fn symlink_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        return std::os::windows::fs::symlink_dir(target, link).is_ok();
    }

    #[test]
    fn delete_link_keeps_target() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("target");
        fs::create_dir(&t).unwrap();
        fs::write(t.join("keep"), "").unwrap();
        let l = d.path().join("link");
        if !symlink_dir(&t, &l) {
            return; // no permission to create links (Windows without Developer Mode)
        }
        assert_eq!(delete_kind(&l).unwrap(), DeleteKind::Link);
        delete(&l, DeleteKind::Link, None).unwrap();
        assert!(!l.exists() && fs::symlink_metadata(&l).is_err());
        assert!(t.join("keep").exists());
    }

    #[test]
    fn validator_root_dotdot_symlink() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let out = d.path().join("outside");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir(&out).unwrap();
        fs::write(root.join("sub/f.txt"), "").unwrap();
        fs::write(out.join("secret"), "").unwrap();
        let roots = vec![root.clone()];
        assert!(under_root(&roots, &root.join("sub/f.txt")).is_ok());
        assert!(under_root(&roots, &root).is_ok());
        assert!(under_root(&roots, &out.join("secret")).unwrap_err().contains("outside the browser root"));
        assert!(under_root(&roots, &root.join("..").join("outside").join("secret")).is_err());
        assert!(under_root(&roots, Path::new("sub/f.txt")).is_err());
        assert!(under_root(&roots, &root.join("nope")).is_err());
        // a name prefix is not nesting: root2 is not under root
        let root2 = d.path().join("root2");
        fs::create_dir(&root2).unwrap();
        assert!(under_root(&roots, &root2).is_err());
        let l = root.join("escape");
        if symlink_dir(&out, &l) {
            assert!(under_root(&roots, &l.join("secret")).is_err());
        }
    }

    #[test]
    fn refusals_follow_language() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a"), "").unwrap();
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(create(d.path(), "a", false).unwrap_err(), "“a” already exists");
            assert_eq!(check_name("").unwrap_err(), "empty name");
            let b = d.path().join("build");
            fs::create_dir_all(b.join("x")).unwrap();
            assert_eq!(
                delete(&b, DeleteKind::NonEmptyDir, Some("nope")).unwrap_err(),
                "the name does not match — deletion cancelled"
            );
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(create(d.path(), "a", false).unwrap_err(), "«a» уже существует"); // i18n-allow-cyrillic
        assert_eq!(check_name("").unwrap_err(), "пустое имя"); // i18n-allow-cyrillic
        let b = d.path().join("build");
        assert_eq!(
            delete(&b, DeleteKind::NonEmptyDir, Some("nope")).unwrap_err(),
            "имя не совпало — удаление отменено" // i18n-allow-cyrillic
        );
        assert!(b.exists());
    }

    #[test]
    fn verbatim_prefix() {
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\C:\x")), PathBuf::from(r"C:\x"));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\UNC\srv\s")), PathBuf::from(r"\\srv\s"));
        assert_eq!(strip_verbatim(PathBuf::from("/a/b")), PathBuf::from("/a/b"));
    }
}
