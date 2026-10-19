//! Lanes' own state: one SQLite database per user, for lanes, port blocks, the operation log and
//! request ids. It's per user rather than per repo because port blocks must not clash across
//! repos, and because `laned` is one process per user.
//!
//! - **One writer.** Every write takes an exclusive lock on `state.lock` next to the database,
//!   then an immediate transaction, so two `lane` processes (or `lane` and `laned`) never
//!   interleave a write. Reads take no lock: SQLite's WAL mode lets them run alongside a writer.
//! - **Request ids.** A write can carry a request id. The first time, its result is stored with
//!   the id; a retry with the same id returns that result and changes nothing, so an agent that
//!   retries after a timeout never makes a second lane. Ids are kept for 7 days.
//! - **Reads never create files.** `open_existing` returns `None` before the first write.
//!
//! `LANE_STATE_DIR` moves the folder (tests, CI, sandboxes).

use crate::error::{Error, ErrorKind, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The schema version in `PRAGMA user_version`. Bump only with a migration below.
pub const SCHEMA_VERSION: i64 = 4;

/// How long request ids are remembered.
const REQUEST_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// A lane still being created after this long was left behind by a crash.
const STALE_CREATING_SECS: i64 = 15 * 60;

/// The highest port a block may end on.
const MAX_PORT: u32 = 65_535;

/// The folder Lanes keeps its state in: `~/.local/state/lane`, or `%USERPROFILE%\.lane\state` on
/// Windows (see [`crate::config::windows_home`] for why not AppData).
pub fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("LANE_STATE_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if cfg!(windows) {
        return crate::config::windows_home()
            .map(|home| home.join("state"))
            .ok_or_else(|| {
                Error::new(ErrorKind::State, "couldn't find your home folder")
                    .hint("Set LANE_STATE_DIR to a folder you can write to.")
            });
    }
    use etcetera::BaseStrategy;
    let base = etcetera::choose_base_strategy().map_err(|_| {
        Error::new(ErrorKind::State, "couldn't find a folder for Lanes' state")
            .hint("Set LANE_STATE_DIR to a folder you can write to.")
    })?;
    Ok(base
        .state_dir()
        .unwrap_or_else(|| base.data_dir())
        .join("lane"))
}

/// The state database.
pub fn db_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("state.db"))
}

/// How a lane ended. `None` while it's still in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Promoted,
    Failed,
    Abandoned,
    NotChosen,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Promoted => "promoted",
            Outcome::Failed => "failed",
            Outcome::Abandoned => "abandoned",
            Outcome::NotChosen => "not_chosen",
        }
    }
    fn parse(s: &str) -> Option<Outcome> {
        Some(match s {
            "promoted" => Outcome::Promoted,
            "failed" => Outcome::Failed,
            "abandoned" => Outcome::Abandoned,
            "not_chosen" => Outcome::NotChosen,
            _ => return None,
        })
    }
}

/// Whether a lane is ready to use. A lane is `creating` while `lane new` copies its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Creating,
    Ready,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Creating => "creating",
            Status::Ready => "ready",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortBlock {
    pub start: u16,
    pub count: u16,
}

impl PortBlock {
    pub fn end(self) -> u32 {
        self.start as u32 + self.count as u32 - 1
    }
    fn overlaps(self, other: PortBlock) -> bool {
        (self.start as u32) <= other.end() && (other.start as u32) <= self.end()
    }
}

/// A lane as the state store knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub id: String,
    /// The repo's top-level folder.
    pub repo: PathBuf,
    pub name: String,
    /// The lane's folder.
    pub path: PathBuf,
    /// The commit the lane started from.
    pub base_commit: Option<String>,
    pub ports: PortBlock,
    /// The agent working in it, when known (`claude`, `codex`, …).
    pub agent: Option<String>,
    pub outcome: Option<Outcome>,
    pub status: Status,
    /// Unix seconds.
    pub created_at: i64,
}

impl Lane {
    /// Still `creating` long after any `lane new` would have finished: left by a crash, so the
    /// next `lane new` of that name takes it over.
    pub fn is_stale(&self) -> bool {
        self.status == Status::Creating && now() - self.created_at > STALE_CREATING_SECS
    }

    /// One word for where the lane stands: `creating`, `ready`, or how it ended.
    pub fn state(&self) -> &'static str {
        match (self.status, self.outcome) {
            (Status::Creating, _) => Status::Creating.as_str(),
            (Status::Ready, None) => Status::Ready.as_str(),
            (Status::Ready, Some(o)) => o.as_str(),
        }
    }
}

/// A repo's clean `_base` lane: where it is, the commit it's at, and what its dependencies
/// were installed from (so they're installed again only when that changes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    pub path: PathBuf,
    pub commit: String,
    pub install_key: Option<String>,
    pub updated_at: i64,
}

/// What a lane cost to make, and what it saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneStats {
    /// `clone` (copy-on-write: shares disk until files change) or `copy`.
    pub method: String,
    pub files: u64,
    /// The size of the lane's tree when it was made: what a separate checkout with its own
    /// install would take.
    pub bytes: u64,
    pub made_ms: u64,
    /// The dependency install this lane didn't have to run, when one was timed.
    pub install_ms: Option<u64>,
}

/// A lane's [`LaneStats`] with when it was made and, once removed, when and what it took by then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneRecord {
    pub lane_id: String,
    pub stats: LaneStats,
    /// When the lane was made (Unix seconds).
    pub at: i64,
    pub removed_at: Option<i64>,
    /// The disk the lane took when it was removed: the files a clone changed, all of a copy.
    /// `None` while it's still here, or when its folder had gone before `lane rm`.
    pub extra_bytes: Option<u64>,
}

/// What a new lane needs; the store adds its id and time.
#[derive(Debug, Clone)]
pub struct NewLane<'a> {
    pub repo: &'a Path,
    pub name: &'a str,
    pub path: &'a Path,
    pub base_commit: Option<&'a str>,
    pub ports: PortBlock,
    pub agent: Option<&'a str>,
    /// `Creating` for `lane new` (finished with [`Tx::finish_lane`]); `Ready` for `lane adopt`.
    pub status: Status,
}

/// A write that's safe to retry: the same `id` with the same `command` runs once.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub id: &'a str,
    pub command: &'a str,
}

/// Lane names become folder names and `<name>.localhost` URLs, so they're kept to what both
/// allow: lowercase letters, digits and hyphens, 1 to 40 long, starting with a letter or digit.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(Error::usage(format!("`{name}` can't be a lane name")).hint(
            "Use lowercase letters, digits and hyphens, up to 40 characters, like `fix-auth`.",
        ))
    }
}

fn db_err(e: rusqlite::Error) -> Error {
    Error::new(ErrorKind::State, format!("Lanes' state store failed: {e}"))
        .hint("Run the command again. If it keeps failing, open an issue at https://github.com/Autter-dev/lanes/issues.")
}

