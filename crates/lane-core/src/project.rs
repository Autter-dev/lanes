//! `lane.toml`: a repo's Lanes settings, checked into the repo so teammates get the same setup.
//!
//! The file is optional: without one, every setting has a default. With one, `version` is
//! required, and unknown keys are refused so a typo never silently does nothing. A file written
//! by a newer Lanes (a higher `version`) asks for an update instead of being misread.
//!
//! ```toml
//! version = 1
//!
//! [lanes]
//! dir = "../acme.lanes"     # where lane folders go; same disk as the repo, so copies are instant
//! base = "main"             # the branch the clean `_base` lane follows
//!
//! [ports]
//! start = 4100              # the first port Lanes hands out
//! per_lane = 10             # each lane's block, never reused while the lane exists
//!
//! [setup]
//! install = ["pnpm install"]   # run once in `_base`, and in a lane when its lockfile changes
//!
//! [env]
//! template = ".env.example"    # each lane's `.env` starts from this
//!
//! [dev]
//! command = "pnpm dev"      # the dev server; Lanes sets PORT (and VITE_PORT) per lane
//!
//! [cleanup]
//! keep_failed = "3d"        # how long a failed lane is kept for debugging
//! keep_db = true            # keep a failed lane's database (hosted branches are dumped, then deleted)
//! bundle = true             # write a failure bundle for every failed lane
//! parked_cap = "10GB"       # disk for all kept failures together
//! ```

use crate::error::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The `lane.toml` format this Lanes reads and writes.
pub const PROJECT_VERSION: u32 = 1;

/// The file name, at the repo's top level.
pub const FILE_NAME: &str = "lane.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub version: u32,
    #[serde(default)]
    pub lanes: Lanes,
    #[serde(default)]
    pub ports: Ports,
    #[serde(default)]
    pub setup: Setup,
    #[serde(default)]
    pub env: Env,
    #[serde(default)]
    pub dev: Dev,
    #[serde(default)]
    pub cleanup: Cleanup,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Lanes {
    /// Where lane folders go, relative to the repo's top level. Default: `../<repo>.lanes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// The branch `_base` follows. Default: the remote's default branch, else `main`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ports {
    pub start: u16,
    pub per_lane: u16,
}

