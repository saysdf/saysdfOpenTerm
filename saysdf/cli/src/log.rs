//! Debug log of the TUI tools: `SAYSDF_LOG=<file>` appends lines to the file (input, screen
//! transitions). Without the variable it does nothing.

use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

fn path() -> Option<&'static std::path::PathBuf> {
    static P: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        std::env::var_os("SAYSDF_LOG")
            .filter(|v| !v.is_empty())
            .map(Into::into)
    })
    .as_ref()
}

pub fn enabled() -> bool {
    path().is_some()
}

pub fn write(msg: &str) {
    let Some(p) = path() else { return };
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{ms} {msg}");
    }
}

/// `slog!("...", args)` — writes to the log; formats only when the log is enabled.
macro_rules! slog {
    ($($t:tt)*) => {
        if $crate::log::enabled() {
            $crate::log::write(&format!($($t)*));
        }
    };
}
pub(crate) use slog;
