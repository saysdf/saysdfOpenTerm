//! `saysdf mon` — load and process monitor; `--status-file` is the background collector for
//! the status bar (change system-monitor, design D1–D3).
pub mod actions;
pub mod metrics;
pub mod procs;
mod ui;

use clap::Args;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-mon");

#[derive(Args)]
pub struct MonArgs {
    /// Background mode: atomically write the metrics JSON to a file every `--interval` seconds.
    /// Exits when the file is deleted or the `--parent-pid` process has ended.
    #[arg(long, value_name = "PATH")]
    pub status_file: Option<PathBuf>,
    /// Sampling interval in seconds (at least 1)
    #[arg(long, default_value_t = 2.0)]
    pub interval: f64,
    /// PID of the parent terminal: the collector exits without it
    #[arg(long, value_name = "PID")]
    pub parent_pid: Option<u32>,
    /// Highlight a process (the tab foreground PID) and its descendants
    #[arg(long, value_name = "PID")]
    pub highlight_pid: Option<u32>,
}

pub fn run(args: MonArgs) -> i32 {
    let interval = Duration::from_secs_f64(if args.interval.is_finite() { args.interval.max(1.0) } else { 2.0 });
    match &args.status_file {
        Some(path) => match status_loop(path, interval, args.parent_pid) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{}", I18N.f("saysdf mon: {error}", &[("error", &e)]));
                1
            }
        },
        None => match ui::run(interval, args.highlight_pid) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{}", I18N.f("saysdf mon: {error}", &[("error", &e)]));
                1
            }
        },
    }
}

/// Atomic write: a temporary file next to it + rename.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn parent_alive(sys: &mut System, pid: u32) -> bool {
    let p = Pid::from_u32(pid);
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, ProcessRefreshKind::nothing());
    sys.process(p).is_some()
}

fn status_loop(path: &Path, interval: Duration, parent: Option<u32>) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut sampler = metrics::Sampler::new();
    let mut psys = System::new();
    let mut first = true;
    loop {
        if let Some(pp) = parent {
            if !parent_alive(&mut psys, pp) {
                let _ = std::fs::remove_file(path);
                return Ok(());
            }
        }
        // The file was deleted (metrics turned off in Lua): exit. On the first pass the file may not exist yet.
        if !first && !path.exists() {
            return Ok(());
        }
        let s = sampler.sample();
        write_atomic(path, &s.to_json().to_string())?;
        first = false;
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_in_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("mon.json");
        let s = metrics::Sample { ts: 1, cpu: 5.0, ..Default::default() };
        write_atomic(&p, &s.to_json().to_string()).unwrap();
        write_atomic(&p, &s.to_json().to_string()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["cpu"], 5.0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "the temporary file did not remain");
    }

    #[test]
    fn exits_without_parent() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("mon.json");
        // A PID that almost certainly does not exist
        status_loop(&p, Duration::from_secs(1), Some(0x7fff_fff0)).unwrap();
        assert!(!p.exists());
    }
}
