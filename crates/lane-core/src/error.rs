//! One error type for every command. Each error has a kind (which sets its exit code and its
//! machine-readable `code`), a message saying what went wrong, and a hint saying what to do next.
//!
//! With `--json`, a failed command prints exactly one object on stdout and nothing else:
//!
//! ```json
//! { "error": { "code": "not_a_repo", "message": "…", "hint": "…", "exit": 1 } }
//! ```
//!
//! `code` values and exit codes are part of the CLI contract: add new ones, never rename them.

use crate::exit;
use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The command line was wrong: unknown command or option, missing argument.
    Usage,
    /// The command exists but isn't built yet.
    NotImplemented,
    /// The folder isn't inside a Git repo.
    NotARepo,
    /// A lane (or something else that was named) doesn't exist.
    NotFound,
    /// The request clashes with what's there: a name that's taken, a request id used for
    /// another command.
    Conflict,
    /// `lane.toml` or the settings file is invalid.
    Config,
    /// Lanes' own state store couldn't be read or written.
    State,
    /// A file or folder couldn't be read or written.
    Io,
    /// Anything else that went wrong while running the command.
    Failure,
}

impl ErrorKind {
    /// The stable machine-readable name, as in the JSON `code` field.
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::Usage => "usage",
            ErrorKind::NotImplemented => "not_implemented",
            ErrorKind::NotARepo => "not_a_repo",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Config => "config",
            ErrorKind::State => "state",
            ErrorKind::Io => "io",
            ErrorKind::Failure => "failure",
        }
    }

    /// The process exit code (see [`crate::exit`]).
    pub fn exit_code(self) -> u8 {
        match self {
            ErrorKind::Usage => exit::USAGE,
            ErrorKind::NotImplemented => exit::NOT_IMPLEMENTED,
            _ => exit::FAILURE,
        }
    }
}

/// Serializable so `laned` can hand an error back exactly as the engine made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub kind: ErrorKind,
    /// What went wrong, without a trailing full stop.
    pub message: String,
    /// What to do next, as a sentence.
    pub hint: Option<String>,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    /// Say what to do next. Every error a person or an agent can act on should have one.
    pub fn hint(mut self, hint: impl Into<String>) -> Error {
        self.hint = Some(hint.into());
        self
    }

    pub fn usage(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Usage, message)
    }

    pub fn not_implemented(command: &str) -> Error {
        Error::new(
            ErrorKind::NotImplemented,
            format!("`lane {command}` isn't built yet"),
        )
        .hint(format!("Follow progress at {}.", crate::PROGRESS_URL))
    }

    pub fn exit_code(&self) -> u8 {
        self.kind.exit_code()
    }

    /// The `--json` form: `{"error": {...}}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": {
                "code": self.kind.code(),
                "message": self.message,
                "hint": self.hint,
                "exit": self.exit_code(),
            }
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.hint {
            Some(hint) => write!(f, "{}. {hint}", self.message),
            None => write!(f, "{}.", self.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<crate::config::ConfigError> for Error {
    fn from(e: crate::config::ConfigError) -> Error {
        // ConfigError's text already says what to do next, as one sentence pair.
        Error::new(ErrorKind::Config, e.to_string().trim_end_matches('.'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_the_stable_exit_codes() {
        assert_eq!(ErrorKind::Usage.exit_code(), exit::USAGE);
        assert_eq!(ErrorKind::NotImplemented.exit_code(), exit::NOT_IMPLEMENTED);
        for kind in [
            ErrorKind::NotARepo,
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::Config,
            ErrorKind::State,
            ErrorKind::Io,
            ErrorKind::Failure,
        ] {
            assert_eq!(kind.exit_code(), exit::FAILURE, "{kind:?}");
        }
    }

    #[test]
    fn codes_are_unique_snake_case() {
        let kinds = [
            ErrorKind::Usage,
            ErrorKind::NotImplemented,
            ErrorKind::NotARepo,
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::Config,
            ErrorKind::State,
            ErrorKind::Io,
            ErrorKind::Failure,
        ];
        let mut codes: Vec<_> = kinds.iter().map(|k| k.code()).collect();
        assert!(
            codes
                .iter()
                .all(|c| c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'))
        );
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), kinds.len());
        // serde's name matches code(), so the two can never drift.
        for k in kinds {
            assert_eq!(serde_json::to_value(k).unwrap(), k.code());
        }
    }

    #[test]
    fn text_says_what_to_do_next() {
        let e = Error::new(ErrorKind::NotFound, "no lane called fix-auth")
            .hint("Run `lane list` to see your lanes.");
        assert_eq!(
            e.to_string(),
            "no lane called fix-auth. Run `lane list` to see your lanes."
        );
        assert_eq!(
            Error::usage("missing a name").to_string(),
            "missing a name."
        );
    }

    #[test]
    fn json_has_the_contract_shape() {
        let v = Error::not_implemented("new").to_json();
        assert_eq!(v["error"]["code"], "not_implemented");
        assert_eq!(v["error"]["exit"], 3);
        assert!(v["error"]["message"].as_str().unwrap().contains("lane new"));
        assert!(v["error"]["hint"].as_str().unwrap().contains("github.com"));
        assert_eq!(v.as_object().unwrap().len(), 1, "nothing beside `error`");
    }
}
