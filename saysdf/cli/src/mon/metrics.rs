//! System metrics collector, formatting, ring buffer and braille graph
//! (system-monitor D1, D2, D4). The pure functions are covered by the tests at the bottom.
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{
    CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System,
};

/// One metrics snapshot. Rates are in bytes/s.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub ts: u64,
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub disk_read: u64,
    pub disk_write: u64,
    /// GPU load, % (None means the OS/driver does not provide it).
    pub gpu: Option<f32>,
}

impl Sample {
    /// Status file JSON (one line; the format is read by `saysdf.monitor`).
    pub fn to_json(&self) -> Value {
        json!({
            "v": 1,
            "ts": self.ts,
            "cpu": round1(self.cpu),
            "cores": self.cores.iter().map(|c| round1(*c)).collect::<Vec<_>>(),
            "mem_used": self.mem_used,
            "mem_total": self.mem_total,
            "swap_used": self.swap_used,
            "swap_total": self.swap_total,
            "net_rx": self.net_rx,
            "net_tx": self.net_tx,
            "disk_read": self.disk_read,
            "disk_write": self.disk_write,
            "gpu": self.gpu.map(round1),
        })
    }
}

fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Collector on top of sysinfo: CPU, memory, network, disk (no processes).
pub struct Sampler {
    pub sys: System,
    networks: Networks,
    disks: Disks,
    last: Instant,
    gpu: GpuProbe,
}

impl Sampler {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::everything()),
        );
        // The first CPU reading in sysinfo is 0: take a second one after the minimum interval (design, Risks).
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_usage();
        let networks = Networks::new_with_refreshed_list();
        let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_io_usage());
        Self { sys, networks, disks, last: Instant::now(), gpu: GpuProbe::detect() }
    }

    pub fn sample(&mut self) -> Sample {
        let dt = self.last.elapsed().as_secs_f64().max(0.001);
        self.last = Instant::now();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.networks.refresh(true);
        self.disks.refresh_specifics(true, DiskRefreshKind::nothing().with_io_usage());
        let (rx, tx) = self
            .networks
            .iter()
            .fold((0u64, 0u64), |(r, t), (_, n)| (r + n.received(), t + n.transmitted()));
        let (dr, dw) = self.disks.iter().fold((0u64, 0u64), |(r, w), d| {
            let u = d.usage();
            (r + u.read_bytes, w + u.written_bytes)
        });
        let per = |v: u64| (v as f64 / dt) as u64;
        Sample {
            ts: now_unix(),
            cpu: self.sys.global_cpu_usage(),
            cores: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            net_rx: per(rx),
            net_tx: per(tx),
            disk_read: per(dr),
            disk_write: per(dw),
            gpu: self.gpu.read(),
        }
    }
}

// ---------- GPU ----------

/// GPU load source: amdgpu sysfs (Linux) or `nvidia-smi` from PATH (with a timeout).
/// Windows PDH is not implemented in the MVP: without nvidia-smi the GPU is shown as unavailable.
enum GpuProbe {
    None,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Sysfs(Vec<std::path::PathBuf>),
    NvidiaSmi(std::path::PathBuf),
}

impl GpuProbe {
    fn detect() -> Self {
        #[cfg(target_os = "linux")]
        {
            let mut v = Vec::new();
            if let Ok(rd) = std::fs::read_dir("/sys/class/drm") {
                for e in rd.flatten() {
                    let p = e.path().join("device/gpu_busy_percent");
                    if p.exists() {
                        v.push(p);
                    }
                }
            }
            if !v.is_empty() {
                return GpuProbe::Sysfs(v);
            }
        }
        match find_in_path(if cfg!(windows) { "nvidia-smi.exe" } else { "nvidia-smi" }) {
            Some(p) => GpuProbe::NvidiaSmi(p),
            None => GpuProbe::None,
        }
    }

