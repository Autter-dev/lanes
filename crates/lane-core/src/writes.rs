//! The writes that change lanes: `lane new`, `lane rm`, `lane adopt`, and making an adopted lane
//! ready. They run in `laned`, the one writer, when it's there; otherwise in this process, under
//! the state store's lock as before. Either way the engine does the same work, and the result and
//! any error are the same.
//!
//! Through `laned`, install output comes back a line at a time, and installs run with the
//! caller's environment, not the daemon's. Once `laned` has a write, it finishes it even if the
//! caller goes away, so no lane is left half made.

use crate::daemon::{self, Client, Endpoint, WriteError};
use crate::engine::{self, Made, Output, Ready, Removed, Repo};
use crate::env::Env;
use crate::error::{Error, ErrorKind, Result};
use crate::state::{Lane, Store};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One change to the lanes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Write {
    New {
        repo: PathBuf,
        name: String,
        agent: Option<String>,
        request_id: Option<String>,
    },
    Remove {
        repo: PathBuf,
        name: String,
        keep_files: bool,
        request_id: Option<String>,
    },
    /// Make the worktree at `path` a lane.
    Adopt {
        repo: PathBuf,
        path: PathBuf,
        name: Option<String>,
        agent: Option<String>,
        request_id: Option<String>,
    },
    /// Make the lane at `path` ready to run: its `.env`, agent files and dependencies.
    Ready { repo: PathBuf, path: PathBuf },
}

impl Write {
    /// The repo it changes: `laned` makes one write at a time per repo.
    pub fn repo(&self) -> &Path {
        match self {
            Write::New { repo, .. }
            | Write::Remove { repo, .. }
            | Write::Adopt { repo, .. }
            | Write::Ready { repo, .. } => repo,
        }
    }
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "done", rename_all = "snake_case")]
pub enum Done {
    Made(Box<Made>),
    Removed(Removed),
    Adopted { lane: Lane },
    Ready(Ready),
}

impl Done {
    pub fn made(self) -> Result<Made> {
        match self {
            Done::Made(m) => Ok(*m),
            other => Err(mismatch(&other)),
        }
    }
    pub fn lane(self) -> Result<Lane> {
        match self {
            Done::Removed(Removed { lane, .. }) | Done::Adopted { lane } => Ok(lane),
            other => Err(mismatch(&other)),
        }
    }
    pub fn removed(self) -> Result<Removed> {
        match self {
            Done::Removed(r) => Ok(r),
            other => Err(mismatch(&other)),
        }
    }
    pub fn ready(self) -> Result<Ready> {
        match self {
            Done::Ready(r) => Ok(r),
            other => Err(mismatch(&other)),
        }
    }
}

fn mismatch(done: &Done) -> Error {
    Error::new(
        ErrorKind::Failure,
        format!("laned answered a different write ({done:?})"),
    )
    .hint("Run `lane daemon start` to replace it with this version's, then try again.")
}

/// Make the write in this process.
pub fn run(write: &Write) -> Result<Done> {
    let mut store = Store::open()?;
    match write {
        Write::New {
            repo,
            name,
            agent,
            request_id,
        } => {
            let repo = Repo::open(repo)?;
            let made = engine::new_lane(
                &mut store,
                &repo,
                name,
                agent.as_deref(),
                request_id.as_deref(),
            )?;
            Ok(Done::Made(Box::new(made)))
        }
        Write::Remove {
            repo,
            name,
            keep_files,
            request_id,
        } => {
            let repo = Repo::open(repo)?;
            let removed =
                engine::remove(&mut store, &repo, name, *keep_files, request_id.as_deref())?;
            Ok(Done::Removed(removed))
        }
        Write::Adopt {
            repo,
            path,
            name,
            agent,
            request_id,
        } => {
            let repo = Repo::open(repo)?;
            let lane = engine::adopt(
                &mut store,
                &repo,
                path,
                name.as_deref(),
                agent.as_deref(),
                request_id.as_deref(),
            )?;
            Ok(Done::Adopted { lane })
        }
        Write::Ready { repo, path } => {
            let repo = Repo::open(repo)?;
            let lane = store.lane_at(path)?.ok_or_else(|| {
                Error::new(
                    ErrorKind::NotFound,
                    format!("{} isn't a lane", path.display()),
                )
                .hint("Run `lane adopt` on it first.")
            })?;
            Ok(Done::Ready(engine::make_ready(&mut store, &repo, &lane)?))
        }
    }
}