fn io_err(path: &Path, e: std::io::Error) -> Error {
    Error::new(
        ErrorKind::State,
        format!("couldn't use {}: {e}", path.display()),
    )
    .hint("Check that you can write to that folder, or set LANE_STATE_DIR to one you can.")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const SCHEMA: &str = "
CREATE TABLE repos (
  id INTEGER PRIMARY KEY,
  root TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL
);
CREATE TABLE lanes (
  id TEXT PRIMARY KEY,
  repo_id INTEGER NOT NULL REFERENCES repos(id),
  name TEXT NOT NULL,
  path TEXT NOT NULL,
  base_commit TEXT,
  port_start INTEGER NOT NULL,
  port_count INTEGER NOT NULL,
  agent TEXT,
  outcome TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (repo_id, name)
);
CREATE TABLE operations (
  id INTEGER PRIMARY KEY,
  lane_id TEXT,
  kind TEXT NOT NULL,
  detail TEXT NOT NULL,
  at INTEGER NOT NULL
);
CREATE INDEX operations_by_lane ON operations (lane_id, id);
CREATE TABLE requests (
  id TEXT PRIMARY KEY,
  command TEXT NOT NULL,
  result TEXT NOT NULL,
  at INTEGER NOT NULL
);
";

const MIGRATE_2: &str = "
ALTER TABLE lanes ADD COLUMN status TEXT NOT NULL DEFAULT 'ready';
CREATE TABLE bases (
  repo_id INTEGER PRIMARY KEY REFERENCES repos(id),
  path TEXT NOT NULL,
  base_commit TEXT NOT NULL,
  install_key TEXT,
  updated_at INTEGER NOT NULL
);
";

// What each lane cost to make, and what it saved: how big its tree is, whether it was cloned or
// copied, how long it took, and the install it didn't have to run. `bases.install_ms` is how long
// the last install in `_base` took, which every lane after it skips.
const MIGRATE_3: &str = "
CREATE TABLE lane_stats (
  lane_id TEXT PRIMARY KEY,
  method TEXT NOT NULL,
  files INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  made_ms INTEGER NOT NULL,
  install_ms INTEGER,
  at INTEGER NOT NULL
);
ALTER TABLE bases ADD COLUMN install_ms INTEGER;
";

// What lanes saved outlives them: `lane rm` keeps a lane's record, marked removed, with the disk
// the lane took by then (`extra_bytes`: the files a clone changed, all of a copy; NULL when its
// folder had already gone), so the totals since the first lane still add up.
const MIGRATE_4: &str = "
ALTER TABLE lane_stats ADD COLUMN repo_id INTEGER;
ALTER TABLE lane_stats ADD COLUMN removed_at INTEGER;
ALTER TABLE lane_stats ADD COLUMN extra_bytes INTEGER;
UPDATE lane_stats SET repo_id = (SELECT repo_id FROM lanes WHERE lanes.id = lane_stats.lane_id);
";

const LANE_COLUMNS: &str =
    "l.id, r.root, l.name, l.path, l.base_commit, l.port_start, l.port_count,
     l.agent, l.outcome, l.created_at, l.status";

pub struct Store {
    conn: Connection,
    lock_path: PathBuf,
}

impl Store {
    /// Open the state store, creating it on first use.
    pub fn open() -> Result<Store> {
        Store::open_at(&db_path()?)
    }

    /// Open the state store if it exists. `None` before anything was ever written.
    pub fn open_existing() -> Result<Option<Store>> {
        Store::open_existing_at(&db_path()?)
    }

    pub fn open_existing_at(path: &Path) -> Result<Option<Store>> {
        if path.exists() {
            Store::open_at(path).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn open_at(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))
            .map_err(db_err)?;
        // Switching to WAL takes a lock SQLite doesn't wait for, so several processes opening a
        // brand-new store at once (agents asking for lanes on a new machine) could be told
        // "database is locked". The mode is kept in the file: only the first open switches it,
        // under Lanes' own write lock.
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .map_err(db_err)?;
        if !mode.eq_ignore_ascii_case("wal") {
            let _lock = WriteLock::acquire(&path.with_extension("lock"))?;
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(db_err)?;
        }
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(db_err)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(db_err)?;
        let mut store = Store {
            conn,
            lock_path: path.with_extension("lock"),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_err)?;
        if version > SCHEMA_VERSION {
            return Err(Error::new(
                ErrorKind::State,
                format!("Lanes' state was written by a newer Lanes (schema {version})"),
            )
            .hint("Update Lanes (see lanes.autter.dev/download), then run the command again."));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        let _lock = WriteLock::acquire(&self.lock_path)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        // Another process may have migrated while we waited for the lock.
        let version: i64 = tx
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_err)?;
        if version < 1 {
            tx.execute_batch(SCHEMA).map_err(db_err)?;
        }
        if version < 2 {
            tx.execute_batch(MIGRATE_2).map_err(db_err)?;
        }
        if version < 3 {
            tx.execute_batch(MIGRATE_3).map_err(db_err)?;
        }
        if version < 4 {
            tx.execute_batch(MIGRATE_4).map_err(db_err)?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    /// Every lane, or only the lanes of one repo, oldest first.
    pub fn lanes(&self, repo: Option<&Path>) -> Result<Vec<Lane>> {
        let sql = format!(
            "SELECT {LANE_COLUMNS} FROM lanes l JOIN repos r ON r.id = l.repo_id
             WHERE ?1 IS NULL OR r.root = ?1
             ORDER BY l.created_at, l.name"
        );
        let repo = repo.map(|p| p.to_string_lossy().into_owned());
        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt.query_map([repo], row_to_lane).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    }

    /// One lane of a repo, by name.
    pub fn lane(&self, repo: &Path, name: &str) -> Result<Option<Lane>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {LANE_COLUMNS} FROM lanes l JOIN repos r ON r.id = l.repo_id
                     WHERE r.root = ?1 AND l.name = ?2"
                ),
                params![repo.to_string_lossy(), name],
                row_to_lane,
            )
            .optional()
            .map_err(db_err)
    }

    /// The lane whose folder is `path`, in any repo.
    pub fn lane_at(&self, path: &Path) -> Result<Option<Lane>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {LANE_COLUMNS} FROM lanes l JOIN repos r ON r.id = l.repo_id
                     WHERE l.path = ?1"
                ),
                [path.to_string_lossy()],
                row_to_lane,
            )
            .optional()
            .map_err(db_err)
    }

    /// A repo's `_base`, if it has one.
    pub fn base(&self, repo: &Path) -> Result<Option<Base>> {
        self.conn
            .query_row(
                "SELECT b.path, b.base_commit, b.install_key, b.updated_at
                 FROM bases b JOIN repos r ON r.id = b.repo_id WHERE r.root = ?1",
                [repo.to_string_lossy()],
                |r| {
                    Ok(Base {
                        path: PathBuf::from(r.get::<_, String>(0)?),
                        commit: r.get(1)?,
                        install_key: r.get(2)?,
                        updated_at: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)
    }

    /// What each lane cost and saved, by lane id, for one repo's lanes or every lane.
    pub fn lane_stats(&self, repo: Option<&Path>) -> Result<HashMap<String, LaneStats>> {
        let sql = "SELECT s.lane_id, s.method, s.files, s.bytes, s.made_ms, s.install_ms
                   FROM lane_stats s JOIN lanes l ON l.id = s.lane_id JOIN repos r ON r.id = l.repo_id
                   WHERE ?1 IS NULL OR r.root = ?1";
        let mut stmt = self.conn.prepare(sql).map_err(db_err)?;
        let rows = stmt
            .query_map([repo.map(|p| p.to_string_lossy().into_owned())], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    LaneStats {
                        method: r.get(1)?,
                        files: r.get::<_, i64>(2)? as u64,
                        bytes: r.get::<_, i64>(3)? as u64,
                        made_ms: r.get::<_, i64>(4)? as u64,
                        install_ms: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                    },
                ))
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()
            .map_err(db_err)
    }

    /// Every lane's record of what it cost and saved, removed lanes included, for one repo or
    /// every repo, oldest first.
    pub fn lane_records(&self, repo: Option<&Path>) -> Result<Vec<LaneRecord>> {
        let sql = "SELECT s.lane_id, s.method, s.files, s.bytes, s.made_ms, s.install_ms, s.at,
                          s.removed_at, s.extra_bytes
                   FROM lane_stats s LEFT JOIN repos r ON r.id = s.repo_id
                   WHERE ?1 IS NULL OR r.root = ?1
                   ORDER BY s.at, s.lane_id";
        let mut stmt = self.conn.prepare(sql).map_err(db_err)?;
        let rows = stmt
            .query_map([repo.map(|p| p.to_string_lossy().into_owned())], |r| {
                Ok(LaneRecord {
                    lane_id: r.get(0)?,
                    stats: LaneStats {
                        method: r.get(1)?,
                        files: r.get::<_, i64>(2)? as u64,
                        bytes: r.get::<_, i64>(3)? as u64,
                        made_ms: r.get::<_, i64>(4)? as u64,
                        install_ms: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                    },
                    at: r.get(6)?,
                    removed_at: r.get(7)?,
                    extra_bytes: r.get::<_, Option<i64>>(8)?.map(|v| v as u64),
                })
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    }

    /// How long the last dependency install in a repo's `_base` took, if Lanes timed one.
    pub fn base_install_ms(&self, repo: &Path) -> Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT b.install_ms FROM bases b JOIN repos r ON r.id = b.repo_id WHERE r.root = ?1",
                [repo.to_string_lossy()],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|v| v.flatten().map(|v| v as u64))
            .map_err(db_err)
    }

    /// A number that changes whenever any process writes the store, to notice changes cheaply.
    pub fn version(&self) -> Result<i64> {
        self.conn
            .pragma_query_value(None, "data_version", |r| r.get(0))
            .map_err(db_err)
    }

    /// The stored result of a request that already ran, without taking the write lock. Lets a
    /// long command answer a retry before doing any work.
    pub fn request_result<T: DeserializeOwned>(&self, request: Request<'_>) -> Result<Option<T>> {
        let seen: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT command, result FROM requests WHERE id = ?1",
                [request.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db_err)?;
        match seen {
            None => Ok(None),
            Some((command, _)) if command != request.command => Err(Error::new(
                ErrorKind::Conflict,
                format!(
                    "request id `{}` was already used for `{command}`",
                    request.id
                ),
            )
            .hint("Use a new request id for a different command.")),
            Some((_, result)) => serde_json::from_str(&result).map(Some).map_err(|e| {
                Error::new(
                    ErrorKind::State,
                    format!("couldn't read the stored result: {e}"),
                )
                .hint("Run the command again with a new request id.")
            }),
        }
    }

    /// Run `f` as the only writer, in one transaction. With a request, a retry of the same
    /// request returns the first result and runs nothing.
    pub fn write<T, F>(&mut self, request: Option<Request<'_>>, f: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce(&Tx<'_>) -> Result<T>,
    {
        let _lock = WriteLock::acquire(&self.lock_path)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        if let Some(req) = request {
            let seen: Option<(String, String)> = tx
                .query_row(
                    "SELECT command, result FROM requests WHERE id = ?1",
                    [req.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db_err)?;
            if let Some((command, result)) = seen {
                if command != req.command {
                    return Err(Error::new(
                        ErrorKind::Conflict,
                        format!("request id `{}` was already used for `{command}`", req.id),
                    )
                    .hint("Use a new request id for a different command."));
                }
                return serde_json::from_str(&result).map_err(|e| {
                    Error::new(
                        ErrorKind::State,
                        format!("couldn't read the stored result: {e}"),
                    )
                    .hint("Run the command again with a new request id.")
                });
            }
        }
        let out = f(&Tx { tx: &tx })?;
        if let Some(req) = request {
            let result = serde_json::to_string(&out).expect("results always serialize");
            tx.execute(
                "INSERT INTO requests (id, command, result, at) VALUES (?1, ?2, ?3, ?4)",
                params![req.id, req.command, result, now()],
            )
            .map_err(db_err)?;
        }
        tx.execute(
            "DELETE FROM requests WHERE at < ?1",
            [now() - REQUEST_TTL_SECS],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(out)
    }

    /// Bring across what an older store (`old`) has and this one doesn't: its lanes, with their
    /// history and what they cost and saved, its repos' `_base`, and its request ids. With
    /// `since` (Unix seconds), only lanes made at or after it: the rest were looked at last time,
    /// and a lane removed here since then mustn't come back.
    ///
    /// A lane already here (the same id) is left as it is. One whose folder has gone, or whose
    /// name a lane of its repo here has, stays out. One whose ports a lane here has gets the next
    /// free block. Safe to repeat: the same `old` again changes nothing. `from` names the old
    /// store in the operation log.
    pub fn take_in(&mut self, old: &Store, since: Option<i64>, from: &str) -> Result<TakenIn> {
        let since = since.unwrap_or(i64::MIN);
        let repos: HashMap<i64, (String, i64)> =
            old.rows("SELECT id, root, created_at FROM repos", [], |r| {
                Ok((r.get(0)?, (r.get(1)?, r.get(2)?)))
            })?;
        let lanes: Vec<(Lane, i64)> = old.rows(
            &format!(
                "SELECT {LANE_COLUMNS}, l.updated_at FROM lanes l JOIN repos r ON r.id = l.repo_id
                 WHERE l.created_at >= ?1 ORDER BY l.created_at, l.name"
            ),
            [since],
            |r| Ok((row_to_lane(r)?, r.get(11)?)),
        )?;
        let operations: Vec<(String, String, String, i64)> = old.rows(
            "SELECT lane_id, kind, detail, at FROM operations WHERE lane_id IS NOT NULL ORDER BY id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let mut history: HashMap<String, Vec<(String, String, i64)>> = HashMap::new();
        for (lane, kind, detail, at) in operations {
            history.entry(lane).or_default().push((kind, detail, at));
        }
        let records: Vec<(LaneRecord, Option<String>)> = old.rows(
            "SELECT s.lane_id, s.method, s.files, s.bytes, s.made_ms, s.install_ms, s.at,
                    s.removed_at, s.extra_bytes, r.root
             FROM lane_stats s LEFT JOIN repos r ON r.id = s.repo_id
             WHERE s.at >= ?1 ORDER BY s.at",
            [since],
            |r| {
                Ok((
                    LaneRecord {
                        lane_id: r.get(0)?,
                        stats: LaneStats {
                            method: r.get(1)?,
                            files: r.get::<_, i64>(2)? as u64,
                            bytes: r.get::<_, i64>(3)? as u64,
                            made_ms: r.get::<_, i64>(4)? as u64,
                            install_ms: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                        },
                        at: r.get(6)?,
                        removed_at: r.get(7)?,
                        extra_bytes: r.get::<_, Option<i64>>(8)?.map(|v| v as u64),
                    },
                    r.get(9)?,
                ))
            },
        )?;
        let bases: Vec<(String, Base, Option<i64>)> = old.rows(
            "SELECT r.root, b.path, b.base_commit, b.install_key, b.updated_at, b.install_ms
             FROM bases b JOIN repos r ON r.id = b.repo_id",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    Base {
                        path: PathBuf::from(r.get::<_, String>(1)?),
                        commit: r.get(2)?,
                        install_key: r.get(3)?,
                        updated_at: r.get(4)?,
                    },
                    r.get(5)?,
                ))
            },
        )?;
        let requests: Vec<(String, String, String, i64)> =
            old.rows("SELECT id, command, result, at FROM requests", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;

        self.write(None, |tx| {
            let t = tx.tx;
            let repo_id = |root: &str| -> Result<i64> {
                let created = repos
                    .values()
                    .find(|(r, _)| r == root)
                    .map_or_else(now, |(_, at)| *at);
                t.execute(
                    "INSERT INTO repos (root, created_at) VALUES (?1, ?2) ON CONFLICT (root) DO NOTHING",
                    params![root, created],
                )
                .map_err(db_err)?;
                t.query_row("SELECT id FROM repos WHERE root = ?1", [root], |r| r.get(0))
                    .map_err(db_err)
            };
            let exists = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<bool> {
                t.query_row(&format!("SELECT EXISTS ({sql})"), p, |r| r.get(0))
                    .map_err(db_err)
            };
            let mut out = TakenIn::default();
            for (mut lane, updated_at) in lanes {
                if exists("SELECT 1 FROM lanes WHERE id = ?1", &[&lane.id])? {
                    continue;
                }
                if !lane.path.exists() {
                    out.left_out.push((lane, "its folder has gone".into()));
                    continue;
                }
                let root = lane.repo.to_string_lossy().into_owned();
                let repo = repo_id(&root)?;
                if exists(
                    "SELECT 1 FROM lanes WHERE repo_id = ?1 AND name = ?2",
                    &[&repo, &lane.name],
                )? {
                    out.left_out
                        .push((lane, "a lane of its repo here has its name".into()));
                    continue;
                }
                let was = lane.ports;
                if exists(
                    "SELECT 1 FROM lanes WHERE port_start <= ?2 AND port_start + port_count - 1 >= ?1",
                    &[&was.start, &was.end()],
                )? {
                    match tx.allocate_ports(was.start, was.count) {
                        Ok(block) => {
                            lane.ports = block;
                            out.new_ports.push((lane.clone(), was));
                        }
                        Err(_) => {
                            out.left_out
                                .push((lane, "there are no free ports for it".into()));
                            continue;
                        }
                    }
                }
                t.execute(
                    "INSERT INTO lanes (id, repo_id, name, path, base_commit, port_start, port_count,
                                        agent, outcome, created_at, updated_at, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        lane.id,
                        repo,
                        lane.name,
                        lane.path.to_string_lossy(),
                        lane.base_commit,
                        lane.ports.start,
                        lane.ports.count,
                        lane.agent,
                        lane.outcome.map(Outcome::as_str),
                        lane.created_at,
                        updated_at,
                        lane.status.as_str(),
                    ],
                )
                .map_err(db_err)?;
                for (kind, detail, at) in history.remove(&lane.id).unwrap_or_default() {
                    t.execute(
                        "INSERT INTO operations (lane_id, kind, detail, at) VALUES (?1, ?2, ?3, ?4)",
                        params![lane.id, kind, detail, at],
                    )
                    .map_err(db_err)?;
                }
                tx.log(
                    Some(&lane.id),
                    "moved",
                    &serde_json::json!({ "name": lane.name, "from": from }),
                )?;
                out.lanes.push(lane);
            }
            // What lanes saved outlives them: a record comes across even for a lane that
            // didn't, marked removed then, so the totals still add up.
            for (record, root) in records {
                if exists(
                    "SELECT 1 FROM lane_stats WHERE lane_id = ?1",
                    &[&record.lane_id],
                )? {
                    continue;
                }
                let here = exists("SELECT 1 FROM lanes WHERE id = ?1", &[&record.lane_id])?;
                let removed_at = match here {
                    true => record.removed_at,
                    false => record.removed_at.or(Some(now())),
                };
                let repo = root.as_deref().map(repo_id).transpose()?;
                let s = &record.stats;
                t.execute(
                    "INSERT INTO lane_stats (lane_id, repo_id, method, files, bytes, made_ms,
                                             install_ms, at, removed_at, extra_bytes)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        record.lane_id,
                        repo,
                        s.method,
                        s.files as i64,
                        s.bytes as i64,
                        s.made_ms as i64,
                        s.install_ms.map(|v| v as i64),
                        record.at,
                        removed_at,
                        record.extra_bytes.map(|v| v as i64),
                    ],
                )
                .map_err(db_err)?;
            }
            // A repo has one `_base` folder: whichever store touched it last knows it best.
            for (root, base, install_ms) in bases {
                let repo = repo_id(&root)?;
                let here: Option<i64> = t
                    .query_row(
                        "SELECT updated_at FROM bases WHERE repo_id = ?1",
                        [repo],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(db_err)?;
                if here.is_some_and(|at| at >= base.updated_at) {
                    continue;
                }
                t.execute(
                    "INSERT INTO bases (repo_id, path, base_commit, install_key, updated_at, install_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (repo_id) DO UPDATE SET
                       path = excluded.path, base_commit = excluded.base_commit,
                       install_key = excluded.install_key, updated_at = excluded.updated_at,
                       install_ms = excluded.install_ms",
                    params![
                        repo,
                        base.path.to_string_lossy(),
                        base.commit,
                        base.install_key,
                        base.updated_at,
                        install_ms,
                    ],
                )
                .map_err(db_err)?;
            }
            // A retry of a write made before the move still gets its first answer.
            for (id, command, result, at) in requests {
                t.execute(
                    "INSERT INTO requests (id, command, result, at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (id) DO NOTHING",
                    params![id, command, result, at],
                )
                .map_err(db_err)?;
            }
            Ok(out)
        })
    }

    /// Every row of a query, collected.
    fn rows<C, P, F>(&self, sql: &str, params: P, f: F) -> Result<C>
    where
        C: FromIterator<<C as IntoIterator>::Item> + IntoIterator,
        P: rusqlite::Params,
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<<C as IntoIterator>::Item>,
    {
        let mut stmt = self.conn.prepare(sql).map_err(db_err)?;
        let rows = stmt.query_map(params, f).map_err(db_err)?;
        rows.collect::<rusqlite::Result<C>>().map_err(db_err)
    }
}