impl Default for Ports {
    fn default() -> Ports {
        Ports {
            start: 4100,
            per_lane: 10,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Setup {
    /// Commands that install dependencies, in order.
    pub install: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Env {
    /// The file each lane's `.env` starts from, relative to the repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dev {
    /// The command that starts the dev server, run with `lane exec <name> -- <command>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cleanup {
    pub keep_failed: String,
    pub keep_db: bool,
    pub bundle: bool,
    pub parked_cap: String,
}

impl Default for Cleanup {
    fn default() -> Cleanup {
        Cleanup {
            keep_failed: "3d".into(),
            keep_db: true,
            bundle: true,
            parked_cap: "10GB".into(),
        }
    }
}

impl Default for Project {
    fn default() -> Project {
        Project {
            version: PROJECT_VERSION,
            lanes: Lanes::default(),
            ports: Ports::default(),
            setup: Setup::default(),
            env: Env::default(),
            dev: Dev::default(),
            cleanup: Cleanup::default(),
        }
    }
}

impl Cleanup {
    pub fn keep_failed(&self) -> Result<Duration> {
        parse_duration(&self.keep_failed).map_err(|m| invalid("cleanup.keep_failed", &m))
    }
    pub fn parked_cap(&self) -> Result<u64> {
        parse_size(&self.parked_cap).map_err(|m| invalid("cleanup.parked_cap", &m))
    }
}

fn invalid(key: &str, message: &str) -> Error {
    Error::new(ErrorKind::Config, format!("`{key}` in lane.toml {message}"))
        .hint("Fix it in lane.toml, or remove the line to use the default.")
}

impl Project {
    /// Read `lane.toml` from a repo's top level, or the defaults when there's no file.
    pub fn load(repo_root: &Path) -> Result<Project> {
        let path = repo_root.join(FILE_NAME);
        match fs::read_to_string(&path) {
            Ok(text) => Project::parse(&text, &path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Project::default()),
            Err(e) => Err(Error::new(
                ErrorKind::Io,
                format!("couldn't read {}: {e}", path.display()),
            )
            .hint("Check that you can read the file.")),
        }
    }

    /// Parse the text of a `lane.toml`; `path` is only used in messages.
    pub fn parse(text: &str, path: &Path) -> Result<Project> {
        // Check the version before the rest, so a newer file asks for an update rather than
        // failing on keys this Lanes doesn't know.
        #[derive(Deserialize)]
        struct VersionOnly {
            version: Option<toml::Value>,
        }
        let shown = path.display();
        let probe: VersionOnly = toml::from_str(text).map_err(|e| {
            Error::new(
                ErrorKind::Config,
                format!("{shown} isn't valid TOML: {}", e.message()),
            )
            .hint("Fix the file, or delete it to use the defaults.")
        })?;
        match probe.version {
            None => {
                return Err(
                    Error::new(ErrorKind::Config, format!("{shown} has no `version`")).hint(
                        format!("Add `version = {PROJECT_VERSION}` as its first line."),
                    ),
                );
            }
            Some(toml::Value::Integer(v)) if v > PROJECT_VERSION as i64 => {
                return Err(Error::new(
                    ErrorKind::Config,
                    format!("{shown} is version {v}, from a newer Lanes"),
                )
                .hint(
                    "Update Lanes (see lanes.autter.dev/download), then run the command again.",
                ));
            }
            Some(toml::Value::Integer(v)) if v >= 1 => {}
            Some(other) => {
                return Err(Error::new(
                    ErrorKind::Config,
                    format!("`version` in {shown} must be a whole number, not {other}"),
                )
                .hint(format!("Set it to `version = {PROJECT_VERSION}`.")));
            }
        }
        let project: Project = toml::from_str(text).map_err(|e| {
            Error::new(
                ErrorKind::Config,
                format!("{shown}: {}", e.message().trim_end_matches('.')),
            )
            .hint("Fix or remove that line, then run the command again.")
        })?;
        if project.ports.per_lane == 0 {
            return Err(invalid("ports.per_lane", "must be at least 1"));
        }
        if project.ports.start < 1024 {
            return Err(invalid(
                "ports.start",
                "must be 1024 or higher, so lanes never need admin rights",
            ));
        }
        project.cleanup.keep_failed()?;
        project.cleanup.parked_cap()?;
        Ok(project)
    }

    /// The folder lanes go in for the repo at `repo_root`.
    pub fn lanes_dir(&self, repo_root: &Path) -> PathBuf {
        match &self.lanes.dir {
            Some(dir) => repo_root.join(dir),
            None => {
                let name = repo_root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "repo".into());
                repo_root.with_file_name(format!("{name}.lanes"))
            }
        }
    }

    /// The file as TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("lane.toml always serializes")
    }

    /// The `lane.toml` that `lane init` writes, with a comment on each setting.
    pub fn init_text(install: &[String], dev: Option<&str>, env_template: Option<&Path>) -> String {
        let q = |s: &str| toml::Value::String(s.to_string()).to_string();
        let mut out = String::from(
            "# Lanes settings for this repo. Check it in, so teammates get the same lanes.\n\
             # Every setting and its default: https://github.com/Autter-dev/lanes\n\n",
        );
        out.push_str(&format!("version = {PROJECT_VERSION}\n\n"));
        out.push_str(
            "[setup]\n# Run once in the clean `_base` lane, and again when a lockfile changes.\n",
        );
        let list = install.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("install = [{list}]\n"));
        if let Some(cmd) = dev {
            out.push_str("\n[dev]\n# Start it in a lane with `lane exec <name> -- <command>`; Lanes sets PORT for each lane.\n");
            out.push_str(&format!("command = {}\n", q(cmd)));
        }
        if let Some(t) = env_template {
            out.push_str(
                "\n[env]\n# Each lane's `.env` starts from this, with its own PORT and URL.\n",
            );
            out.push_str(&format!(
                "template = {}\n",
                q(&t.to_string_lossy().replace('\\', "/"))
            ));
        }
        out
    }
}

/// "90m", "12h", "3d" or "2w".
pub fn parse_duration(s: &str) -> std::result::Result<Duration, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let n: u64 = n
        .parse()
        .map_err(|_| format!("must look like \"3d\", not \"{s}\""))?;
    let secs = match unit.trim() {
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        _ => {
            return Err(format!(
                "must end in m, h, d or w (like \"3d\"), not \"{s}\""
            ));
        }
    };
    Ok(Duration::from_secs(n * secs))
}

