//! What runs in each lane, so `lane rm` can stop it before the folder goes.
//!
//! `lane exec` starts its command with [`Runs::start`]: in a group of its own (see
//! [`crate::process`]), recorded in Lanes' state folder for as long as `lane exec` waits for it,
//! and known to the lane's actor in `laned` when `laned` is running already.
//!
//! A record is two files in `<state>/running/<lane id>/`: `<pid>.json` says what runs, and
//! `<pid>.lock` stays locked while `lane exec` is there. While it holds that lock, `lane exec`
//! holds the command too, so its process id can't be reused. A record whose lock is free was
//! left by a `lane exec` that didn't finish (killed, or the machine restarted): its process id
//! may belong to something else by now, so it's never signalled, only cleared away.

use crate::daemon::{self, Client, Endpoint};
use crate::process::{self, Group};
use crate::state::Lane;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

/// A command running in a lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    /// Its process id, which also names its group.
    pub pid: u32,
    /// The command, as given to `lane exec`.
    pub command: String,
    /// When it started, in Unix seconds.
    pub started: i64,
}

/// Where a state folder keeps what runs in its lanes.
#[derive(Debug, Clone)]
pub struct Runs {
    state_dir: PathBuf,
}

impl Runs {
    pub fn at(state_dir: &Path) -> Runs {
        Runs {
            state_dir: state_dir.to_path_buf(),
        }
    }

    /// Lanes' own state folder (`LANE_STATE_DIR`, or the usual one).
    pub fn current() -> crate::error::Result<Runs> {
        Ok(Runs::at(&crate::state::state_dir()?))
    }

    fn dir(&self, lane: &str) -> PathBuf {
        self.state_dir.join("running").join(lane)
    }

    /// Note that `running` runs in the lane with id `lane`, for as long as the record is kept.
    pub fn record(&self, lane: &str, running: &Running) -> io::Result<Record> {
        let dir = self.dir(lane);
        fs::create_dir_all(&dir)?;
        // The lock first: a record readers can see is always locked.
        let lock_path = dir.join(format!("{}.lock", running.pid));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock.try_lock().map_err(io::Error::from)?;
        let info = dir.join(format!("{}.json", running.pid));
        let tmp = info.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(running).map_err(io::Error::other)?)?;
        fs::rename(&tmp, &info)?;
        Ok(Record {
            lock: Some(lock),
            lock_path,
            info,
        })
    }

    /// What runs in the lane with id `lane` now, oldest first. Records left by a `lane exec`
    /// that's gone are cleared away.
    pub fn list(&self, lane: &str) -> Vec<Running> {
        let Ok(entries) = fs::read_dir(self.dir(lane)) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let lock = path.with_extension("lock");
            match held(&lock) {
                Some(true) => {
                    if let Some(running) = fs::read(&path)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Running>(&b).ok())
                    {
                        out.push(running);
                    }
                }
                Some(false) => {
                    let _ = fs::remove_file(&path);
                    let _ = fs::remove_file(&lock);
                }
                // Can't tell: leave it for next time.
                None => {}
            }
        }
        out.sort_by_key(|r| (r.started, r.pid));
        out
    }

    /// Stop what runs in the lane with id `lane`, and `also` (process ids `laned` was told of):
    /// asked to stop first, stopped outright after [`process::GRACE`]. Returns what was
    /// recorded as running.
    pub fn stop(&self, lane: &str, also: &[u32]) -> Vec<Running> {
        let running = self.list(lane);
        let mut pids: Vec<u32> = running
            .iter()
            .map(|r| r.pid)
            .chain(also.iter().copied())
            .collect();
        pids.sort_unstable();
        pids.dedup();
        if pids.is_empty() {
            return running;
        }
        let left = process::system().stop(&pids, process::GRACE);
        for pid in left {
            crate::log::warn(
                "rm",
                format!("process {pid} kept running after being stopped in lane {lane}"),
            )
            .write();
        }
        running
    }

    /// Start `cmd` in `lane` the way `lane exec` does: in a group of its own (with the terminal,
    /// when `foreground`), recorded so `lane rm` can stop it, and known to the lane's actor in
    /// `laned` when it's running. `laned` is never started for this: agents, hooks and CI run
    /// `lane exec` too. `command` is how it's shown.
    pub fn start(
        &self,
        lane: &Lane,
        cmd: &mut Command,
        command: &str,
        foreground: bool,
    ) -> io::Result<Exec> {
        let group = process::system().spawn(cmd, foreground)?;
        let running = Running {
            pid: group.id(),
            command: command.to_string(),
            started: unix_now(),
        };
        let record = match self.record(&lane.id, &running) {
            Ok(r) => Some(r),
            Err(e) => {
                crate::log::warn(
                    "exec",
                    format!("couldn't note that `{command}` runs in the lane ({e}); `lane rm` won't stop it"),
                )
                .lane(&lane.name)
                .write();
                None
            }
        };
        let laned = daemon::connect_if_running(&Endpoint::at(&self.state_dir))
            .and_then(|mut c| c.started(&lane.id, &running).ok().map(|()| c));
        Ok(Exec {
            record,
            group,
            laned,
            lane: lane.id.clone(),
            running,
        })
    }
}