/// What [`Store::take_in`] brought across.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakenIn {
    /// The lanes that came across, as they are here now.
    pub lanes: Vec<Lane>,
    /// Those of them that got other ports, because a lane here had theirs, with the ports they
    /// had before.
    pub new_ports: Vec<(Lane, PortBlock)>,
    /// Lanes that stayed out, and why.
    pub left_out: Vec<(Lane, String)>,
}

fn row_to_lane(r: &rusqlite::Row<'_>) -> rusqlite::Result<Lane> {
    let outcome: Option<String> = r.get(8)?;
    Ok(Lane {
        id: r.get(0)?,
        repo: PathBuf::from(r.get::<_, String>(1)?),
        name: r.get(2)?,
        path: PathBuf::from(r.get::<_, String>(3)?),
        base_commit: r.get(4)?,
        ports: PortBlock {
            start: r.get(5)?,
            count: r.get(6)?,
        },
        agent: r.get(7)?,
        outcome: outcome.as_deref().and_then(Outcome::parse),
        created_at: r.get(9)?,
        status: match r.get::<_, String>(10)?.as_str() {
            "creating" => Status::Creating,
            _ => Status::Ready,
        },
    })
}

/// The open write transaction handed to [`Store::write`].
pub struct Tx<'a> {
    tx: &'a rusqlite::Transaction<'a>,
}