    fn read(&self) -> Option<f32> {
        match self {
            GpuProbe::None => None,
            GpuProbe::Sysfs(paths) => {
                let vals: Vec<f32> = paths
                    .iter()
                    .filter_map(|p| std::fs::read_to_string(p).ok())
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                vals.into_iter().reduce(f32::max)
            }
            GpuProbe::NvidiaSmi(bin) => {
                let out = run_with_timeout(
                    bin,
                    &["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"],
                    Duration::from_millis(1500),
                )?;
                parse_nvidia_smi(&out)
            }
        }
    }
}

fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

fn run_with_timeout(bin: &std::path::Path, args: &[&str], timeout: Duration) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(bin);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut s = String::new();
    child.stdout.take()?.read_to_string(&mut s).ok()?;
    Some(s)
}

/// Parse `nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits`:
/// one line per GPU; take the maximum. `[N/A]` and garbage are skipped.
pub fn parse_nvidia_smi(out: &str) -> Option<f32> {
    out.lines()
        .filter_map(|l| l.trim().trim_end_matches('%').trim().parse::<f32>().ok())
        .filter(|v| (0.0..=100.0).contains(v))
        .reduce(f32::max)
}

// ---------- Formatting ----------

/// `12.4` → `12%`.
pub fn fmt_pct(v: f32) -> String {
    format!("{:.0}%", v.clamp(0.0, 999.0))
}

/// Bytes in short form: `1.2M`, `40K`, `512B`, `3.4G`.
pub fn fmt_bytes(v: u64) -> String {
    const UNITS: [&str; 5] = ["K", "M", "G", "T", "P"];
    if v < 1024 {
        return format!("{v}B");
    }
    let mut x = v as f64 / 1024.0;
    let mut i = 0;
    while x >= 1024.0 && i + 1 < UNITS.len() {
        x /= 1024.0;
        i += 1;
    }
    if x < 10.0 {
        format!("{x:.1}{}", UNITS[i])
    } else {
        format!("{x:.0}{}", UNITS[i])
    }
}

/// Used/total in gigabytes: `7.1/16G`.
pub fn fmt_mem(used: u64, total: u64) -> String {
    let g = |v: u64| v as f64 / (1024.0 * 1024.0 * 1024.0);
    let t = g(total);
    let total_s = if t >= 10.0 || (t - t.round()).abs() < 0.05 { format!("{t:.0}") } else { format!("{t:.1}") };
    format!("{:.1}/{}G", g(used), total_s)
}

/// Metrics line in the status bar style: `CPU 12% RAM 7.1/16G ↓1.2M ↑40K D 3M/s`.
pub fn status_line(s: &Sample) -> String {
    let mut out = format!(
        "CPU {} RAM {} ↓{} ↑{} D {}/s",
        fmt_pct(s.cpu),
        fmt_mem(s.mem_used, s.mem_total),
        fmt_bytes(s.net_rx),
        fmt_bytes(s.net_tx),
        fmt_bytes(s.disk_read + s.disk_write)
    );
    if let Some(g) = s.gpu {
        out.push_str(&format!(" GPU {}", fmt_pct(g)));
    }
    out
}

// ---------- Ring buffer ----------

#[derive(Clone, Debug)]
pub struct Ring {
    cap: usize,
    buf: VecDeque<f64>,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), buf: VecDeque::with_capacity(cap) }
    }
    pub fn push(&mut self, v: f64) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
    }
    pub fn values(&self) -> Vec<f64> {
        self.buf.iter().copied().collect()
    }
    pub fn max(&self) -> f64 {
        self.buf.iter().copied().fold(0.0, f64::max)
    }
}

// ---------- Braille graph ----------