/// "500MB", "10GB" or "1TB" (powers of 1000, as disks are sold).
pub fn parse_size(s: &str) -> std::result::Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let n: u64 = n
        .parse()
        .map_err(|_| format!("must look like \"10GB\", not \"{s}\""))?;
    let mult = match unit.trim().to_ascii_uppercase().as_str() {
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        _ => {
            return Err(format!(
                "must end in MB, GB or TB (like \"10GB\"), not \"{s}\""
            ));
        }
    };
    Ok(n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Project> {
        Project::parse(text, Path::new("lane.toml"))
    }

    #[test]
    fn no_file_means_defaults() {
        let dir = std::env::temp_dir().join(format!("lane-project-none-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(Project::load(&dir).unwrap(), Project::default());
    }

    #[test]
    fn a_full_file_parses() {
        let p = parse(
            r#"
version = 1
[lanes]
dir = "../acme.lanes"
base = "develop"
[ports]
start = 5000
per_lane = 20
[setup]
install = ["pnpm install", "pnpm build"]
[env]
template = ".env.example"
[dev]
command = "pnpm dev"
[cleanup]
keep_failed = "12h"
keep_db = false
bundle = true
parked_cap = "500MB"
"#,
        )
        .unwrap();
        assert_eq!(p.lanes.base.as_deref(), Some("develop"));
        assert_eq!(
            p.ports,
            Ports {
                start: 5000,
                per_lane: 20
            }
        );
        assert_eq!(p.setup.install.len(), 2);
        assert_eq!(p.dev.command.as_deref(), Some("pnpm dev"));
        assert_eq!(
            p.cleanup.keep_failed().unwrap(),
            Duration::from_secs(12 * 3600)
        );
        assert_eq!(p.cleanup.parked_cap().unwrap(), 500_000_000);
    }

    #[test]
    fn version_alone_gives_defaults_for_the_rest() {
        let p = parse("version = 1\n").unwrap();
        assert_eq!(p, Project::default());
        assert_eq!(
            p.cleanup.keep_failed().unwrap(),
            Duration::from_secs(3 * 86400)
        );
        assert_eq!(p.cleanup.parked_cap().unwrap(), 10_000_000_000);
    }

    #[test]
    fn a_missing_version_says_what_to_add() {
        let e = parse("[ports]\nstart = 5000\n").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Config);
        assert!(e.to_string().contains("version = 1"), "{e}");
    }

    #[test]
    fn a_newer_version_asks_for_an_update() {
        let e = parse("version = 2\n[lanes]\nsomething_new = true\n").unwrap_err();
        assert!(e.to_string().contains("newer Lanes"), "{e}");
        assert!(e.to_string().contains("Update Lanes"), "{e}");
    }

    #[test]
    fn typos_are_refused_by_name() {
        let e = parse("version = 1\n[ports]\nper_lanes = 10\n").unwrap_err();
        assert!(e.to_string().contains("per_lanes"), "{e}");
        let e = parse("version = 1\n[ui]\ncolor = true\n").unwrap_err();
        assert!(e.to_string().contains("ui"), "{e}");
    }

    #[test]
    fn bad_values_point_at_the_key() {
        let e = parse("version = 1\n[cleanup]\nkeep_failed = \"three days\"\n").unwrap_err();
        assert!(e.to_string().contains("cleanup.keep_failed"), "{e}");
        let e = parse("version = 1\n[ports]\nstart = 80\n").unwrap_err();
        assert!(e.to_string().contains("ports.start"), "{e}");
        let e = parse("version = \"one\"\n").unwrap_err();
        assert!(e.to_string().contains("whole number"), "{e}");
    }

    #[test]
    fn lanes_go_next_to_the_repo_by_default() {
        let root = Path::new("/work/acme");
        assert_eq!(
            Project::default().lanes_dir(root),
            Path::new("/work/acme.lanes")
        );
        let custom = parse("version = 1\n[lanes]\ndir = \"../lanes\"\n").unwrap();
        assert_eq!(custom.lanes_dir(root), Path::new("/work/acme/../lanes"));
    }

    #[test]
    fn writing_then_reading_round_trips() {
        let mut p = Project::default();
        p.setup.install = vec!["npm ci".into()];
        let back = parse(&p.to_toml()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn the_init_file_is_valid_and_says_what_each_part_does() {
        let text = Project::init_text(
            &[
                "pnpm install --frozen-lockfile".into(),
                "uv sync --frozen".into(),
            ],
            Some("pnpm dev"),
            Some(Path::new(".env.example")),
        );
        let p = parse(&text).unwrap();
        assert_eq!(
            p.setup.install,
            ["pnpm install --frozen-lockfile", "uv sync --frozen"]
        );
        assert_eq!(p.dev.command.as_deref(), Some("pnpm dev"));
        assert_eq!(p.env.template.as_deref(), Some(Path::new(".env.example")));
        assert!(text.contains("# Run once in the clean `_base` lane"));
        let bare = Project::init_text(&[], None, None);
        assert_eq!(parse(&bare).unwrap().setup.install, Vec::<String>::new());
        assert!(!bare.contains("[dev]") && !bare.contains("[env]"));
    }

    #[test]
    fn durations_and_sizes() {
        assert_eq!(parse_duration("90m").unwrap(), Duration::from_secs(5400));
        assert_eq!(
            parse_duration("2w").unwrap(),
            Duration::from_secs(14 * 86400)
        );
        assert!(parse_duration("3").is_err());
        assert!(parse_duration("d").is_err());
        assert_eq!(parse_size("1TB").unwrap(), 1_000_000_000_000);
        assert_eq!(parse_size("10gb").unwrap(), 10_000_000_000);
        assert!(parse_size("10").is_err());
    }
}
