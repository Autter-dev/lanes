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
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The schema version in `PRAGMA user_version`. Bump only with a migration below.
pub const SCHEMA_VERSION: i64 = 2;

/// How long request ids are remembered.
const REQUEST_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// A lane still being created after this long was left behind by a crash.
const STALE_CREATING_SECS: i64 = 15 * 60;

/// The highest port a block may end on.
const MAX_PORT: u32 = 65_535;

/// The folder Lanes keeps its state in.
pub fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("LANE_STATE_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(local).join("lane"));
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
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(db_err)?;
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

    /// Forget a lane by id (a failed `lane new` cleaning up after itself).
    pub fn drop_lane(&self, lane_id: &str) -> Result<()> {
        self.tx
            .execute("DELETE FROM lanes WHERE id = ?1", [lane_id])
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

    /// Forget a lane and free its ports. Returns what was removed.
    pub fn remove_lane(&self, repo: &Path, name: &str) -> Result<Lane> {
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
            .write(None, |tx| tx.remove_lane(repo, "fix-auth"))
            .unwrap();
        assert_eq!(gone.id, a.id);
        assert!(store.lanes(Some(repo)).unwrap().is_empty());
        let e = store
            .write(None, |tx| tx.remove_lane(repo, "fix-auth"))
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
        store.write(None, |tx| tx.remove_lane(repo, "b")).unwrap();
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
            .write(Some(other), |tx| tx.remove_lane(repo, "fix-auth"))
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
