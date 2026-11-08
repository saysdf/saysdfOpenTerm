//! Shared file helpers. One atomic write for ssh config, connections, merge results, the monitor
//! status file and annotations (rule 2: no second copy).

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Write via a temporary file in the same folder plus rename over the target.
///
/// The data is flushed and synced before the rename. On any failure the target is left as it was
/// and the temporary file is removed. The permissions of an existing target are preserved; for a
/// new file `private` means 0600 on Unix (on Windows the inherited ACLs of the folder apply).
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-saysdf-{}", std::process::id()));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        if let Ok(m) = fs::metadata(path) {
            fs::set_permissions(&tmp, m.permissions())?;
        } else if private {
            set_private(&tmp)?;
        }
        // Windows: `fs::rename` replaces an existing file (MoveFileEx with REPLACE_EXISTING).
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

#[cfg(unix)]
fn set_private(p: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o600))
}

/// Windows: the inherited ACLs of the user profile.
#[cfg(not(unix))]
fn set_private(_p: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn writes_new_file_without_leftovers() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.txt");
        write_atomic(&p, b"one", false).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one");
        assert_eq!(names(d.path()), vec!["a.txt"]);
    }

    #[test]
    fn replaces_existing_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.txt");
        fs::write(&p, "old").unwrap();
        write_atomic(&p, b"new", true).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
        assert_eq!(names(d.path()), vec!["a.txt"]);
    }

    #[test]
    fn failure_keeps_target_and_removes_temp() {
        let d = tempfile::tempdir().unwrap();
        // The target is a non-empty directory: rename over it fails after the temp file was written.
        let p = d.path().join("t");
        fs::create_dir(&p).unwrap();
        fs::write(p.join("inner"), "x").unwrap();
        assert!(write_atomic(&p, b"data", false).is_err());
        assert!(p.join("inner").exists());
        assert_eq!(names(d.path()), vec!["t"]);
    }

    #[test]
    fn missing_folder_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("no").join("a.txt");
        assert!(write_atomic(&p, b"x", false).is_err());
        assert!(!p.exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_and_preserved_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        write_atomic(&p, b"x", true).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&p, b"y", true).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
    }
}
