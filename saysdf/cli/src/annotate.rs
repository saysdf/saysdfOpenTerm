//! Annotation write session: the only place where a viewer writes to a user file (design D4).
//!
//! [`Session::open`] takes a snapshot of the file (raw bytes, length, content hash). It is
//! refused for large, binary and non-UTF-8 files and for anything that is not a regular file.
//! [`Session::apply`] re-reads the file right before writing and refuses when the bytes on disk
//! differ from the snapshot; otherwise it builds the new text with `saysdf_render::critic` and
//! writes it with [`crate::fsutil::write_atomic`] (permissions kept), through a symlink to its
//! target. The session never creates files. The check-then-rename race is accepted (design Q6).

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

use saysdf_render::critic::{self, Edit, Mark};
use saysdf_render::source;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-annotate");

/// One write request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Add a new annotation ([`critic::annotate`]).
    Annotate(Edit),
    /// Remove a mark, keeping the text it covered ([`critic::remove`]).
    Remove(Mark),
    /// Replace the typed part of a mark ([`critic::amend`]).
    Amend(Mark, String),
}

/// Why annotating is unavailable or a write did not happen. The file is never modified then.
#[derive(Debug)]
pub enum Refusal {
    /// The file is larger than the viewer's `max_size` (Large view).
    TooLarge,
    /// The file looks binary (hex view).
    Binary,
    /// The bytes are not valid UTF-8; the decoded lines would not round-trip.
    NotUtf8,
    /// The path is not a regular file (a folder, a device, a dangling symlink target...).
    NotAFile,
    /// The file (or the symlink target) is read-only.
    ReadOnly,
    /// The file no longer exists.
    Gone,
    /// The bytes on disk differ from the snapshot; the caller reloads and tells the user.
    Changed,
    /// The markup could not be built.
    Markup(critic::Refusal),
    /// Reading the file failed.
    Read(io::Error),
    /// Writing failed; the file was left as it was.
    Write(io::Error),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Refusal::TooLarge => I18N.t("large file: annotations are not available").into_owned(),
            Refusal::Binary => I18N.t("binary file: annotations are not available").into_owned(),
            Refusal::NotUtf8 => I18N.t("not a UTF-8 file: annotations are not available").into_owned(),
            Refusal::NotAFile => I18N.t("not a regular file: annotations are not available").into_owned(),
            Refusal::ReadOnly => I18N.t("the file is read-only; annotation not saved").into_owned(),
            Refusal::Gone => I18N.t("the file no longer exists; annotation not saved").into_owned(),
            Refusal::Changed => I18N.t("file changed on disk; reloaded, annotation not saved").into_owned(),
            Refusal::Markup(r) => r.to_string(),
            Refusal::Read(e) => I18N.f("cannot read the file: {error}", &[("error", &e.to_string())]),
            Refusal::Write(e) => {
                I18N.f("cannot write the file, it was left as it was: {error}", &[("error", &e.to_string())])
            }
        };
        f.write_str(&s)
    }
}

impl std::error::Error for Refusal {}

impl From<critic::Refusal> for Refusal {
    fn from(r: critic::Refusal) -> Self {
        Refusal::Markup(r)
    }
}

/// What the file looked like when it was loaded (or last written by the session).
#[derive(Debug, Clone)]
struct Snapshot {
    bytes: Vec<u8>,
    len: u64,
    hash: u64,
}

impl Snapshot {
    fn new(bytes: Vec<u8>) -> Self {
        Self { len: bytes.len() as u64, hash: hash(&bytes), bytes }
    }

    /// Whether `bytes` read from disk are the snapshot. The full comparison decides; length and
    /// hash only make the common mismatch cheap.
    fn same(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.len && hash(bytes) == self.hash && bytes == self.bytes.as_slice()
    }
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn read_err(e: io::Error) -> Refusal {
    if e.kind() == io::ErrorKind::NotFound {
        Refusal::Gone
    } else {
        Refusal::Read(e)
    }
}

/// Metadata of the resolved path; refuses anything that is not an existing regular file.
fn regular(path: &Path) -> Result<fs::Metadata, Refusal> {
    let m = fs::metadata(path).map_err(read_err)?;
    if m.is_file() {
        Ok(m)
    } else {
        Err(Refusal::NotAFile)
    }
}

/// Annotation session over one file.
#[derive(Debug)]
pub struct Session {
    path: PathBuf,
    snapshot: Snapshot,
}

impl Session {
    /// Loads the snapshot. Refused for files over `max_size`, binary or non-UTF-8 content (a BOM
    /// is allowed) and anything that is not a regular file (symlinks are followed).
    pub fn open(path: &Path, max_size: u64) -> Result<Self, Refusal> {
        let m = regular(path)?;
        if m.len() > max_size {
            return Err(Refusal::TooLarge);
        }
        let bytes = fs::read(path).map_err(read_err)?;
        if bytes.len() as u64 > max_size {
            return Err(Refusal::TooLarge);
        }
        if source::is_binary(&bytes) {
            return Err(Refusal::Binary);
        }
        if std::str::from_utf8(&bytes).is_err() {
            return Err(Refusal::NotUtf8);
        }
        Ok(Self { path: path.to_path_buf(), snapshot: Snapshot::new(bytes) })
    }

