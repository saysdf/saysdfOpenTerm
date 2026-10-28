//! Platform actions on processes (system-monitor D3). With the user's own rights only:
//! no elevation, an OS refusal is returned as `permission denied`. Called only on an explicit action.
use super::procs::Action;
use std::path::Path;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-mon");

pub type Result<T> = std::result::Result<T, String>;

fn os_err(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(5) {
        I18N.t("permission denied").into_owned()
    } else {
        e.to_string()
    }
}

/// Perform the action on a PID. CopyCmd/OpenDir/Tree are handled by the UI.
pub fn apply(pid: u32, action: Action) -> Result<String> {
    match action {
        Action::Suspend => imp::suspend(pid).map(|_| I18N.t("paused").into_owned()),
        Action::Resume => imp::resume(pid).map(|_| I18N.t("resumed").into_owned()),
        Action::Stop => imp::stop(pid).map(|_| I18N.t("stop requested").into_owned()),
        Action::Kill => imp::kill(pid).map(|_| I18N.t("terminated").into_owned()),
        Action::PriorityLower => imp::priority(pid, false),
        Action::PriorityRaise => imp::priority(pid, true),
        Action::Tree | Action::CopyCmd | Action::OpenDir => Ok(String::new()),
    }
}

/// OSC 52: put a string on the terminal clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= ch.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Open the exe folder in the file manager (only on a user action).
pub fn open_dir(exe: &Path) -> Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd;
    if cfg!(windows) {
        cmd = Command::new("explorer");
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(exe.as_os_str());
        cmd.arg(arg);
    } else if cfg!(target_os = "macos") {
        cmd = Command::new("open");
        cmd.arg("-R").arg(exe);
    } else {
        cmd = Command::new("xdg-open");
        cmd.arg(exe.parent().unwrap_or(exe));
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map(|_| ()).map_err(os_err)
}

#[cfg(unix)]
mod imp {
    use super::{os_err, Result};

