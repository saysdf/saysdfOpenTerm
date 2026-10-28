//! Write ("dangerous") mode state machine (design D5, spec `database-write-mode`).
//!
//! `ReadOnly` -> (toolbar button only) `ConfirmPending` -> (typed database name) `Write`.
//! Any reconnect / connection switch / database or branch switch / start -> `ReadOnly`.
//! Exit from `Write` is one action.

use std::time::SystemTime;

/// The only event that may start the transition: the toolbar button activated by mouse or
/// Enter on the focused button. There is deliberately no hotkey variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonActivated {
    WriteButton,
}

/// Proof that the user confirmed write mode for a database; required by
/// [`crate::Access::Write`]. Not constructible outside this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteGrant {
    database: String,
}

impl WriteGrant {
    pub fn database(&self) -> &str {
        &self.database
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WriteMode {
    #[default]
    ReadOnly,
    ConfirmPending {
        db: String,
    },
    Write {
        db: String,
        since: SystemTime,
        reason: String,
    },
}

/// Why the mode was reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetCause {
    Reconnect,
    SwitchConnection,
    SwitchDatabase,
    SwitchBranch,
}

impl WriteMode {
    pub fn is_write(&self) -> bool {
        matches!(self, WriteMode::Write { .. })
    }

    /// Button pressed: ask for confirmation for `db`. Ignored unless in `ReadOnly`.
    pub fn request_write(&mut self, _event: ButtonActivated, db: &str) {
        if matches!(self, WriteMode::ReadOnly) && !db.is_empty() {
            *self = WriteMode::ConfirmPending { db: db.into() };
        }
    }

    /// Typed name must equal the database name exactly; otherwise back to `ReadOnly`.
    /// On success returns the grant the caller uses to reconnect without read-only.
    pub fn confirm(&mut self, typed_name: &str, reason: &str) -> Option<WriteGrant> {
        let WriteMode::ConfirmPending { db } = self else {
            return None;
        };
        if typed_name != db.as_str() {
            *self = WriteMode::ReadOnly;
            return None;
        }
        let db = db.clone();
        *self = WriteMode::Write {
            db: db.clone(),
            since: SystemTime::now(),
            reason: reason.into(),
        };
        Some(WriteGrant { database: db })
    }

    /// Cancel the confirmation dialog.
    pub fn cancel(&mut self) {
        if matches!(self, WriteMode::ConfirmPending { .. }) {
            *self = WriteMode::ReadOnly;
        }
    }

    /// One action (click on the banner, `Esc Esc`, `F12`).
    pub fn exit_write(&mut self) {
        *self = WriteMode::ReadOnly;
    }

    pub fn reset(&mut self, _cause: ResetCause) {
        *self = WriteMode::ReadOnly;
    }

    /// Current grant, if in `Write`.
    pub fn grant(&self) -> Option<WriteGrant> {
        match self {
            WriteMode::Write { db, .. } => Some(WriteGrant {
                database: db.clone(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_read_only_and_confirm_without_request_does_nothing() {
        let mut m = WriteMode::default();
        assert_eq!(m, WriteMode::ReadOnly);
        assert!(m.confirm("db", "x").is_none());
        assert_eq!(m, WriteMode::ReadOnly);
        assert!(m.grant().is_none());
    }

    #[test]
    fn wrong_name_returns_to_read_only() {
        let mut m = WriteMode::default();
        m.request_write(ButtonActivated::WriteButton, "prod");
        assert_eq!(m, WriteMode::ConfirmPending { db: "prod".into() });
        assert!(m.confirm("Prod", "fix").is_none());
        assert_eq!(m, WriteMode::ReadOnly);
    }

    #[test]
    fn confirm_exit_and_resets() {
        let mut m = WriteMode::default();
        m.request_write(ButtonActivated::WriteButton, "prod");
        let g = m.confirm("prod", "fix").unwrap();
        assert_eq!(g.database(), "prod");
        assert!(m.is_write());
        m.request_write(ButtonActivated::WriteButton, "other");
        assert!(m.is_write());
        m.exit_write();
        assert_eq!(m, WriteMode::ReadOnly);

        for cause in [
            ResetCause::Reconnect,
            ResetCause::SwitchConnection,
            ResetCause::SwitchDatabase,
            ResetCause::SwitchBranch,
        ] {
            m.request_write(ButtonActivated::WriteButton, "prod");
            m.confirm("prod", "").unwrap();
            m.reset(cause);
            assert_eq!(m, WriteMode::ReadOnly);
        }
        m.request_write(ButtonActivated::WriteButton, "prod");
        m.cancel();
        assert_eq!(m, WriteMode::ReadOnly);
    }
}
