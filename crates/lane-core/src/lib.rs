//! Shared core for the Lanes CLI (`lane`) and daemon (`laned`).

pub mod agents;
pub mod clone;
pub mod config;
pub mod daemon;
pub mod detect;
pub mod doctor;
pub mod engine;
pub mod env;
pub mod error;
pub mod git;
pub mod insight;
pub mod lane_env;
pub mod log;
pub mod machine;
pub mod next;
pub mod process;
pub mod project;
pub mod repo;
pub mod running;
pub mod snapshot;
pub mod state;
pub mod telemetry;
pub mod writes;

/// The Lanes version, shared by every binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where to follow progress while a command is still being built.
pub const PROGRESS_URL: &str = "https://github.com/Autter-dev/lanes";

/// Stable exit codes. Agents and scripts rely on these, so never renumber them.
pub mod exit {
    /// The command succeeded.
    pub const OK: u8 = 0;
    /// Something went wrong while running the command.
    pub const FAILURE: u8 = 1;
    /// The command line was wrong: unknown command, missing argument.
    pub const USAGE: u8 = 2;
    /// The command exists but isn't built yet.
    pub const NOT_IMPLEMENTED: u8 = 3;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }

    /// The release workflow takes a release (0.1.0) or a release candidate (0.1.0-rc.1), and the
    /// archive names, installers and npm's dist-tags are made for those, so catch anything else
    /// here rather than at the tag.
    #[test]
    fn version_is_a_release_or_a_release_candidate() {
        fn number(s: &str) -> bool {
            !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_digit())
                && (s == "0" || !s.starts_with('0'))
        }
        fn release(v: &str) -> bool {
            let (base, rc) = match v.split_once('-') {
                Some((base, pre)) => match pre.strip_prefix("rc.") {
                    Some(n) => (base, Some(n)),
                    None => return false,
                },
                None => (v, None),
            };
            let parts: Vec<&str> = base.split('.').collect();
            parts.len() == 3 && parts.iter().all(|p| number(p)) && rc.is_none_or(number)
        }
        for ok in ["0.0.9", "0.1.0", "0.1.0-rc.1", "1.10.0-rc.12"] {
            assert!(release(ok), "{ok}");
        }
        for bad in [
            "0.1",
            "0.1.0-rc1",
            "0.1.0-beta.1",
            "0.1.0-rc.",
            "01.0.0",
            "0.1.0+b",
            "v0.1.0",
        ] {
            assert!(!release(bad), "{bad}");
        }
        assert!(
            release(VERSION),
            "{VERSION} isn't a release like 0.1.0 or a release candidate like 0.1.0-rc.1"
        );
    }

    #[test]
    fn exit_codes_are_distinct() {
        let codes = [exit::OK, exit::FAILURE, exit::USAGE, exit::NOT_IMPLEMENTED];
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