impl Tx<'_> {
    /// The first free block of `count` ports at or after `start`, across every repo's lanes.
    pub fn allocate_ports(&self, start: u16, count: u16) -> Result<PortBlock> {
        let mut stmt = self
            .tx
            .prepare("SELECT port_start, port_count FROM lanes ORDER BY port_start")
            .map_err(db_err)?;
        let used = stmt
            .query_map([], |r| {
                Ok(PortBlock {
                    start: r.get(0)?,
                    count: r.get(1)?,
                })
            })
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;
        let mut candidate = start as u32;
        loop {
            if candidate + count as u32 - 1 > MAX_PORT {
                return Err(Error::new(
                    ErrorKind::Conflict,
                    format!("no free block of {count} ports from {start} up"),
                )
                .hint("Remove lanes you're done with (`lane rm <name>`), or lower `ports.start` in lane.toml."));
            }
            let block = PortBlock {
                start: candidate as u16,
                count,
            };
            match used.iter().find(|u| u.overlaps(block)) {
                Some(u) => candidate = u.end() + 1,
                None => return Ok(block),
            }
        }
    }

    /// Record a new lane. The name must be free in its repo.
    pub fn insert_lane(&self, new: NewLane<'_>) -> Result<Lane> {
        check_name(new.name)?;
        let at = now();
        let root = new.repo.to_string_lossy();
        self.tx
            .execute(
                "INSERT INTO repos (root, created_at) VALUES (?1, ?2) ON CONFLICT (root) DO NOTHING",
                params![root, at],
            )
            .map_err(db_err)?;
        let repo_id: i64 = self
            .tx
            .query_row("SELECT id FROM repos WHERE root = ?1", [&root], |r| {
                r.get(0)
            })
            .map_err(db_err)?;
        // A lane left `creating` by a crash long ago doesn't hold its name forever.
        self.tx
            .execute(
                "DELETE FROM lanes WHERE repo_id = ?1 AND name = ?2 AND status = 'creating' AND updated_at < ?3",
                params![repo_id, new.name, at - STALE_CREATING_SECS],
            )
            .map_err(db_err)?;
        let existing: Option<String> = self
            .tx
            .query_row(
                "SELECT status FROM lanes WHERE repo_id = ?1 AND name = ?2",
                params![repo_id, new.name],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        if existing.as_deref() == Some("creating") {
            return Err(Error::new(
                ErrorKind::Conflict,
                format!("the lane `{}` is being created right now", new.name),
            )
            .hint("Wait for it to finish, or pick another name."));
        }
        if existing.is_some() {
            return Err(Error::new(
                ErrorKind::Conflict,
                format!("there's already a lane called `{}` in this repo", new.name),
            )
            .hint(format!(
                "Pick another name, or remove it first with `lane rm {}`.",
                new.name
            )));
        }
        let id: String = self
            .tx
            .query_row("SELECT 'ln_' || lower(hex(randomblob(5)))", [], |r| {
                r.get(0)
            })
            .map_err(db_err)?;
        self.tx
            .execute(
                "INSERT INTO lanes (id, repo_id, name, path, base_commit, port_start, port_count,
                                    agent, outcome, created_at, updated_at, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?9, ?10)",
                params![
                    id,
                    repo_id,
                    new.name,
                    new.path.to_string_lossy(),
                    new.base_commit,
                    new.ports.start,
                    new.ports.count,
                    new.agent,
                    at,
                    new.status.as_str()
                ],
            )
            .map_err(db_err)?;
        self.log(Some(&id), "new", &serde_json::json!({ "name": new.name }))?;
        Ok(Lane {
            id,
            repo: new.repo.to_path_buf(),
            name: new.name.to_string(),
            path: new.path.to_path_buf(),
            base_commit: new.base_commit.map(str::to_string),
            ports: new.ports,
            agent: new.agent.map(str::to_string),
            outcome: None,
            status: new.status,
            created_at: at,
        })
    }

    /// Mark a lane `creating` → `ready`, with the commit it started from.
    pub fn finish_lane(&self, lane_id: &str, base_commit: &str) -> Result<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE lanes SET status = 'ready', base_commit = ?2, updated_at = ?3 WHERE id = ?1",
                params![lane_id, base_commit, now()],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(
                Error::new(ErrorKind::NotFound, format!("no lane with id `{lane_id}`"))
                    .hint("Run `lane list` to see your lanes."),
            );
        }
        self.log(
            Some(lane_id),
            "ready",
            &serde_json::json!({ "base_commit": base_commit }),
        )
    }

    /// Record what a lane cost to make and what it saved.
    pub fn set_lane_stats(&self, lane_id: &str, stats: &LaneStats) -> Result<()> {
        self.tx
            .execute(
                "INSERT INTO lane_stats (lane_id, repo_id, method, files, bytes, made_ms, install_ms, at)
                 VALUES (?1, (SELECT repo_id FROM lanes WHERE id = ?1), ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (lane_id) DO UPDATE SET repo_id = excluded.repo_id,
                   method = excluded.method, files = excluded.files, bytes = excluded.bytes,
                   made_ms = excluded.made_ms, install_ms = excluded.install_ms, at = excluded.at",
                params![
                    lane_id,
                    stats.method,
                    stats.files as i64,
                    stats.bytes as i64,
                    stats.made_ms as i64,
                    stats.install_ms.map(|v| v as i64),
                    now()
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// How long the last install in a repo's `_base` took, read inside this write.
    pub fn base_install_ms(&self, repo: &Path) -> Option<u64> {
        self.tx
            .query_row(
                "SELECT b.install_ms FROM bases b JOIN repos r ON r.id = b.repo_id WHERE r.root = ?1",
                [repo.to_string_lossy()],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .map(|v| v as u64)
    }

    /// Record how long the dependency install in a repo's `_base` took.
    pub fn set_base_install_ms(&self, repo: &Path, ms: u64) -> Result<()> {
        self.tx
            .execute(
                "UPDATE bases SET install_ms = ?2 WHERE repo_id = (SELECT id FROM repos WHERE root = ?1)",
                params![repo.to_string_lossy(), ms as i64],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Forget a lane by id (a failed `lane new` cleaning up after itself).
    pub fn drop_lane(&self, lane_id: &str) -> Result<()> {
        self.tx
            .execute("DELETE FROM lanes WHERE id = ?1", [lane_id])
            .map_err(db_err)?;
        self.tx
            .execute("DELETE FROM lane_stats WHERE lane_id = ?1", [lane_id])
            .map_err(db_err)?;
        self.log(Some(lane_id), "dropped", &serde_json::json!({}))
    }

    /// Record where a repo's `_base` is and what it's at.
    pub fn set_base(
        &self,
        repo: &Path,
        path: &Path,
        commit: &str,
        install_key: Option<&str>,
    ) -> Result<()> {
        let at = now();
        let root = repo.to_string_lossy();
        self.tx
            .execute(
                "INSERT INTO repos (root, created_at) VALUES (?1, ?2) ON CONFLICT (root) DO NOTHING",
                params![root, at],
            )
            .map_err(db_err)?;
        self.tx
            .execute(
                "INSERT INTO bases (repo_id, path, base_commit, install_key, updated_at)
                 SELECT id, ?2, ?3, ?4, ?5 FROM repos WHERE root = ?1
                 ON CONFLICT (repo_id) DO UPDATE SET
                   path = excluded.path, base_commit = excluded.base_commit,
                   install_key = excluded.install_key, updated_at = excluded.updated_at",
                params![root, path.to_string_lossy(), commit, install_key, at],
            )
            .map_err(db_err)?;
        self.log(
            None,
            "base",
            &serde_json::json!({ "repo": root, "commit": commit }),
        )
    }

    /// Forget a lane and free its ports, keeping its record of what it saved. `extra_bytes` is
    /// the disk it took by then, when its folder was there to measure. Returns what was removed.
    pub fn remove_lane(&self, repo: &Path, name: &str, extra_bytes: Option<u64>) -> Result<Lane> {
        let root = repo.to_string_lossy();
        let lane = self
            .tx
            .query_row(
                &format!(
                    "SELECT {LANE_COLUMNS} FROM lanes l JOIN repos r ON r.id = l.repo_id
                     WHERE r.root = ?1 AND l.name = ?2"
                ),
                params![root, name],
                row_to_lane,
            )
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::NotFound,
                    format!("no lane called `{name}` in this repo"),
                )
                .hint("Run `lane list` to see this repo's lanes.")
            })?;
        self.tx
            .execute("DELETE FROM lanes WHERE id = ?1", [&lane.id])
            .map_err(db_err)?;
        self.tx
            .execute(
                "UPDATE lane_stats SET removed_at = ?2, extra_bytes = ?3 WHERE lane_id = ?1",
                params![lane.id, now(), extra_bytes.map(|v| v as i64)],
            )
            .map_err(db_err)?;
        self.log(Some(&lane.id), "rm", &serde_json::json!({ "name": name }))?;
        Ok(lane)
    }

    /// Set (or clear) how a lane ended.
    pub fn set_outcome(&self, lane_id: &str, outcome: Option<Outcome>) -> Result<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE lanes SET outcome = ?2, updated_at = ?3 WHERE id = ?1",
                params![lane_id, outcome.map(Outcome::as_str), now()],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(
                Error::new(ErrorKind::NotFound, format!("no lane with id `{lane_id}`"))
                    .hint("Run `lane list` to see your lanes."),
            );
        }
        self.log(
            Some(lane_id),
            "outcome",
            &serde_json::json!({ "outcome": outcome }),
        )
    }

    /// Add an entry to the operation log.
    pub fn log(&self, lane_id: Option<&str>, kind: &str, detail: &serde_json::Value) -> Result<()> {
        self.tx
            .execute(
                "INSERT INTO operations (lane_id, kind, detail, at) VALUES (?1, ?2, ?3, ?4)",
                params![lane_id, kind, detail.to_string(), now()],
            )
            .map(|_| ())
            .map_err(db_err)
    }
}

/// An exclusive lock on `state.lock`, held for one write and released when dropped.
struct WriteLock(#[allow(dead_code)] File);

impl WriteLock {
    fn acquire(path: &Path) -> Result<WriteLock> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| io_err(path, e))?;
        file.lock().map_err(|e| io_err(path, e))?;
        Ok(WriteLock(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lane-state-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("state.db")
    }

    fn new_lane<'a>(
        repo: &'a Path,
        name: &'a str,
        path: &'a Path,
        ports: PortBlock,
    ) -> NewLane<'a> {
        NewLane {
            repo,
            name,
            path,
            base_commit: Some("abc123"),
            ports,
            agent: Some("claude"),
            status: Status::Ready,
        }
    }

    #[test]
    fn opening_creates_the_schema_once() {
        let db = temp_db("schema");
        let store = Store::open_at(&db).unwrap();
        let version: i64 = store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let mode: String = store
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        drop(store);
        // Opening again is a no-op.
        assert!(Store::open_at(&db).unwrap().lanes(None).unwrap().is_empty());
    }

    #[test]
    fn reading_before_any_write_creates_nothing() {
        let db = temp_db("existing");
        assert!(Store::open_existing_at(&db).unwrap().is_none());
        assert!(!db.exists());
    }

    #[test]
    fn a_newer_schema_asks_for_an_update() {
        let db = temp_db("newer");
        drop(Store::open_at(&db).unwrap());
        Connection::open(&db)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let e = Store::open_at(&db).err().unwrap();
        assert!(e.to_string().contains("newer Lanes"), "{e}");
    }

    #[test]
    fn lanes_are_recorded_listed_and_removed() {
        let mut store = Store::open_at(&temp_db("crud")).unwrap();
        let (repo, other) = (Path::new("/work/acme"), Path::new("/work/other"));
        let a = store
            .write(None, |tx| {
                let ports = tx.allocate_ports(4100, 10)?;
                tx.insert_lane(new_lane(
                    repo,
                    "fix-auth",
                    Path::new("/work/acme.lanes/fix-auth"),
                    ports,
                ))
            })
            .unwrap();
        assert!(a.id.starts_with("ln_") && a.id.len() == 13, "{}", a.id);
        store
            .write(None, |tx| {
                let ports = tx.allocate_ports(4100, 10)?;
                tx.insert_lane(new_lane(
                    other,
                    "fix-auth",
                    Path::new("/work/other.lanes/fix-auth"),
                    ports,
                ))
            })
            .unwrap();
        assert_eq!(store.lanes(Some(repo)).unwrap().len(), 1);
        assert_eq!(store.lanes(None).unwrap().len(), 2);

        // Same name in the same repo is refused, with what to do instead.
        let e = store
            .write(None, |tx| {
                tx.insert_lane(new_lane(
                    repo,
                    "fix-auth",
                    Path::new("/x"),
                    PortBlock {
                        start: 9000,
                        count: 10,
                    },
                ))
            })
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Conflict);
        assert!(e.to_string().contains("lane rm fix-auth"), "{e}");

        let gone = store
            .write(None, |tx| tx.remove_lane(repo, "fix-auth", None))
            .unwrap();
        assert_eq!(gone.id, a.id);
        assert!(store.lanes(Some(repo)).unwrap().is_empty());
        let e = store
            .write(None, |tx| tx.remove_lane(repo, "fix-auth", None))
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound);
    }

    #[test]
    fn port_blocks_never_overlap_and_are_reused_after_rm() {
        let mut store = Store::open_at(&temp_db("ports")).unwrap();
        let repo = Path::new("/work/acme");
        let mut starts = Vec::new();
        for name in ["a", "b", "c"] {
            let lane = store
                .write(None, |tx| {
                    let ports = tx.allocate_ports(4100, 10)?;
                    tx.insert_lane(new_lane(repo, name, Path::new("/l"), ports))
                })
                .unwrap();
            starts.push(lane.ports.start);
        }
        assert_eq!(starts, [4100, 4110, 4120]);
        store
            .write(None, |tx| tx.remove_lane(repo, "b", None))
            .unwrap();
        let next = store.write(None, |tx| tx.allocate_ports(4100, 10)).unwrap();
        assert_eq!(next.start, 4110, "a freed block is used again");
        // A bigger block skips gaps too small for it.
        let big = store.write(None, |tx| tx.allocate_ports(4100, 15)).unwrap();
        assert_eq!(big.start, 4130);
        // Running out says what to do.
        let e = store
            .write(None, |tx| tx.allocate_ports(65_530, 10))
            .unwrap_err();
        assert!(e.to_string().contains("lane rm"), "{e}");
    }

    #[test]
    fn a_retried_request_runs_once() {
        let mut store = Store::open_at(&temp_db("request")).unwrap();
        let repo = Path::new("/work/acme");
        let req = Request {
            id: "req-1",
            command: "new fix-auth",
        };
        let make = |tx: &Tx<'_>| {
            let ports = tx.allocate_ports(4100, 10)?;
            tx.insert_lane(new_lane(repo, "fix-auth", Path::new("/l"), ports))
        };
        let first = store.write(Some(req), make).unwrap();
        // Without the request id, this would be a "name taken" conflict.
        let again = store.write(Some(req), make).unwrap();
        assert_eq!(first, again);
        assert_eq!(store.lanes(None).unwrap().len(), 1);

        // The same id for a different command is refused.
        let other = Request {
            id: "req-1",
            command: "rm fix-auth",
        };
        let e = store
            .write(Some(other), |tx| tx.remove_lane(repo, "fix-auth", None))
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Conflict);
        assert_eq!(store.lanes(None).unwrap().len(), 1, "nothing changed");
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let mut store = Store::open_at(&temp_db("rollback")).unwrap();
        let repo = Path::new("/work/acme");
        let req = Request {
            id: "req-2",
            command: "new x",
        };
        let e = store
            .write::<Lane, _>(Some(req), |tx| {
                let ports = tx.allocate_ports(4100, 10)?;
                tx.insert_lane(new_lane(repo, "x", Path::new("/l"), ports))?;
                Err(Error::new(ErrorKind::Failure, "copy failed"))
            })
            .unwrap_err();
        assert_eq!(e.message, "copy failed");
        assert!(store.lanes(None).unwrap().is_empty(), "rolled back");
        // The request wasn't recorded, so a retry runs for real.
        let lane = store
            .write(Some(req), |tx| {
                let ports = tx.allocate_ports(4100, 10)?;
                tx.insert_lane(new_lane(repo, "x", Path::new("/l"), ports))
            })
            .unwrap();
        assert_eq!(lane.name, "x");
    }

    #[test]
    fn outcomes_are_stored() {
        let mut store = Store::open_at(&temp_db("outcome")).unwrap();
        let repo = Path::new("/work/acme");
        let lane = store
            .write(None, |tx| {
                tx.insert_lane(new_lane(
                    repo,
                    "x",
                    Path::new("/l"),
                    PortBlock {
                        start: 4100,
                        count: 10,
                    },
                ))
            })
            .unwrap();
        store
            .write(None, |tx| tx.set_outcome(&lane.id, Some(Outcome::Failed)))
            .unwrap();
        assert_eq!(store.lanes(None).unwrap()[0].outcome, Some(Outcome::Failed));
        let e = store
            .write(None, |tx| tx.set_outcome("ln_nope", None))
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound);
    }

    #[test]
    fn a_brand_new_store_opened_by_many_at_once_never_says_locked() {
        // Five agents asking for lanes at once on a new machine, with no daemon: every one opens
        // the store before it exists.
        for round in 0..25 {
            let db = temp_db(&format!("fresh-{round}"));
            let threads: Vec<_> = (0..8)
                .map(|t| {
                    let db = db.clone();
                    std::thread::spawn(move || -> Result<()> {
                        let mut store = Store::open_at(&db)?;
                        let name = format!("t{t}");
                        store.write(None, |tx| {
                            let ports = tx.allocate_ports(4100, 10)?;
                            tx.insert_lane(new_lane(
                                Path::new("/work/acme"),
                                &name,
                                Path::new("/l"),
                                ports,
                            ))
                        })?;
                        Ok(())
                    })
                })
                .collect();
            for t in threads {
                if let Err(e) = t.join().unwrap() {
                    panic!("round {round}: {e}");
                }
            }
            assert_eq!(Store::open_at(&db).unwrap().lanes(None).unwrap().len(), 8);
            let _ = fs::remove_dir_all(db.parent().unwrap());
        }
    }

    #[test]
    fn concurrent_writers_never_clash() {
        let db = temp_db("concurrent");
        drop(Store::open_at(&db).unwrap());
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let db = db.clone();
                std::thread::spawn(move || {
                    let mut store = Store::open_at(&db).unwrap();
                    for i in 0..10 {
                        let name = format!("t{t}-{i}");
                        store
                            .write(None, |tx| {
                                let ports = tx.allocate_ports(4100, 10)?;
                                tx.insert_lane(new_lane(
                                    Path::new("/work/acme"),
                                    &name,
                                    Path::new("/l"),
                                    ports,
                                ))
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let lanes = Store::open_at(&db).unwrap().lanes(None).unwrap();
        assert_eq!(lanes.len(), 40);
        for (i, a) in lanes.iter().enumerate() {
            for b in &lanes[i + 1..] {
                assert!(
                    !a.ports.overlaps(b.ports),
                    "{} and {} share ports",
                    a.name,
                    b.name
                );
            }
        }
    }

    #[test]
    fn a_v1_store_is_migrated_and_keeps_its_lanes() {
        let db = temp_db("migrate");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO repos (root, created_at) VALUES ('/work/acme', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lanes (id, repo_id, name, path, base_commit, port_start, port_count, agent, outcome, created_at, updated_at)
             VALUES ('ln_old', 1, 'old', '/l', NULL, 4100, 10, NULL, NULL, 1, 1)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);
        let store = Store::open_at(&db).unwrap();
        let lanes = store.lanes(None).unwrap();
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].status, Status::Ready, "old lanes count as ready");
        assert!(store.base(Path::new("/work/acme")).unwrap().is_none());
    }

    #[test]
    fn a_creating_lane_holds_its_name_until_finished_or_dropped() {
        let mut store = Store::open_at(&temp_db("creating")).unwrap();
        let repo = Path::new("/work/acme");
        let mut new = new_lane(
            repo,
            "x",
            Path::new("/l"),
            PortBlock {
                start: 4100,
                count: 10,
            },
        );
        new.status = Status::Creating;
        let lane = store.write(None, |tx| tx.insert_lane(new.clone())).unwrap();
        assert_eq!(
            store.lane(repo, "x").unwrap().unwrap().status,
            Status::Creating
        );
        let e = store
            .write(None, |tx| tx.insert_lane(new.clone()))
            .unwrap_err();
        assert!(e.to_string().contains("being created right now"), "{e}");
        store
            .write(None, |tx| tx.finish_lane(&lane.id, "def456"))
            .unwrap();
        let ready = store.lane(repo, "x").unwrap().unwrap();
        assert_eq!(
            (ready.status, ready.base_commit.as_deref()),
            (Status::Ready, Some("def456"))
        );
        assert_eq!(store.lane_at(Path::new("/l")).unwrap().unwrap().id, lane.id);
        store.write(None, |tx| tx.drop_lane(&lane.id)).unwrap();
        assert!(store.lane(repo, "x").unwrap().is_none());
    }

    #[test]
    fn a_lane_left_creating_by_a_crash_frees_its_name_later() {
        let db = temp_db("stale");
        let mut store = Store::open_at(&db).unwrap();
        let repo = Path::new("/work/acme");
        let mut new = new_lane(
            repo,
            "x",
            Path::new("/l"),
            PortBlock {
                start: 4100,
                count: 10,
            },
        );
        new.status = Status::Creating;
        store.write(None, |tx| tx.insert_lane(new.clone())).unwrap();
        // Pretend the crash was an hour ago.
        store
            .conn
            .execute("UPDATE lanes SET updated_at = updated_at - 3600", [])
            .unwrap();
        let again = store.write(None, |tx| tx.insert_lane(new.clone())).unwrap();
        assert_eq!(store.lanes(None).unwrap().len(), 1);
        assert_eq!(store.lanes(None).unwrap()[0].id, again.id);
    }

    #[test]
    fn the_base_is_recorded_and_updated() {
        let mut store = Store::open_at(&temp_db("base")).unwrap();
        let repo = Path::new("/work/acme");
        store
            .write(None, |tx| {
                tx.set_base(repo, Path::new("/work/acme.lanes/_base"), "c1", Some("k1"))
            })
            .unwrap();
        let b = store.base(repo).unwrap().unwrap();
        assert_eq!(
            (b.commit.as_str(), b.install_key.as_deref()),
            ("c1", Some("k1"))
        );
        store
            .write(None, |tx| {
                tx.set_base(repo, Path::new("/work/acme.lanes/_base"), "c2", None)
            })
            .unwrap();
        let b = store.base(repo).unwrap().unwrap();
        assert_eq!((b.commit.as_str(), b.install_key), ("c2", None));
    }

    fn stats(bytes: u64) -> LaneStats {
        LaneStats {
            method: "clone".into(),
            files: 10,
            bytes,
            made_ms: 900,
            install_ms: Some(48_000),
        }
    }

    #[test]
    fn what_a_lane_saved_outlives_it() {
        let mut store = Store::open_at(&temp_db("records")).unwrap();
        let (acme, other) = (Path::new("/work/acme"), Path::new("/work/other"));
        let block = |start| PortBlock { start, count: 10 };
        let ids = store
            .write(None, |tx| {
                let a = tx.insert_lane(new_lane(acme, "a", Path::new("/l/a"), block(4100)))?;
                let b = tx.insert_lane(new_lane(acme, "b", Path::new("/l/b"), block(4110)))?;
                let c = tx.insert_lane(new_lane(other, "c", Path::new("/l/c"), block(4120)))?;
                tx.set_lane_stats(&a.id, &stats(100))?;
                tx.set_lane_stats(&b.id, &stats(200))?;
                tx.set_lane_stats(&c.id, &stats(300))?;
                Ok([a.id, b.id, c.id])
            })
            .unwrap();
        store
            .write(None, |tx| tx.remove_lane(acme, "a", Some(7)))
            .unwrap();

        // What lanes take now counts the lanes still here.
        let live = store.lane_stats(Some(acme)).unwrap();
        assert_eq!(live.keys().collect::<Vec<_>>(), vec![&ids[1]]);
        // The totals keep the removed lane, with what it took when it went.
        let records = store.lane_records(Some(acme)).unwrap();
        assert_eq!(records.len(), 2);
        let a = records.iter().find(|r| r.lane_id == ids[0]).unwrap();
        assert!(a.removed_at.is_some());
        assert_eq!((a.extra_bytes, a.stats.bytes), (Some(7), 100));
        let b = records.iter().find(|r| r.lane_id == ids[1]).unwrap();
        assert_eq!((b.removed_at, b.extra_bytes), (None, None));
        assert_eq!(store.lane_records(None).unwrap().len(), 3);
        assert_eq!(store.lane_records(Some(other)).unwrap().len(), 1);

        // A lane that failed to make saved nothing.
        store.write(None, |tx| tx.drop_lane(&ids[2])).unwrap();
        assert!(store.lane_records(Some(other)).unwrap().is_empty());
    }

    #[test]
    fn a_v3_store_keeps_what_its_lanes_saved() {
        let db = temp_db("migrate-v3");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(MIGRATE_2).unwrap();
        conn.execute_batch(MIGRATE_3).unwrap();
        conn.execute(
            "INSERT INTO repos (root, created_at) VALUES ('/work/acme', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lanes (id, repo_id, name, path, base_commit, port_start, port_count, agent, outcome, created_at, updated_at)
             VALUES ('ln_old', 1, 'old', '/l', NULL, 4100, 10, NULL, NULL, 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lane_stats (lane_id, method, files, bytes, made_ms, install_ms, at)
             VALUES ('ln_old', 'clone', 10, 500, 900, 48000, 1)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        drop(conn);
        let mut store = Store::open_at(&db).unwrap();
        let acme = Path::new("/work/acme");
        assert_eq!(store.lane_records(Some(acme)).unwrap().len(), 1);
        store
            .write(None, |tx| tx.remove_lane(acme, "old", Some(5)))
            .unwrap();
        let records = store.lane_records(Some(acme)).unwrap();
        assert_eq!(records[0].extra_bytes, Some(5), "{records:?}");
    }

    #[test]
    fn a_request_result_can_be_read_before_doing_any_work() {
        let mut store = Store::open_at(&temp_db("peek")).unwrap();
        let req = Request {
            id: "r",
            command: "new x",
        };
        assert_eq!(store.request_result::<String>(req).unwrap(), None);
        store.write(Some(req), |_| Ok("done".to_string())).unwrap();
        assert_eq!(
            store.request_result::<String>(req).unwrap().as_deref(),
            Some("done")
        );
        let e = store
            .request_result::<String>(Request {
                id: "r",
                command: "rm x",
            })
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Conflict);
    }

    /// Two stores that grew apart (a terminal's and an app package's copy of AppData) become
    /// one: every lane once, nothing here overwritten, no ports shared, and doing it again
    /// changes nothing.
    #[test]
    fn an_older_store_is_taken_in_once() {
        let db = temp_db("take-in-here");
        let lanes = db.parent().unwrap().join("lanes");
        let folder = |name: &str| {
            let p = lanes.join(name);
            fs::create_dir_all(&p).unwrap();
            p
        };
        let (acme, other) = (Path::new("/work/acme"), Path::new("/work/other"));
        let block = |start| PortBlock { start, count: 10 };
        let (a, b, gone, same) = (folder("a"), folder("b"), lanes.join("gone"), folder("same"));
        let mut old = Store::open_at(&temp_db("take-in-old")).unwrap();
        let old_lanes = old
            .write(None, |tx| {
                let shared = tx.insert_lane(new_lane(acme, "shared", &a, block(4100)))?;
                let b = tx.insert_lane(new_lane(acme, "b", &b, block(4110)))?;
                let gone = tx.insert_lane(new_lane(other, "gone", &gone, block(4120)))?;
                let same = tx.insert_lane(new_lane(other, "same", &same, block(4130)))?;
                tx.set_lane_stats(&b.id, &stats(200))?;
                tx.set_lane_stats(&gone.id, &stats(300))?;
                tx.set_base(acme, Path::new("/work/acme.lanes/_base"), "new", None)?;
                Ok([shared, b, gone, same])
            })
            .unwrap();
        old.write(
            Some(Request {
                id: "req-1",
                command: "new b",
            }),
            |_| Ok("made b".to_string()),
        )
        .unwrap();

        // Here: the same lane as the old store (copied before they grew apart), the base
        // touched earlier, a lane on b's ports, and another lane called `same`.
        let mut here = Store::open_at(&db).unwrap();
        here.write(None, |tx| {
            tx.set_base(acme, Path::new("/work/acme.lanes/_base"), "old", None)?;
            tx.insert_lane(new_lane(other, "same", &same, block(4200)))?;
            tx.insert_lane(new_lane(other, "x", &folder("x"), block(4110)))?;
            let shared = tx.insert_lane(new_lane(acme, "shared", &a, block(4100)))?;
            tx.tx
                .execute(
                    "UPDATE lanes SET id = ?1 WHERE id = ?2",
                    params![old_lanes[0].id, shared.id],
                )
                .map_err(db_err)?;
            Ok(())
        })
        .unwrap();
        // The base here was touched first.
        here.conn
            .execute("UPDATE bases SET updated_at = 1", [])
            .unwrap();

        let taken = here.take_in(&old, None, "C:/old/lane").unwrap();
        let names = |l: &[Lane]| l.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&taken.lanes), ["b"], "shared is here already");
        let out: Vec<(&str, &str)> = taken
            .left_out
            .iter()
            .map(|(l, why)| (l.name.as_str(), why.as_str()))
            .collect();
        assert_eq!(
            out,
            [
                ("gone", "its folder has gone"),
                ("same", "a lane of its repo here has its name")
            ]
        );
        assert_eq!(taken.new_ports.len(), 1, "x had b's ports");
        let b_here = here.lane(acme, "b").unwrap().unwrap();
        assert_eq!(b_here.id, old_lanes[1].id, "it keeps its id");
        assert_ne!(b_here.ports, block(4110));
        assert_eq!(taken.new_ports[0].1, block(4110));
        // Its history came too, and a note of where from.
        let kinds: Vec<String> = here
            .rows(
                "SELECT kind FROM operations WHERE lane_id = ?1 ORDER BY id",
                [&b_here.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kinds, ["new", "moved"]);
        // What lanes saved: b's, and gone's, marked removed since it didn't come.
        let records = here.lane_records(None).unwrap();
        assert_eq!(records.len(), 2);
        let gone_record = records
            .iter()
            .find(|r| r.lane_id == old_lanes[2].id)
            .unwrap();
        assert!(gone_record.removed_at.is_some());
        // The base the old store touched last.
        assert_eq!(here.base(acme).unwrap().unwrap().commit, "new");
        // A retry of a request made before the move gets its first answer.
        let again: Option<String> = here
            .request_result(Request {
                id: "req-1",
                command: "new b",
            })
            .unwrap();
        assert_eq!(again.as_deref(), Some("made b"));

        // Again: nothing more.
        let before = here.lanes(None).unwrap();
        let ops: i64 = here
            .conn
            .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))
            .unwrap();
        let twice = here.take_in(&old, None, "C:/old/lane").unwrap();
        assert!(twice.lanes.is_empty() && twice.new_ports.is_empty());
        assert_eq!(here.lanes(None).unwrap(), before);
        let ops_after: i64 = here
            .conn
            .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ops, ops_after);
        assert_eq!(here.lane_records(None).unwrap().len(), 2);
    }

    #[test]
    fn taking_in_since_a_time_leaves_older_lanes_alone() {
        let db = temp_db("take-in-since");
        let lanes = db.parent().unwrap().join("lanes");
        fs::create_dir_all(lanes.join("kept")).unwrap();
        let acme = Path::new("/work/acme");
        let mut old = Store::open_at(&temp_db("take-in-since-old")).unwrap();
        let kept = lanes.join("kept");
        old.write(None, |tx| {
            tx.insert_lane(new_lane(
                acme,
                "kept",
                &kept,
                PortBlock {
                    start: 4100,
                    count: 10,
                },
            ))
        })
        .unwrap();
        // Removed here with `--keep-files` after an earlier move: its folder is still there,
        // but it mustn't come back.
        let mut here = Store::open_at(&db).unwrap();
        let taken = here.take_in(&old, Some(now() + 60), "old").unwrap();
        assert!(taken.lanes.is_empty() && taken.left_out.is_empty());
        assert!(here.lanes(None).unwrap().is_empty());
    }

    #[test]
    fn names_fit_folders_and_urls() {
        for ok in ["fix-auth", "a", "v2", "add-tests-for-billing"] {
            assert!(check_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Fix",
            "fix_auth",
            "-x",
            "x-",
            "a b",
            "_base",
            &"x".repeat(41),
        ] {
            let e = check_name(bad).unwrap_err();
            assert_eq!(e.kind, ErrorKind::Usage, "{bad}");
        }
    }
}
