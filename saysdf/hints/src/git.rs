//! Running the git CLI: directory, safe polling mode (no hooks/fsmonitor/locks), timeout with kill.
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("hints");

/// Default timeout of background polling (design D3).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A git invocation.
#[derive(Debug, Clone)]
pub struct Git {
    dir: Option<PathBuf>,
    safe: bool,
    timeout: Option<Duration>,
}

/// Result of [`Git::run`].
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status.success()
    }
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Error running git.
#[derive(Debug)]
pub enum Error {
    /// git is not on PATH.
    NotFound,
    /// The process exceeded the timeout and was killed.
    Timeout,
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => f.write_str(&I18N.t("git not found in PATH")),
            Error::Timeout => f.write_str(&I18N.t("git did not respond in time")),
            Error::Io(e) => write!(f, "git: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            Error::NotFound
        } else {
            Error::Io(e)
        }
    }
}

impl Default for Git {
    fn default() -> Self {
        Self::new()
    }
}

impl Git {
    /// Interactive call: no safe flags and no timeout (commit/push see hooks, password prompts).
    pub fn new() -> Self {
        Self {
            dir: None,
            safe: false,
            timeout: None,
        }
    }

    /// Background polling: safe flags and env (D3), timeout [`DEFAULT_TIMEOUT`].
    pub fn safe() -> Self {
        Self {
            dir: None,
            safe: true,
            timeout: Some(DEFAULT_TIMEOUT),
        }
    }

    /// Working directory (`git -C <dir>`).
    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// Timeout (`None` = unlimited).
    pub fn timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// Full list of git arguments (without the program name).
    pub fn args<I, S>(&self, args: I) -> Vec<OsString>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut out: Vec<OsString> = Vec::new();
        if let Some(dir) = &self.dir {
            out.push("-C".into());
            out.push(dir.as_os_str().to_owned());
        }
        if self.safe {
            for a in [
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "status.submoduleSummary=false",
            ] {
                out.push(a.into());
            }
        }
        out.extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        out
    }

    /// Environment variables of the call.
    pub fn envs(&self) -> Vec<(&'static str, &'static str)> {
        if self.safe {
            vec![
                ("GIT_OPTIONAL_LOCKS", "0"),
                ("GIT_TERMINAL_PROMPT", "0"),
                ("LC_ALL", "C"),
            ]
        } else {
            Vec::new()
        }
    }

    /// The ready command (stdio not configured).
    pub fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new("git");
        cmd.args(self.args(args));
        for (k, v) in self.envs() {
            cmd.env(k, v);
        }
        cmd
    }

    /// Runs capturing stdout/stderr; stdin is closed. The process is killed on timeout.
    pub fn run<I, S>(&self, args: I) -> Result<Output, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        run_command(self.command(args), self.timeout)
    }
}

fn run_command(mut cmd: Command, timeout: Option<Duration>) -> Result<Output, Error> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let out_h = read_thread(child.stdout.take());
    let err_h = read_thread(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if let Some(t) = timeout {
            if start.elapsed() > t {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Timeout);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Ok(Output {
        status,
        stdout: out_h.join().unwrap_or_default(),
        stderr: err_h.join().unwrap_or_default(),
    })
}

fn read_thread<R: Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocking_command() -> Command {
        #[cfg(windows)]
        {
            let mut c = Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        }
    }

    #[test]
    fn timeout_kills_a_blocking_process() {
        let t = Instant::now();
        let r = run_command(blocking_command(), Some(Duration::from_millis(100)));
        assert!(matches!(r, Err(Error::Timeout)), "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn error_text_follows_language() {
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(Error::NotFound.to_string(), "git not found in PATH");
            assert_eq!(Error::Timeout.to_string(), "git did not respond in time");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(Error::NotFound.to_string(), "git не найден в PATH"); // i18n-allow-cyrillic
    }

    #[test]
    fn safe_args_and_env() {
        let g = Git::safe().dir("/r");
        let args: Vec<String> = g
            .args(["status"])
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-C",
                "/r",
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "status.submoduleSummary=false",
                "status"
            ]
        );
        assert!(g.envs().contains(&("GIT_TERMINAL_PROMPT", "0")));
        assert!(g.envs().contains(&("GIT_OPTIONAL_LOCKS", "0")));
        let plain = Git::new();
        assert_eq!(plain.args(["push"]), [OsString::from("push")]);
        assert!(plain.envs().is_empty());
    }
}