    /// The snapshot text (with the BOM, if any); positions for [`Op`] are byte offsets into it.
    pub fn text(&self) -> &str {
        // Checked in `open` and for every write.
        std::str::from_utf8(&self.snapshot.bytes).unwrap_or_default()
    }

    /// Applies `op` to the file on disk and returns the new text. Nothing is written on any refusal.
    pub fn apply(&mut self, op: &Op) -> Result<&str, Refusal> {
        self.apply_with(op, |p, data| crate::fsutil::write_atomic(p, data, false))
    }

    fn apply_with(
        &mut self,
        op: &Op,
        write: impl FnOnce(&Path, &[u8]) -> io::Result<()>,
    ) -> Result<&str, Refusal> {
        // D4 step 4: write through a symlink to its target instead of replacing the link.
        let target = fs::canonicalize(&self.path).map_err(read_err)?;
        let m = regular(&target)?;
        // D4 step 2: compare with the disk right before writing.
        let disk = fs::read(&target).map_err(read_err)?;
        if !self.snapshot.same(&disk) {
            return Err(Refusal::Changed);
        }
        if m.permissions().readonly() {
            return Err(Refusal::ReadOnly);
        }
        let text = self.text();
        let new = match op {
            Op::Annotate(edit) => critic::annotate(text, edit)?,
            Op::Remove(mark) => critic::remove(text, mark)?,
            Op::Amend(mark, typed) => critic::amend(text, mark, typed)?,
        };
        write(&target, new.as_bytes()).map_err(Refusal::Write)?;
        self.snapshot = Snapshot::new(new.into_bytes());
        Ok(self.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saysdf_render::critic::Action;

    const MAX: u64 = 1 << 20;

    fn file(d: &tempfile::TempDir, bytes: &[u8]) -> PathBuf {
        let p = d.path().join("a.md");
        fs::write(&p, bytes).unwrap();
        p
    }

    fn at(text: &str, needle: &str) -> usize {
        text.find(needle).unwrap()
    }

    fn insert(text: &str, after: &str, typed: &str) -> Op {
        let i = at(text, after) + after.len();
        Op::Annotate(Edit { start: i, end: i, action: Action::Insert(typed.into()) })
    }

    #[test]
    fn annotate_cyrillic_multiline() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, "один\nдва\n".as_bytes()); // i18n-allow-cyrillic
        let mut s = Session::open(&p, MAX).unwrap();
        let t = s.text().to_string();
        let start = at(&t, "два"); // i18n-allow-cyrillic
        let op = Op::Annotate(Edit { start, end: start + "два".len(), action: Action::Substitute("три\nчетыре".into()) }); // i18n-allow-cyrillic
        s.apply(&op).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "один\n{~~два~>три\nчетыре~~}\n"); // i18n-allow-cyrillic
    }

    #[test]
    fn annotate_keeps_bom_and_crlf() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"\xEF\xBB\xBFone\r\ntwo\r\n");
        let mut s = Session::open(&p, MAX).unwrap();
        let op = insert(s.text(), "one", "a\nb");
        s.apply(&op).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"\xEF\xBB\xBFone{++a\r\nb++}\r\ntwo\r\n");
    }

    #[test]
    fn second_op_uses_updated_snapshot_then_remove_and_amend() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"one two
");
        let mut s = Session::open(&p, MAX).unwrap();
        let op = insert(s.text(), "one", "X");
        s.apply(&op).unwrap();
        let op = insert(s.text(), "two", "Y");
        s.apply(&op).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one{++X++} two{++Y++}
");

        let mark = critic::parse(s.text())[1].clone();
        s.apply(&Op::Amend(mark, "Z".into())).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one{++X++} two{++Z++}