/// Whether `lane exec` still holds a record's lock. None when that can't be told.
fn held(lock: &Path) -> Option<bool> {
    let file = match OpenOptions::new().read(true).write(true).open(lock) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    match file.try_lock() {
        Ok(()) => Some(false),
        Err(TryLockError::WouldBlock) => Some(true),
        Err(TryLockError::Error(_)) => None,
    }
}

/// A command noted as running in a lane. Dropping it removes the note.
pub struct Record {
    lock: Option<File>,
    lock_path: PathBuf,
    info: PathBuf,
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.info);
        drop(self.lock.take());
        let _ = fs::remove_file(&self.lock_path);
        // The lane's folder too, once nothing else runs there.
        if let Some(dir) = self.info.parent() {
            let _ = fs::remove_dir(dir);
        }
    }
}

/// A command `lane exec` started in a lane, until it ends.
pub struct Exec {
    // Dropped before the group: while the record is there, so is the group's handle.
    record: Option<Record>,
    group: Group,
    laned: Option<Client>,
    lane: String,
    running: Running,
}

impl Exec {
    /// The command's process id.
    pub fn pid(&self) -> u32 {
        self.running.pid
    }

    /// Wait for the command to end. Then it's no longer recorded, and `laned` hears it ended.
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        let status = process::system().wait(&mut self.group);
        drop(self.record.take());
        if let Some(c) = &mut self.laned {
            let _ = c.exited(&self.lane, self.running.pid);
        }
        status
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lane-running-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn running(pid: u32) -> Running {
        Running {
            pid,
            command: "npm run dev".into(),
            started: 1,
        }
    }

    #[test]
    fn a_record_lasts_as_long_as_it_is_kept() {
        let dir = state("record");
        let runs = Runs::at(&dir);
        assert!(runs.list("ln_a").is_empty());
        let record = runs.record("ln_a", &running(4242)).unwrap();
        assert_eq!(runs.list("ln_a"), [running(4242)]);
        assert!(runs.list("ln_b").is_empty(), "each lane has its own");
        drop(record);
        assert!(runs.list("ln_a").is_empty());
        assert!(
            !dir.join("running").join("ln_a").exists(),
            "nothing left behind"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_left_by_a_lane_exec_that_died_is_cleared_and_never_signalled() {
        let dir = state("stale");
        let runs = Runs::at(&dir);
        let lane = dir.join("running").join("ln_a");
        fs::create_dir_all(&lane).unwrap();
        // As a killed `lane exec` leaves it: the note, and a lock nobody holds. The process id
        // is ours, so stopping it would stop this test.
        let me = running(std::process::id());
        fs::write(
            lane.join(format!("{}.json", me.pid)),
            serde_json::to_vec(&me).unwrap(),
        )
        .unwrap();
        fs::write(lane.join(format!("{}.lock", me.pid)), "").unwrap();
        assert!(runs.list("ln_a").is_empty());
        assert!(runs.stop("ln_a", &[]).is_empty());
        assert!(!lane.join(format!("{}.json", me.pid)).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
