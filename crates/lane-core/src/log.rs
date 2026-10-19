//! Lanes' own log: what changed lanes, what agents' hooks did, and how dependency installs went,
//! one JSON object per line in `<state>/lane.log`. Nothing here is ever printed into an agent's
//! output; `lane logs` reads it back. Writing is best effort: a log that can't be written never
//! fails the command that wanted to write it.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The log moves to `lane.log.1` once it passes this size, so it never grows without bound.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Set while an agent's hook runs: output that would otherwise go to stderr (an install's, say)
/// goes to the log instead, because the agent would read it.
static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

/// Whether output meant for a person should go to the log instead (inside an agent's hook).
pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// One thing that happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// UTC, like `2026-10-17T19:20:01Z`.
    pub at: String,
    pub level: Level,
    /// What did it: `new`, `rm`, `hook claude worktree-create`, `install`, ...
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<PathBuf>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
}

impl Entry {
    pub fn new(level: Level, event: impl Into<String>, message: impl Into<String>) -> Entry {
        Entry {
            at: now(),
            level,
            event: event.into(),
            lane: None,
            repo: None,
            message: message.into(),
            ms: None,
        }
    }
    pub fn lane(mut self, lane: impl Into<String>) -> Entry {
        self.lane = Some(lane.into());
        self
    }
    pub fn repo(mut self, repo: &Path) -> Entry {
        self.repo = Some(repo.to_path_buf());
        self
    }
    pub fn ms(mut self, ms: u64) -> Entry {
        self.ms = Some(ms);
        self
    }
    /// Append it to the log. Best effort, never an error. Unit tests write nothing, so a test
    /// never touches the log of whoever runs it.
    pub fn write(self) {
        if cfg!(test) {
            return;
        }
        if let Ok(path) = path() {
            let _ = append(&path, &self);
        }
    }
}

pub fn info(event: impl Into<String>, message: impl Into<String>) -> Entry {
    Entry::new(Level::Info, event, message)
}
pub fn warn(event: impl Into<String>, message: impl Into<String>) -> Entry {
    Entry::new(Level::Warn, event, message)
}
pub fn error(event: impl Into<String>, message: impl Into<String>) -> Entry {
    Entry::new(Level::Error, event, message)
}

/// The log file: `<state>/lane.log`.
pub fn path() -> crate::error::Result<PathBuf> {
    Ok(crate::state::state_dir()?.join("lane.log"))
}

/// The log open for appending, for a child process's output (an install inside a hook).
pub fn file() -> Option<fs::File> {
    let path = path().ok()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok()?;
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn append(path: &Path, entry: &Entry) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    let line = serde_json::to_string(entry).expect("entries serialize") + "\n";
    // One write per line, in append mode, so lines from several processes don't interleave.
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// The last `limit` entries, oldest first, optionally only those about `lane`. Lines that aren't
/// entries (an install's own output) are skipped.
pub fn read(limit: usize, lane: Option<&str>) -> crate::error::Result<Vec<Entry>> {
    let path = path()?;
    let mut text = fs::read_to_string(path.with_extension("log.1")).unwrap_or_default();
    text.push_str(&fs::read_to_string(&path).unwrap_or_default());
    let mut entries: Vec<Entry> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
        .filter(|e| lane.is_none() || e.lane.as_deref() == lane)
        .collect();
    let skip = entries.len().saturating_sub(limit);
    entries.drain(..skip);
    Ok(entries)
}

/// Now, in UTC, as `YYYY-MM-DDTHH:MM:SSZ`.
fn now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    utc(secs)
}

/// Unix seconds as an RFC 3339 UTC time (days to a civil date, after Howard Hinnant).
pub fn utc(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_utc_rfc3339() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_792_257_600), "2026-10-17T17:20:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn entries_round_trip_and_filter_by_lane() {
        let dir = std::env::temp_dir().join(format!("lane-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("lane.log");
        append(
            &path,
            &info("new", "made the lane").lane("fix-auth").ms(240),
        )
        .unwrap();
        append(&path, &error("install", "`npm ci` failed").lane("other")).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"npm WARN deprecated something\n")
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let entries: Vec<Entry> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        assert_eq!(entries.len(), 2, "an install's own output isn't an entry");
        assert_eq!(entries[0].lane.as_deref(), Some("fix-auth"));
        assert_eq!(entries[0].ms, Some(240));
        assert_eq!(entries[1].level, Level::Error);
        let _ = fs::remove_dir_all(&dir);
    }
}