");

        let mark = critic::parse(s.text())[0].clone();
        s.apply(&Op::Remove(mark)).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one two{++Z++}
");
        assert_eq!(s.text(), "one two{++Z++}
");
    }

    #[test]
    fn changed_on_disk_is_refused_and_untouched() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"one\n");
        let mut s = Session::open(&p, MAX).unwrap();
        let op = insert(s.text(), "one", "X");
        // Same length, different bytes: the full comparison catches it.
        fs::write(&p, b"two\n").unwrap();
        assert!(matches!(s.apply(&op), Err(Refusal::Changed)));
        assert_eq!(fs::read(&p).unwrap(), b"two\n");
        // The snapshot stays the loaded one; a new session would be needed.
        assert_eq!(s.text(), "one\n");
    }

    #[test]
    fn deleted_file_is_refused_and_not_created() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"one\n");
        let mut s = Session::open(&p, MAX).unwrap();
        let op = insert(s.text(), "one", "X");
        fs::remove_file(&p).unwrap();
        assert!(matches!(s.apply(&op), Err(Refusal::Gone)));
        assert!(!p.exists());
        assert!(matches!(Session::open(&p, MAX), Err(Refusal::Gone)));
    }

    #[test]
    fn unavailable_files() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"ok \xFF\xFE bad\n");
        assert!(matches!(Session::open(&p, MAX), Err(Refusal::NotUtf8)));
        fs::write(&p, b"a\0b").unwrap();
        assert!(matches!(Session::open(&p, MAX), Err(Refusal::Binary)));
        fs::write(&p, b"0123456789").unwrap();
        assert!(matches!(Session::open(&p, 5), Err(Refusal::TooLarge)));
        assert!(matches!(Session::open(d.path(), MAX), Err(Refusal::NotAFile)));
    }

    #[test]
    fn markup_refusal_does_not_write() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"one\n");
        let mut s = Session::open(&p, MAX).unwrap();
        let op = insert(s.text(), "one", "{++");
        assert!(matches!(s.apply(&op), Err(Refusal::Markup(_))));
        assert_eq!(fs::read(&p).unwrap(), b"one\n");
    }

    #[test]
    fn read_only_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"one\n");
        let mut s = Session::open(&p, MAX).unwrap();
        let orig = fs::metadata(&p).unwrap().permissions();
        let mut perm = orig.clone();
        perm.set_readonly(true);
        fs::set_permissions(&p, perm).unwrap();
        let op = insert(s.text(), "one", "X");
        let res = s.apply(&op);
        fs::set_permissions(&p, orig).unwrap();
        assert!(matches!(res, Err(Refusal::ReadOnly)));
        assert_eq!(fs::read(&p).unwrap(), b"one\n");
    }

    #[test]
    fn write_failure_leaves_file_and_snapshot() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, b"one\n");
        let mut s = Session::open(&p, MAX).unwrap();
        let op = insert(s.text(), "one", "X");
        let res = s.apply_with(&op, |_, _| Err(io::Error::other("disk full")));
        assert!(matches!(res, Err(Refusal::Write(_))));
        assert_eq!(fs::read(&p).unwrap(), b"one\n");
        // The snapshot still matches the disk, so a retry works.
        s.apply(&op).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one{++X++}\n");
    }

    #[test]
    fn messages_follow_language() {
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(Refusal::Changed.to_string(), "file changed on disk; reloaded, annotation not saved");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(Refusal::Changed.to_string(), "файл изменён на диске; перечитан, аннотация не сохранена"); // i18n-allow-cyrillic
    }

    #[cfg(unix)]
    #[test]
    fn symlink_writes_through_to_target() {
        let d = tempfile::tempdir().unwrap();
        let target = file(&d, b"one\n");
        let link = d.path().join("link.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut s = Session::open(&link, MAX).unwrap();
        let op = insert(s.text(), "one", "X");
        s.apply(&op).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"one{++X++}\n");
    }

    #[cfg(windows)]
    #[test]
    fn symlink_writes_through_to_target() {
        let d = tempfile::tempdir().unwrap();
        let target = file(&d, b"one\n");
        let link = d.path().join("link.md");
        // Creating symlinks needs a privilege or developer mode; skip without it.
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            eprintln!("symlink_writes_through_to_target: no symlink privilege, skipped");
            return;
        }
        let mut s = Session::open(&link, MAX).unwrap();
        let op = insert(s.text(), "one", "X");
        s.apply(&op).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"one{++X++}\n");
    }
}
