//! Watching a file (design D5): a watch on the parent folder (editors save through a rename),
//! filtered by name, debounced; if the watch cannot be set up, mtime is polled every 2 s.

use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant, SystemTime};

pub const DEBOUNCE: Duration = Duration::from_millis(150);
pub const POLL_EVERY: Duration = Duration::from_secs(2);

pub struct FileWatch {
    path: PathBuf,
    // Keeps the watcher alive; None — polling mode
    _watcher: Option<notify::RecommendedWatcher>,
    rx: Option<Receiver<()>>,
    pending: Option<Instant>,
    last_poll: Instant,
    mtime: Option<SystemTime>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl FileWatch {
    pub fn new(path: &Path) -> Self {
        let path = path.to_path_buf();
        let (watcher, rx) = match Self::start(&path) {
            Ok((w, rx)) => (Some(w), Some(rx)),
            Err(_) => (None, None),
        };
        Self {
            mtime: mtime(&path),
            path,
            _watcher: watcher,
            rx,
            pending: None,
            last_poll: Instant::now(),
        }
    }

    fn start(path: &Path) -> notify::Result<(notify::RecommendedWatcher, Receiver<()>)> {
        let dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let name = path.file_name().map(|n| n.to_os_string());
        let (tx, rx) = channel();
        let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if ev.paths.iter().any(|p| p.file_name().map(|n| n.to_os_string()) == name) {
                    let _ = tx.send(());
                }
            }
        })?;
        w.watch(&dir, RecursiveMode::NonRecursive)?;
        Ok((w, rx))
    }

    pub fn is_polling(&self) -> bool {
        self.rx.is_none()
    }

    /// Called from the main loop; `true` — time to reread the file.
    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        if let Some(rx) = &self.rx {
            while rx.try_recv().is_ok() {
                self.pending = Some(now);
            }
        } else if now.duration_since(self.last_poll) >= POLL_EVERY {
            self.last_poll = now;
            let m = mtime(&self.path);
            if m != self.mtime {
                self.mtime = m;
                self.pending = Some(now - DEBOUNCE);
            }
        }
        match self.pending {
            Some(t) if now.duration_since(t) >= DEBOUNCE => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }
}
