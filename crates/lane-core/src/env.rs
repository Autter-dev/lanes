//! What the environment says about how Lanes may behave: may it open a full-screen app, is this
//! CI or an agent's session, has the user turned the install count off.
//!
//! Everything reads from an [`Env`] snapshot so it can be tested with fake variables.

use std::collections::HashMap;
use std::io::IsTerminal;

/// A snapshot of environment variables and terminal state.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: HashMap<String, String>,
    stdin_tty: bool,
    stdout_tty: bool,
}

/// Agents that set a recognisable variable in the shells they start.
const AGENT_VARS: [(&str, &str); 6] = [
    ("CLAUDECODE", "Claude Code"),
    ("CURSOR_AGENT", "Cursor"),
    ("CODEX_SANDBOX", "Codex"),
    ("GEMINI_CLI", "Gemini CLI"),
    ("OPENCODE", "OpenCode"),
    ("LANE_AGENT", "an agent"),
];

const CI_VARS: [&str; 5] = ["CI", "GITHUB_ACTIONS", "GITLAB_CI", "BUILDKITE", "TF_BUILD"];

impl Env {
    /// The real environment of this process.
    pub fn current() -> Env {
        Env {
            vars: std::env::vars().collect(),
            stdin_tty: std::io::stdin().is_terminal(),
            stdout_tty: std::io::stdout().is_terminal(),
        }
    }

    /// A fake environment, for tests.
    pub fn fake(vars: &[(&str, &str)], tty: bool) -> Env {
        Env {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            stdin_tty: tty,
            stdout_tty: tty,
        }
    }

    pub fn var(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    fn truthy(&self, key: &str) -> bool {
        self.var(key).is_some_and(|v| {
            !matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
    }

    pub fn is_ci(&self) -> bool {
        CI_VARS.iter().any(|k| self.truthy(k))
    }

    /// The agent whose session this is, if any.
    pub fn agent_session(&self) -> Option<&'static str> {
        AGENT_VARS
            .iter()
            .find(|(k, _)| self.var(k).is_some())
            .map(|(_, name)| *name)
    }

    /// Why the install count is off no matter what the settings say.
    pub fn telemetry_forced_off(&self) -> Option<&'static str> {
        if self.truthy("DO_NOT_TRACK") {
            Some("DO_NOT_TRACK is set")
        } else if self.var("LANE_TELEMETRY").is_some() && !self.truthy("LANE_TELEMETRY") {
            Some("LANE_TELEMETRY is off")
        } else if self.is_ci() {
            Some("this is CI")
        } else {
            None
        }
    }

    pub fn no_color(&self) -> bool {
        self.var("NO_COLOR").is_some()
    }

    pub fn no_animation(&self) -> bool {
        self.truthy("LANE_NO_ANIM")
    }

    pub fn truecolor(&self) -> bool {
        matches!(self.var("COLORTERM"), Some("truecolor" | "24bit"))
            || self.var("WT_SESSION").is_some()
    }

    /// `Ok` when a full-screen app may open; otherwise why not, in words for a plain-text notice.
    pub fn interactive(&self) -> Result<(), &'static str> {
        if !self.stdin_tty || !self.stdout_tty {
            return Err("output isn't a terminal");
        }
        if self.var("TERM") == Some("dumb") {
            return Err("TERM is dumb");
        }
        if self.is_ci() {
            return Err("this is CI");
        }
        if self.agent_session().is_some() {
            return Err("this is an agent's session");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_terminal_is_interactive() {
        assert!(
            Env::fake(&[("TERM", "xterm-256color")], true)
                .interactive()
                .is_ok()
        );
    }

    #[test]
    fn pipes_ci_dumb_terminals_and_agents_never_get_the_tui() {
        assert!(Env::fake(&[], false).interactive().is_err());
        assert!(Env::fake(&[("CI", "true")], true).interactive().is_err());
        assert!(Env::fake(&[("TERM", "dumb")], true).interactive().is_err());
        assert_eq!(
            Env::fake(&[("CLAUDECODE", "1")], true).interactive(),
            Err("this is an agent's session")
        );
    }

    #[test]
    fn ci_false_is_not_ci() {
        assert!(!Env::fake(&[("CI", "false")], true).is_ci());
        assert!(!Env::fake(&[("CI", "")], true).is_ci());
    }

    #[test]
    fn telemetry_opt_outs_win() {
        assert_eq!(
            Env::fake(&[("DO_NOT_TRACK", "1")], true).telemetry_forced_off(),
            Some("DO_NOT_TRACK is set")
        );
        assert_eq!(
            Env::fake(&[("LANE_TELEMETRY", "0")], true).telemetry_forced_off(),
            Some("LANE_TELEMETRY is off")
        );
        assert_eq!(
            Env::fake(&[("LANE_TELEMETRY", "1")], true).telemetry_forced_off(),
            None
        );
        assert_eq!(Env::fake(&[], true).telemetry_forced_off(), None);
    }
}