    fn signal(pid: u32, sig: libc::c_int) -> Result<()> {
        // SAFETY: kill(2) with a specific PID (>0) and a signal; no side effects in our own process.
        let r = unsafe { libc::kill(pid as libc::pid_t, sig) };
        if r == 0 {
            Ok(())
        } else {
            Err(os_err(std::io::Error::last_os_error()))
        }
    }
    pub fn suspend(pid: u32) -> Result<()> {
        signal(pid, libc::SIGSTOP)
    }
    pub fn resume(pid: u32) -> Result<()> {
        signal(pid, libc::SIGCONT)
    }
    pub fn stop(pid: u32) -> Result<()> {
        signal(pid, libc::SIGTERM)
    }
    pub fn kill(pid: u32) -> Result<()> {
        signal(pid, libc::SIGKILL)
    }
    pub fn priority(pid: u32, raise: bool) -> Result<String> {
        // SAFETY: getpriority/setpriority for a single PID; the values are checked by the OS.
        let cur = unsafe { libc::getpriority(libc::PRIO_PROCESS as _, pid as libc::id_t) };
        let new = (if raise { cur - 5 } else { cur + 5 }).clamp(-20, 19);
        let r = unsafe { libc::setpriority(libc::PRIO_PROCESS as _, pid as libc::id_t, new) };
        if r == 0 {
            Ok(format!("nice {cur} → {new}"))
        } else {
            Err(os_err(std::io::Error::last_os_error()))
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{os_err, Result, I18N};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{
        GetPriorityClass, OpenProcess, SetPriorityClass, TerminateProcess, ABOVE_NORMAL_PRIORITY_CLASS,
        BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SUSPEND_RESUME,
        PROCESS_TERMINATE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    };

    #[link(name = "ntdll")]
    extern "system" {
        fn NtSuspendProcess(h: HANDLE) -> i32;
        fn NtResumeProcess(h: HANDLE) -> i32;
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from OpenProcess and is closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn open(pid: u32, access: u32) -> Result<Handle> {
        // SAFETY: OpenProcess without inheritance; null is an error and is checked.
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            Err(os_err(std::io::Error::last_os_error()))
        } else {
            Ok(Handle(h))
        }
    }

    fn nt(status: i32) -> Result<()> {
        match status {
            0 => Ok(()),
            s if s as u32 == 0xC000_0022 => Err(I18N.t("permission denied").into_owned()), // STATUS_ACCESS_DENIED
            s => Err(format!("NTSTATUS 0x{:08X}", s as u32)),
        }
    }

    pub fn suspend(pid: u32) -> Result<()> {
        let h = open(pid, PROCESS_SUSPEND_RESUME)?;
        // SAFETY: a valid handle with the SUSPEND_RESUME right.
        nt(unsafe { NtSuspendProcess(h.0) })
    }
    pub fn resume(pid: u32) -> Result<()> {
        let h = open(pid, PROCESS_SUSPEND_RESUME)?;
        // SAFETY: see suspend.
        nt(unsafe { NtResumeProcess(h.0) })
    }
    pub fn kill(pid: u32) -> Result<()> {
        let h = open(pid, PROCESS_TERMINATE)?;
        // SAFETY: a valid handle with the TERMINATE right.
        if unsafe { TerminateProcess(h.0, 1) } != 0 {
            Ok(())
        } else {
            Err(os_err(std::io::Error::last_os_error()))
        }
    }

    struct Ctx {
        pid: u32,
        posted: u32,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lp: LPARAM) -> i32 {
        // SAFETY: lp points to the Ctx from stop() and lives for the duration of EnumWindows.
        let ctx = unsafe { &mut *(lp as *mut Ctx) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == ctx.pid && unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
            ctx.posted += 1;
        }
        1
    }

    /// Soft stop: WM_CLOSE to the visible windows of the process. A console process has nothing to close.
    pub fn stop(pid: u32) -> Result<()> {
        let mut ctx = Ctx { pid, posted: 0 };
        // SAFETY: the callback does not keep the pointer after EnumWindows returns.
        unsafe { EnumWindows(Some(enum_cb), &mut ctx as *mut Ctx as LPARAM) };
        if ctx.posted > 0 {
            Ok(())
        } else {
            Err(I18N.t("the process has no windows — use Kill").into_owned())
        }
    }

    const LADDER: [u32; 5] = [
        IDLE_PRIORITY_CLASS,
        BELOW_NORMAL_PRIORITY_CLASS,
        NORMAL_PRIORITY_CLASS,
        ABOVE_NORMAL_PRIORITY_CLASS,
        HIGH_PRIORITY_CLASS,
    ];
    const NAMES: [&str; 5] = ["idle", "below normal", "normal", "above normal", "high"];

    pub fn priority(pid: u32, raise: bool) -> Result<String> {
        let h = open(pid, PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION)?;
        // SAFETY: a valid handle with the needed rights.
        let cur = unsafe { GetPriorityClass(h.0) };
        let i = LADDER.iter().position(|c| *c == cur).unwrap_or(2);
        let j = if raise { (i + 1).min(LADDER.len() - 1) } else { i.saturating_sub(1) };
        // SAFETY: see above; REALTIME is not used.
        if unsafe { SetPriorityClass(h.0, LADDER[j]) } != 0 {
            Ok(I18N.f("priority {from} → {to}", &[("from", &NAMES[i]), ("to", &NAMES[j])]))
        } else {
            Err(os_err(std::io::Error::last_os_error()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_osc52() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }

    /// Pause/resume/kill only on our own dummy child process.
    #[test]
    fn suspend_resume_kill_own_child() {
        use std::process::{Command, Stdio};
        let mut child = if cfg!(windows) {
            Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).spawn().unwrap()
        } else {
            Command::new("sleep").arg("30").spawn().unwrap()
        };
        let pid = child.id();
        assert!(apply(pid, Action::Suspend).is_ok());
        assert!(apply(pid, Action::Resume).is_ok());
        assert!(apply(pid, Action::PriorityLower).is_ok());
        assert!(apply(pid, Action::Kill).is_ok());
        let st = child.wait().unwrap();
        assert!(!st.success());
    }
}
