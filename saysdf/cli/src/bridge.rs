//! Trusted channel "TUI in a pane -> terminal" (Rust side of the Lua `saysdf.bridge`; change
//! text-annotations D7, extracted from the sidebar, change tool-sidebar D3). The TUI sends the user
//! var `<VAR>` = `<token>:<kind>:<payload>` with OSC 1337 SetUserVar (value in base64). The token is
//! the one-time token the GUI put into the TUI's environment when it opened the pane; the GUI
//! accepts the value only from its own pane, with `saysdf` in the foreground, with that token, no
//! longer than [`MAX_VALUE`] and with a kind from its allow-list.
//!
//! Every SetUserVar raises `user-var-changed` in the GUI (the terminal does not compare with the
//! previous value), so each send attempt is a separate event: senders with retries must make the
//! Lua handler tolerate repeats (exact-repeat refusal of `saysdf.bridge.consume`).

use std::io::{self, Write};
use std::time::Duration;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-git");

/// Value length limit (bytes); the GUI checks the same one (`saysdf.bridge.MAX_VALUE`).
pub const MAX_VALUE: usize = 4096;
/// Pause between send retries.
const RETRY_DELAY: Duration = Duration::from_millis(200);

/// Takes the token from the environment variable `env` and removes it there, so child processes
/// (the editor through `pager::suspend`, `saysdf diff`...) do not inherit it. The token is
/// `[0-9a-f-]` only, 8..=64 characters; anything else is no token.
pub fn take_token(env: &str) -> Option<String> {
    let t = std::env::var(env).ok();
    std::env::remove_var(env);
    t.filter(|t| valid_token(t))
}

pub fn valid_token(t: &str) -> bool {
    (8..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// The user var value; `None` — longer than [`MAX_VALUE`] or a kind/token with `:`.
pub fn value(token: &str, kind: &str, payload: &str) -> Option<String> {
    if token.contains(':') || kind.contains(':') {
        return None;
    }
    let v = format!("{token}:{kind}:{payload}");
    (v.len() <= MAX_VALUE).then_some(v)
}

/// Parses a `<token>:<kind>:<payload>` value (the payload may contain `:`) — mirrors
/// `saysdf.bridge.parse`, keeps the format under test.
#[cfg(test)]
pub fn parse_value(v: &str) -> Option<(&str, &str, &str)> {
    let mut it = v.splitn(3, ':');
    Some((it.next()?, it.next()?, it.next()?))
}

/// OSC 1337 SetUserVar `var` = base64(`value`).
pub fn osc(var: &str, value: &str) -> String {
    format!("\x1b]1337;SetUserVar={var}={}\x07", crate::cmd::md::base64(value.as_bytes()))
}

/// One channel: the user var name, the token of this opening and how many times a value is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub var: &'static str,
    pub token: String,
    /// Send attempts (>= 1); more than one only where the Lua side refuses exact repeats.
    pub retries: usize,
}

impl Channel {
    /// The OSC sequence for `kind`/`payload`; an error when the value is over the limit.
    pub fn encode(&self, kind: &str, payload: &str) -> io::Result<String> {
        match value(&self.token, kind, payload) {
            Some(v) => Ok(osc(self.var, &v)),
            None => {
                let msg = I18N.t("the result is longer than 4 KB").into_owned();
                Err(io::Error::new(io::ErrorKind::InvalidInput, msg))
            }
        }
    }

    /// Writes the value to `out` [`Channel::retries`] times.
    pub fn send_to(&self, out: &mut impl Write, kind: &str, payload: &str) -> io::Result<()> {
        let seq = self.encode(kind, payload)?;
        for i in 0..self.retries.max(1) {
            if i > 0 {
                std::thread::sleep(RETRY_DELAY);
            }
            out.write_all(seq.as_bytes())?;
            out.flush()?;
        }
        Ok(())
    }

    /// Sends to stdout (the terminal of the pane).
    pub fn send(&self, kind: &str, payload: &str) -> io::Result<()> {
        self.send_to(&mut io::stdout(), kind, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_and_limit() {
        let ch = Channel { var: "SAYSDF_X", token: "abcd1234".into(), retries: 1 };
        assert_eq!(ch.encode("k", "p:q").unwrap(), osc("SAYSDF_X", "abcd1234:k:p:q"));
        assert_eq!(osc("SAYSDF_X", "a:b:c"), "\x1b]1337;SetUserVar=SAYSDF_X=YTpiOmM=\x07");
        // "abcd1234:k:" is 11 bytes.
        assert!(ch.encode("k", &"x".repeat(MAX_VALUE - 11)).is_ok());
        let e = ch.encode("k", &"x".repeat(MAX_VALUE - 10)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(ch.encode("a:b", "p").is_err());
    }

    #[test]
    fn send_writes_each_attempt() {
        let ch = Channel { var: "V", token: "abcd1234".into(), retries: 2 };
        let mut out = Vec::new();
        ch.send_to(&mut out, "k", "p").unwrap();
        let one = osc("V", "abcd1234:k:p");
        assert_eq!(String::from_utf8(out).unwrap(), one.repeat(2));
        let mut out = Vec::new();
        Channel { retries: 0, ..ch }.send_to(&mut out, "k", "p").unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), one);
    }

    #[test]
    fn take_token_removes_and_validates() {
        let env = "SAYSDF_BRIDGE_TEST_TOKEN";
        std::env::set_var(env, "0123abcd-89ef");
        assert_eq!(take_token(env).as_deref(), Some("0123abcd-89ef"));
        assert!(std::env::var(env).is_err());
        std::env::set_var(env, "bad;token!");
        assert_eq!(take_token(env), None);
        assert_eq!(take_token(env), None);
    }
}