/// Multi-line braille graph: `width`×`height` cells, each 2 columns × 4 dots.
/// The last `2*width` values are used (newest on the right), scale `0..=max`.
pub fn braille_graph(values: &[f64], max: f64, width: usize, height: usize) -> Vec<String> {
    const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01]; // bottom to top
    const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let cols = width * 2;
    let dots = height * 4;
    let start = values.len().saturating_sub(cols);
    let tail = &values[start..];
    let pad = cols - tail.len();
    let level = |v: f64| -> usize {
        if max.is_nan() || max <= 0.0 || !v.is_finite() || v <= 0.0 {
            0
        } else {
            let l = ((v / max).min(1.0) * dots as f64).round() as usize;
            l.max(1)
        }
    };
    let levels: Vec<usize> = std::iter::repeat_n(0, pad).chain(tail.iter().map(|v| level(*v))).collect();
    let mut rows = Vec::with_capacity(height);
    for row in 0..height {
        // row 0 is the top row; the dots of this row run from base to base+3 (from the bottom)
        let base = (height - 1 - row) * 4;
        let mut s = String::with_capacity(width * 3);
        for cx in 0..width {
            let mut bits = 0u32;
            for (side, table) in [(0usize, &LEFT), (1usize, &RIGHT)] {
                let l = levels[cx * 2 + side];
                for (k, bit) in table.iter().enumerate() {
                    if l > base + k {
                        bits |= bit;
                    }
                }
            }
            s.push(char::from_u32(0x2800 + bits).unwrap_or(' '));
        }
        rows.push(s);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_pct(12.4), "12%");
        assert_eq!(fmt_bytes(1_258_291), "1.2M");
        assert_eq!(fmt_bytes(40 * 1024), "40K");
        assert_eq!(fmt_bytes(100), "100B");
        let g = 1024u64 * 1024 * 1024;
        assert_eq!(fmt_mem(g * 71 / 10, g * 16), "7.1/16G");
    }

    #[test]
    fn status_line_matches_spec_shape() {
        let g = 1024u64 * 1024 * 1024;
        let s = Sample {
            cpu: 12.0,
            mem_used: g * 71 / 10,
            mem_total: 16 * g,
            net_rx: 1_258_291,
            net_tx: 40 * 1024,
            disk_read: 3 * 1024 * 1024,
            ..Default::default()
        };
        assert_eq!(status_line(&s), "CPU 12% RAM 7.1/16G ↓1.2M ↑40K D 3.0M/s");
    }

    #[test]
    fn ring_keeps_last() {
        let mut r = Ring::new(3);
        for i in 0..5 {
            r.push(i as f64);
        }
        assert_eq!(r.values(), vec![2.0, 3.0, 4.0]);
        assert_eq!(r.max(), 4.0);
    }

    #[test]
    fn braille_full_and_empty() {
        let g = braille_graph(&[100.0, 100.0], 100.0, 1, 1);
        assert_eq!(g, vec!["⣿".to_string()]);
        let g = braille_graph(&[0.0, 0.0], 100.0, 1, 2);
        assert_eq!(g, vec!["⠀".to_string(), "⠀".to_string()]);
        // left column full, right column empty; two rows
        let g = braille_graph(&[100.0, 0.0], 100.0, 1, 2);
        assert_eq!(g, vec!["⡇".to_string(), "⡇".to_string()]);
        // half height with two rows: top empty, bottom full
        let g = braille_graph(&[50.0, 50.0], 100.0, 1, 2);
        assert_eq!(g, vec!["⠀".to_string(), "⣿".to_string()]);
        // little data: padding on the left; max=0 does not panic
        let g = braille_graph(&[1.0], 0.0, 3, 1);
        assert_eq!(g[0].chars().count(), 3);
    }

    #[test]
    fn nvidia_smi_parse() {
        assert_eq!(parse_nvidia_smi("37\n12\n"), Some(37.0));
        assert_eq!(parse_nvidia_smi("[N/A]\n"), None);
        assert_eq!(parse_nvidia_smi(""), None);
    }

    #[test]
    fn json_roundtrip_fields() {
        let s = Sample { ts: 5, cpu: 12.34, cores: vec![1.0, 2.55], mem_total: 10, gpu: None, ..Default::default() };
        let v: Value = serde_json::from_str(&s.to_json().to_string()).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["ts"], 5);
        assert_eq!(v["cpu"], 12.3);
        assert_eq!(v["cores"].as_array().unwrap().len(), 2);
        assert!(v["gpu"].is_null());
        assert_eq!(v["mem_total"], 10);
    }
}