/// Whether a write may go through `laned`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Through `laned`, starting it if it isn't running: a person's own commands.
    Daemon,
    /// Through `laned` only if it's running already: agents' hooks and scripts, which must stay
    /// quick and leave nothing behind.
    DaemonIfRunning,
    /// In this process.
    Here,
}

impl Via {
    pub fn for_env(env: &Env) -> Via {
        if daemon::may_start(env) {
            Via::Daemon
        } else {
            Via::DaemonIfRunning
        }
    }
}

/// Make `write` through `laned` when `via` allows and it answers, otherwise here. Install output
/// from `laned` reaches `output` a line at a time; here, it goes where it always has (stderr, or
/// the log inside an agent's hook).
pub fn make(write: &Write, via: Via, output: &mut dyn FnMut(Output, &str)) -> Result<Done> {
    if let Some(mut client) = connect(via) {
        match client.write(write, output) {
            Ok(result) => return result,
            // It never started: do it here.
            Err(WriteError::NotSent(_)) => {}
            Err(WriteError::Interrupted(e)) => {
                return Err(Error::new(
                    ErrorKind::Failure,
                    format!("laned stopped before it finished this ({e})"),
                )
                .hint("Run `lane list` to see where it got to, then try again. With `--request-id`, repeating it is safe."));
            }
        }
    }
    run(write)
}

fn connect(via: Via) -> Option<Client> {
    let at = Endpoint::current().ok()?;
    match via {
        Via::Here => None,
        Via::DaemonIfRunning => daemon::connect_if_running(&at),
        Via::Daemon => match daemon::laned_path() {
            Some(laned) => daemon::connect_or_start(&at, &laned).ok(),
            None => Client::connect(&at).ok(),
        },
    }
}

/// Where install output from `laned` goes for a command: stderr, so stdout stays clean, or the
/// log inside an agent's hook, where the agent reads stderr.
pub fn to_stderr_or_log(_: Output, line: &str) {
    if crate::log::quiet() {
        if let Some(mut file) = crate::log::file() {
            use std::io::Write as _;
            let _ = writeln!(file, "{line}");
        }
    } else {
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_results_cross_the_socket_unchanged() {
        let write = Write::New {
            repo: PathBuf::from("/work/app"),
            name: "fix-auth".into(),
            agent: Some("claude".into()),
            request_id: None,
        };
        let text = serde_json::to_string(&write).unwrap();
        assert!(text.starts_with(r#"{"kind":"new","repo":"#), "{text}");
        assert_eq!(serde_json::from_str::<Write>(&text).unwrap(), write);
        assert_eq!(write.repo(), Path::new("/work/app"));

        let error = Error::new(ErrorKind::NotFound, "no lane called `x` in this repo")
            .hint("Run `lane list` to see this repo's lanes.");
        let back: Error = serde_json::from_str(&serde_json::to_string(&error).unwrap()).unwrap();
        assert_eq!(back, error);
        assert_eq!(back.exit_code(), error.exit_code());
    }

    #[test]
    fn people_may_start_it_and_agents_only_use_it() {
        assert_eq!(Via::for_env(&Env::fake(&[], true)), Via::Daemon);
        assert_eq!(
            Via::for_env(&Env::fake(&[("CLAUDECODE", "1")], false)),
            Via::DaemonIfRunning
        );
        assert_eq!(
            Via::for_env(&Env::fake(&[("LANE_DAEMON", "0")], true)),
            Via::DaemonIfRunning
        );
    }
}
