//! Bringing Lanes' state and settings out of AppData, on Windows.
//!
//! Lanes before 0.1.0 kept its state in `%LOCALAPPDATA%\lane` and its settings in
//! `%APPDATA%\lane`. Windows gives an app installed from a package (the Claude desktop app is
//! one) a private copy of AppData: what the programs it starts write there goes to
//! `%LOCALAPPDATA%\Packages\<package>\LocalCache\Local\...` (or `Roaming`), and only they see it.
//! So a `lane` that Claude Code ran inside that app kept lanes your terminal never saw, and both
//! could hand out the same ports. Lanes now keeps both in `%USERPROFILE%\.lane`, which Windows
//! doesn't redirect, and brings across what the old folders hold:
//!
//! - **Every copy.** The old folder and each package's copy of it are each a source. Inside a
//!   package, the old folder's path shows the package's copy of any file it has one of, so the old
//!   folder counts there only when its files are its own (the same file seen by two paths is one
//!   file); otherwise a run outside that package brings it across later.
//! - **Merged, never replaced.** Lanes come across by id ([`Store::take_in`]), so a lane in two
//!   copies comes once and nothing here is overwritten. Settings merge so that a setting switched
//!   off anywhere stays off. Either side can go first: the result is the same.
//! - **Once per source.** `moved.json`, in the new state folder and in the new settings folder,
//!   notes each source and how its files looked. A source comes across again only when it has
//!   changed (an older Lanes still running wrote to it), and then only lanes made since.
//! - **Only ever read.** An old store is read under its own write lock, so no older `lane` is
//!   mid-write, with read-only handles, which inside a package never make a private copy: its
//!   files are copied, and the copy is opened. Nothing old is deleted; `lane doctor` lists the old
//!   folders once everything in them has come across.
//! - **The old daemon stops.** An older `laned` listening for the old folder is asked to stop
//!   before its store is read, so it writes nothing more there. The new one starts when needed.
//!
//! One process moves at a time (`moving.lock`); the others wait, then find it done.

use crate::config::{CONFIG_VERSION, Config};
use crate::daemon::{self, Endpoint};
use crate::log;
use crate::state::{Store, TakenIn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The note of what came across.
const MOVED: &str = "moved.json";

/// A store's data: the database and its write-ahead log.
const STORE: [&str; 2] = ["state.db", "state.db-wal"];

/// The files of a store a package may have its own copy of. Any one of them in the package's
/// copy means the old folder can't be read as it really is from inside that package.
const STORE_FILES: [&str; 4] = ["state.db", "state.db-wal", "state.db-shm", "state.lock"];

/// Lanes' settings files.
const SETTINGS: [&str; 3] = ["config.toml", "install.json", "agent-settings.json"];

/// An old folder as this process sees it, and each app package's private copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Old {
    /// `%LOCALAPPDATA%\lane` (state) or `%APPDATA%\lane` (settings). Inside a package, this path
    /// shows the package's copy of a file wherever the package has one.
    pub folder: PathBuf,
    /// `%LOCALAPPDATA%\Packages\<package>\LocalCache\Local\lane` (or `Roaming\lane`), for every
    /// package that has one: what `lane` wrote while that app had started it.
    pub copies: Vec<PathBuf>,
}

impl Old {
    /// Lanes' old state folder on Windows, and with `copies`, every package's copy of it.
    pub fn state(copies: bool) -> Option<Old> {
        Old::find("LOCALAPPDATA", "Local", copies)
    }

    /// Lanes' old settings folder on Windows, and with `copies`, every package's copy of it.
    pub fn settings(copies: bool) -> Option<Old> {
        Old::find("APPDATA", "Roaming", copies)
    }

    fn find(var: &str, under: &str, copies: bool) -> Option<Old> {
        if !cfg!(windows) {
            return None;
        }
        let home = etcetera::home_dir().ok();
        let appdata = |var: &str, under: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join("AppData").join(under)))
        };
        let folder = appdata(var, under)?.join("lane");
        let copies = match copies {
            true => appdata("LOCALAPPDATA", "Local")
                .map(|local| package_copies(&local.join("Packages"), under))
                .unwrap_or_default(),
            false => Vec::new(),
        };
        Some(Old { folder, copies })
    }
}

/// Every package's copy of the `lane` folder in `packages` (`%LOCALAPPDATA%\Packages`), in order.
/// `under` is `Local` or `Roaming`.
pub fn package_copies(packages: &Path, under: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(packages) else {
        return Vec::new();
    };
    let mut copies: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path().join("LocalCache").join(under).join("lane"))
        .filter(|p| p.is_dir())
        .collect();
    copies.sort();
    copies
}

/// The package a copy belongs to (`Claude_pzs8sxrjxfjjc`), from its path.
pub fn package_name(copy: &Path) -> String {
    copy.ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| copy.display().to_string())
}

/// The package copy shown here in place of `folder`'s own files, if any: inside a package,
/// Windows shows the package's copy of a file at the old path, and the two paths are then one
/// file. Outside every package, or in one that never wrote these `files`, there's none.
pub fn shown_instead(folder: &Path, copies: &[PathBuf], files: &[&str]) -> Option<PathBuf> {
    copies
        .iter()
        .find(|copy| {
            files
                .iter()
                .any(|f| same_file(&folder.join(f), &copy.join(f)))
        })
        .cloned()
}

/// Whether two paths are one file. False when either isn't there.
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((file_id(a), file_id(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(unix)]
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // Read-only: inside a package, that never makes a private copy of the file.
    let file = File::open(path).ok()?;
    // SAFETY: a plain struct of integers, which the call fills in.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle stays open for the call, and `info` is a valid place to write.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    (ok != 0).then(|| {
        (
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        )
    })
}

/// How a folder's `files` look: each one's size and time, so any write changes it. `None` when
/// none of them is there.
fn look(dir: &Path, files: &[&str]) -> Option<String> {
    let parts: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let meta = fs::metadata(dir.join(f)).ok()?;
            let at = meta
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos();
            Some(format!("{f}:{}:{at}", meta.len()))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// How an old store looks, when there's one in `dir`.
fn store_look(dir: &Path) -> Option<String> {
    dir.join("state.db")
        .is_file()
        .then(|| look(dir, &STORE))
        .flatten()
}

/// How an old settings folder looks, when it has any settings.
fn settings_look(dir: &Path) -> Option<String> {
    look(dir, &SETTINGS)
}

/// `moved.json`: each source that came across, and how it looked then.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    sources: BTreeMap<String, Came>,
    /// How the old folder looked from inside a package that shows its own copy there, so that
    /// isn't taken for a change each time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hidden: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Came {
    /// How its files looked when it came across.
    look: String,
    /// When (Unix seconds): a later time it comes across, only lanes made since then do.
    at: i64,
    /// How many lanes came across from it, all told.
    #[serde(default)]
    lanes: usize,
}

impl Record {
    fn load(dir: &Path) -> Option<Record> {
        let text = fs::read_to_string(dir.join(MOVED)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save(&self, dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).expect("the record serializes") + "\n";
        write_atomic(&dir.join(MOVED), text.as_bytes())
    }

    fn came(&self, source: &Path) -> Option<&Came> {
        self.sources.get(&key(source))
    }

    /// Whether a source looking like `look` has come across, or is the package copy seen here.
    fn knows(&self, look: &str) -> bool {
        self.hidden.as_deref() == Some(look) || self.sources.values().any(|c| c.look == look)
    }
}

fn key(source: &Path) -> String {
    source.to_string_lossy().into_owned()
}

/// What came across from the old state folders.
#[derive(Debug, Default)]
pub struct StateMoved {
    /// The state folder now.
    pub to: PathBuf,
    /// Each old store that came across this time, and what came.
    pub from: Vec<(PathBuf, TakenIn)>,
    /// Old stores that couldn't, and why. The next run tries again.
    pub failed: Vec<(PathBuf, String)>,
    /// The package copy shown here in place of the old folder, which then can't be read from
    /// here: a run outside that package brings it across.
    pub hidden_by: Option<PathBuf>,
}

/// Bring each old store that hasn't come across yet, or has changed since, into the store in
/// `to`. See the notes at the top.
pub fn move_state(to: &Path, old: &Old) -> StateMoved {
    let mut out = StateMoved {
        to: to.to_path_buf(),
        hidden_by: shown_instead(&old.folder, &old.copies, &STORE_FILES),
        ..StateMoved::default()
    };
    let hidden_look = out.hidden_by.as_ref().and_then(|_| store_look(&old.folder));
    let sources: Vec<PathBuf> = out
        .hidden_by
        .is_none()
        .then(|| old.folder.clone())
        .into_iter()
        .chain(old.copies.iter().cloned())
        .collect();
    let waiting = |record: &Record| -> Vec<PathBuf> {
        sources
            .iter()
            .filter(|s| store_look(s).is_some_and(|l| record.came(s).is_none_or(|c| c.look != l)))
            .cloned()
            .collect()
    };
    let record = Record::load(to).unwrap_or_default();
    let hidden_noted = hidden_look.is_none() || record.hidden == hidden_look;
    if waiting(&record).is_empty() && hidden_noted {
        return out;
    }
    let _moving = match fs::create_dir_all(to).and_then(|()| lock(&to.join("moving.lock"))) {
        Ok(lock) => lock,
        Err(e) => {
            out.failed.push((old.folder.clone(), e.to_string()));
            return out;
        }
    };
    // Another process may have moved it while this one waited.
    let mut record = Record::load(to).unwrap_or_default();
    if hidden_look.is_some() {
        record.hidden = hidden_look;
    }
    let waiting = waiting(&record);
    if !waiting.is_empty() {
        // An older laned would go on writing to the old store: stop it before reading it.
        let _ = daemon::stop(&Endpoint::at(&old.folder));
        match Store::open_at(&to.join("state.db")) {
            Ok(mut store) => {
                for source in waiting {
                    let came = record.came(&source).cloned();
                    match bring(&mut store, &source, to, came.as_ref().map(|c| c.at)) {
                        Ok((taken, look, at)) => {
                            if record.sources.is_empty() {
                                bring_log(&source, to);
                            }
                            let lanes = came.map_or(0, |c| c.lanes) + taken.lanes.len();
                            record
                                .sources
                                .insert(key(&source), Came { look, at, lanes });
                            out.from.push((source, taken));
                        }
                        Err(why) => out.failed.push((source, why)),
                    }
                }
            }
            Err(e) => out.failed.push((to.join("state.db"), e.to_string())),
        }
    }
    if let Err(e) = record.save(to) {
        out.failed.push((to.join(MOVED), e.to_string()));
    }
    out
}

/// Bring one old store across: copy it under its write lock, open the copy, take in what it
/// has. Returns what came, how the old store looked, and when.
fn bring(
    store: &mut Store,
    source: &Path,
    to: &Path,
    since: Option<i64>,
) -> Result<(TakenIn, String, i64), String> {
    let copy = to.join(format!("moving-{}", std::process::id()));
    let _ = fs::remove_dir_all(&copy);
    fs::create_dir_all(&copy).map_err(|e| format!("couldn't make {}: {e}", copy.display()))?;
    let read = || -> Result<(String, i64), String> {
        let _held = hold(&source.join("state.lock"))?;
        let look = store_look(source).ok_or("its state.db has gone")?;
        let at = now();
        // While the lock is held nothing writes the log (`state.db-wal`), but a reader that
        // closes may copy it into the database and delete it. Copied first, the log still has
        // everything, whatever the database has by the time it's copied.
        copy_file(
            &source.join("state.db-wal"),
            &copy.join("state.db-wal"),
            true,
        )?;
        copy_file(&source.join("state.db"), &copy.join("state.db"), false)?;
        Ok((look, at))
    };
    let result = read().and_then(|(look, at)| {
        let old = Store::open_at(&copy.join("state.db")).map_err(|e| e.to_string())?;
        let taken = store
            .take_in(&old, since, &source.display().to_string())
            .map_err(|e| e.to_string())?;
        Ok((taken, look, at))
    });
    let _ = fs::remove_dir_all(&copy);
    result
}

/// The old store's write lock, held while it's read. Read-only, so inside a package it's the
/// old folder's own lock file and no private copy is made. Waits up to 10 seconds for an older
/// `lane` to finish its write. `None` when the old store never had a lock file.
fn hold(path: &Path) -> Result<Option<File>, String> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("couldn't open {}: {e}", path.display())),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(TryLockError::WouldBlock) => {
                return Err("an older Lanes kept writing to it for 10 seconds".into());
            }
            Err(TryLockError::Error(e)) => {
                return Err(format!("couldn't lock {}: {e}", path.display()));
            }
        }
    }
}

/// An exclusive lock on `path`, waiting as long as it takes.
fn lock(path: &Path) -> io::Result<File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

/// Copy a file by reading it, which never changes it. A missing `optional` one is skipped.
fn copy_file(from: &Path, to: &Path, optional: bool) -> Result<(), String> {
    let mut src = match File::open(from) {
        Ok(f) => f,
        Err(e) if optional && e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("couldn't read {}: {e}", from.display())),
    };
    File::create(to)
        .and_then(|mut dst| io::copy(&mut src, &mut dst))
        .map(drop)
        .map_err(|e| format!("couldn't write {}: {e}", to.display()))
}

/// The first store to come across brings its log too, so `lane logs` goes back past the move.
fn bring_log(source: &Path, to: &Path) {
    if to.join("lane.log").exists() {
        return;
    }
    for name in ["lane.log.1", "lane.log"] {
        let _ = copy_file(&source.join(name), &to.join(name), true);
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("moving.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What came across from the old settings folders.
#[derive(Debug, Default)]
pub struct SettingsMoved {
    /// The settings folder now.
    pub to: PathBuf,
    /// Each old settings folder that came across this time.
    pub from: Vec<PathBuf>,
    /// Old settings folders that couldn't, and why. The next run tries again.
    pub failed: Vec<(PathBuf, String)>,
    /// The package copy shown here in place of the old folder.
    pub hidden_by: Option<PathBuf>,
}

/// Bring each old settings folder that hasn't come across yet, or has changed since, into the
/// settings in `to`.
pub fn move_settings(to: &Path, old: &Old) -> SettingsMoved {
    let mut out = SettingsMoved {
        to: to.to_path_buf(),
        hidden_by: shown_instead(&old.folder, &old.copies, &SETTINGS),
        ..SettingsMoved::default()
    };
    let hidden_look = out
        .hidden_by
        .as_ref()
        .and_then(|_| settings_look(&old.folder));
    let sources: Vec<PathBuf> = out
        .hidden_by
        .is_none()
        .then(|| old.folder.clone())
        .into_iter()
        .chain(old.copies.iter().cloned())
        .collect();
    let waiting = |record: &Record| -> Vec<PathBuf> {
        sources
            .iter()
            .filter(|s| {
                settings_look(s).is_some_and(|l| record.came(s).is_none_or(|c| c.look != l))
            })
            .cloned()
            .collect()
    };
    let record = Record::load(to).unwrap_or_default();
    let hidden_noted = hidden_look.is_none() || record.hidden == hidden_look;
    if waiting(&record).is_empty() && hidden_noted {
        return out;
    }
    let _moving = match fs::create_dir_all(to).and_then(|()| lock(&to.join("moving.lock"))) {
        Ok(lock) => lock,
        Err(e) => {
            out.failed.push((old.folder.clone(), e.to_string()));
            return out;
        }
    };
    let mut record = Record::load(to).unwrap_or_default();
    if hidden_look.is_some() {
        record.hidden = hidden_look;
    }
    for source in waiting(&record) {
        let Some(look) = settings_look(&source) else {
            continue;
        };
        match merge_settings(&source, to) {
            Ok(()) => {
                record.sources.insert(
                    key(&source),
                    Came {
                        look,
                        at: now(),
                        lanes: 0,
                    },
                );
                out.from.push(source);
            }
            Err(why) => out.failed.push((source, why)),
        }
    }
    if let Err(e) = record.save(to) {
        out.failed.push((to.join(MOVED), e.to_string()));
    }
    out
}

/// Merge one old settings folder into `to`: `config.toml` by [`merge_config`]; the install id
/// of the first `install.json` to come across; and Lanes' note of what it set in agents'
/// settings, file by file, keeping what's noted here already.
fn merge_settings(from: &Path, to: &Path) -> Result<(), String> {
    let theirs = from.join("config.toml");
    if theirs.is_file() {
        let theirs = Config::load_from(&theirs).map_err(|e| e.to_string())?;
        let path = to.join("config.toml");
        let ours = Config::load_from(&path).map_err(|e| e.to_string())?;
        let merged = merge_config(&ours, &theirs);
        if merged != ours || !path.exists() {
            merged.save_to(&path).map_err(|e| e.to_string())?;
        }
    }
    let install = to.join("install.json");
    if !install.exists() {
        let theirs = from.join("install.json");
        match fs::read(&theirs) {
            Ok(bytes) => write_atomic(&install, &bytes)
                .map_err(|e| format!("couldn't write {}: {e}", install.display()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("couldn't read {}: {e}", theirs.display())),
        }
    }
    let theirs = from.join("agent-settings.json");
    if theirs.is_file() {
        type Notes = serde_json::Map<String, serde_json::Value>;
        let read = |path: &Path| -> Result<Notes, String> {
            match fs::read_to_string(path) {
                Ok(t) if t.trim().is_empty() => Ok(Notes::new()),
                Ok(t) => serde_json::from_str(&t)
                    .map_err(|e| format!("{} isn't valid JSON ({e})", path.display())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Notes::new()),
                Err(e) => Err(format!("couldn't read {}: {e}", path.display())),
            }
        };
        let path = to.join("agent-settings.json");
        let mut ours = read(&path)?;
        let before = ours.len();
        for (file, noted) in read(&theirs)? {
            ours.entry(file).or_insert(noted);
        }
        if ours.len() != before {
            let text = serde_json::to_string_pretty(&ours).expect("notes serialize") + "\n";
            write_atomic(&path, text.as_bytes())
                .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// Two copies of the settings as one: a setting switched off in either stays off, and the
/// welcome done in either is done (the later version's). The same whichever comes first.
pub fn merge_config(a: &Config, b: &Config) -> Config {
    let numbers = |v: &str| -> Vec<u64> {
        v.split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().unwrap_or(0))
            .collect()
    };
    let onboarded = match (&a.onboarded, &b.onboarded) {
        (Some(x), Some(y)) => {
            Some(std::cmp::max_by_key(x, y, |v| (numbers(v), v.to_string())).to_string())
        }
        (x, y) => x.clone().or_else(|| y.clone()),
    };
    Config {
        version: CONFIG_VERSION,
        onboarded,
        telemetry: a.telemetry && b.telemetry,
        hints: a.hints && b.hints,
        agents: a.agents && b.agents,
    }
}

/// What this run brought across, if anything.
#[derive(Debug, Default)]
pub struct Report {
    pub state: Option<StateMoved>,
    pub settings: Option<SettingsMoved>,
}

/// Whether an environment variable points a folder elsewhere (tests, CI, sandboxes): the old
/// folders aren't Lanes' then.
fn set(var: &str) -> bool {
    std::env::var_os(var).is_some_and(|v| !v.is_empty())
}

/// On Windows, bring Lanes' old folders across when there's anything new in them, before
/// anything reads the new ones. Quick when there isn't: it reads `moved.json` and looks at a
/// few files, and only then looks in every app package. `everywhere` looks in every package
/// anyway (`lane doctor`). Nothing moves where `LANE_STATE_DIR` or `LANE_CONFIG_DIR` points the
/// folder elsewhere, under `LANE_AGENTS_HOME` (a stand-in home for tests), on other systems, or
/// in unit tests.
pub fn run(everywhere: bool) -> Report {
    let mut report = Report::default();
    if !cfg!(windows) || cfg!(test) || set("LANE_AGENTS_HOME") {
        return report;
    }
    if !set("LANE_STATE_DIR")
        && let (Ok(to), Some(old)) = (crate::state::state_dir(), Old::state(false))
        && (everywhere || worth_a_look(&to, &old.folder, store_look))
    {
        let old = Old::state(true).unwrap_or(old);
        report.state = Some(move_state(&to, &old));
    }
    if !set("LANE_CONFIG_DIR")
        && let (Ok(to), Some(old)) = (crate::config::config_dir(), Old::settings(false))
        && (everywhere || worth_a_look(&to, &old.folder, settings_look))
    {
        let old = Old::settings(true).unwrap_or(old);
        report.settings = Some(move_settings(&to, &old));
    }
    report
}

/// Whether to look properly: the old folder has something that hasn't come across, or a source
/// that came across has changed since.
fn worth_a_look(to: &Path, folder: &Path, look: fn(&Path) -> Option<String>) -> bool {
    let folder = look(folder);
    let Some(record) = Record::load(to) else {
        return folder.is_some();
    };
    let changed = record
        .sources
        .iter()
        .any(|(path, came)| look(Path::new(path)).is_some_and(|l| l != came.look));
    changed || folder.is_some_and(|l| !record.knows(&l))
}

impl Report {
    /// How many lanes came across.
    pub fn lanes(&self) -> usize {
        self.state.as_ref().map_or(0, |s| {
            s.from.iter().map(|(_, taken)| taken.lanes.len()).sum()
        })
    }

    fn came_across(&self) -> bool {
        self.state.as_ref().is_some_and(|s| !s.from.is_empty())
            || self.settings.as_ref().is_some_and(|s| !s.from.is_empty())
    }

    fn failed(&self) -> Vec<&(PathBuf, String)> {
        let state = self.state.iter().flat_map(|s| &s.failed);
        state
            .chain(self.settings.iter().flat_map(|s| &s.failed))
            .collect()
    }

    /// Note what happened in Lanes' log.
    pub fn log(&self) {
        if let Some(s) = &self.state {
            for (from, taken) in &s.from {
                log::info(
                    "moved",
                    format!(
                        "{} came across from {} to {}",
                        count(taken.lanes.len()),
                        from.display(),
                        s.to.display()
                    ),
                )
                .write();
                for (lane, was) in &taken.new_ports {
                    log::warn(
                        "moved",
                        format!(
                            "{} has ports {}-{} now, not {}-{}: a lane here had those. Restart what runs in it.",
                            lane.name,
                            lane.ports.start,
                            lane.ports.end(),
                            was.start,
                            was.end()
                        ),
                    )
                    .lane(&lane.name)
                    .repo(&lane.repo)
                    .write();
                }
                for (lane, why) in &taken.left_out {
                    log::warn(
                        "moved",
                        format!("{} stayed in {}: {why}", lane.name, from.display()),
                    )
                    .lane(&lane.name)
                    .repo(&lane.repo)
                    .write();
                }
            }
        }
        if let Some(s) = &self.settings {
            for from in &s.from {
                log::info(
                    "moved",
                    format!(
                        "settings came across from {} to {}",
                        from.display(),
                        s.to.display()
                    ),
                )
                .write();
            }
        }
        for (from, why) in self.failed() {
            log::error(
                "moved",
                format!(
                    "couldn't bring {} across: {why}. Lanes tries again next time; `lane doctor` shows where it stands.",
                    from.display()
                ),
            )
            .write();
        }
    }

    /// One line for a person at a terminal, when anything came across or couldn't.
    pub fn summary(&self) -> Option<String> {
        let to = match (&self.state, &self.settings) {
            (Some(s), Some(c)) if s.to.starts_with(&c.to) => &c.to,
            (Some(s), _) => &s.to,
            (None, Some(c)) => &c.to,
            (None, None) => return None,
        };
        let to = crate::engine::norm(to);
        if !self.failed().is_empty() {
            return Some(format!(
                "some of Lanes' state and settings couldn't come out of AppData to {} yet. `lane doctor` shows why; Lanes tries again each time.",
                to.display()
            ));
        }
        self.came_across().then(|| {
            format!(
                "Lanes keeps its state and settings in {} now, not AppData ({} came across). `lane doctor` lists the old folders.",
                to.display(),
                count(self.lanes())
            )
        })
    }
}

fn count(n: usize) -> String {
    match n {
        1 => "1 lane".into(),
        n => format!("{n} lanes"),
    }
}

/// Where the old folders stand, for `lane doctor`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Leftovers {
    /// Old folders everything has come across from, still there.
    pub across: Vec<PathBuf>,
    /// Old folders with something that hasn't come across.
    pub waiting: Vec<PathBuf>,
    /// A package copy shown here in place of an old folder that hasn't come across from
    /// outside that package yet.
    pub hidden_by: Option<PathBuf>,
}

/// Where the old folders stand now. Reads only.
pub fn leftovers() -> Leftovers {
    let mut out = Leftovers::default();
    if !cfg!(windows) || set("LANE_AGENTS_HOME") {
        return out;
    }
    if !set("LANE_STATE_DIR")
        && let (Ok(to), Some(old)) = (crate::state::state_dir(), Old::state(true))
    {
        leftovers_of(&to, &old, &STORE_FILES, store_look, &mut out);
    }
    if !set("LANE_CONFIG_DIR")
        && let (Ok(to), Some(old)) = (crate::config::config_dir(), Old::settings(true))
    {
        leftovers_of(&to, &old, &SETTINGS, settings_look, &mut out);
    }
    out
}

fn leftovers_of(
    to: &Path,
    old: &Old,
    files: &[&str],
    look: fn(&Path) -> Option<String>,
    out: &mut Leftovers,
) {
    let record = Record::load(to).unwrap_or_default();
    let hidden = shown_instead(&old.folder, &old.copies, files);
    let folder = hidden.is_none().then(|| old.folder.clone());
    for source in folder.into_iter().chain(old.copies.iter().cloned()) {
        let Some(now) = look(&source) else {
            continue;
        };
        let list = match record.came(&source).is_some_and(|c| c.look == now) {
            true => &mut out.across,
            false => &mut out.waiting,
        };
        if !list.contains(&source) {
            list.push(source);
        }
    }
    if let Some(copy) = hidden
        && record.came(&old.folder).is_none()
        && out.hidden_by.is_none()
    {
        out.hidden_by = Some(copy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{NewLane, Status};

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Temp {
            let dir =
                std::env::temp_dir().join(format!("lane-moved-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A store at `dir` with one lane per name, each with a folder under `lanes` and the ports
    /// the store hands out, as an older Lanes would have made it.
    fn old_store(dir: &Path, repo: &Path, lanes: &Path, names: &[&str]) -> Vec<crate::state::Lane> {
        let mut store = Store::open_at(&dir.join("state.db")).unwrap();
        names
            .iter()
            .map(|name| {
                let path = lanes.join(name);
                fs::create_dir_all(&path).unwrap();
                store
                    .write(None, |tx| {
                        let ports = tx.allocate_ports(4100, 10)?;
                        tx.insert_lane(NewLane {
                            repo,
                            name,
                            path: &path,
                            base_commit: Some("abc"),
                            ports,
                            agent: Some("claude"),
                            status: Status::Ready,
                        })
                    })
                    .unwrap()
            })
            .collect()
    }

    fn names(store: &Store) -> Vec<String> {
        let mut names: Vec<String> = store
            .lanes(None)
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect();
        names.sort();
        names
    }

    /// The old folder and a package's copy, laid out as `%LOCALAPPDATA%` has them.
    fn layout(t: &Temp) -> (Old, PathBuf, PathBuf, PathBuf) {
        let local = t.0.join("AppData").join("Local");
        let folder = local.join("lane");
        let copy = local
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join("Local")
            .join("lane");
        fs::create_dir_all(&folder).unwrap();
        fs::create_dir_all(&copy).unwrap();
        let old = Old {
            folder,
            copies: package_copies(&local.join("Packages"), "Local"),
        };
        (old, copy, t.0.join("repo"), t.0.join("repo.lanes"))
    }

    #[test]
    fn copies_are_found_and_named_by_package() {
        let t = Temp::new("copies");
        let (old, copy, _, _) = layout(&t);
        assert_eq!(old.copies, vec![copy.clone()]);
        assert_eq!(package_name(&copy), "Claude_pzs8sxrjxfjjc");
        let packages = t.0.join("AppData").join("Local").join("Packages");
        fs::create_dir_all(packages.join("Other_123").join("LocalCache")).unwrap();
        assert_eq!(package_copies(&packages, "Local"), vec![copy]);
        assert!(package_copies(&packages, "Roaming").is_empty());
        assert!(package_copies(&t.0.join("nothing"), "Local").is_empty());
    }

    /// Inside a package, the old path and the package's copy are one file. A hard link is the
    /// same thing seen from outside: two paths, one file.
    #[test]
    fn a_package_copy_shown_at_the_old_path_is_told_apart() {
        let t = Temp::new("shown");
        let (old, copy, _, _) = layout(&t);
        fs::write(copy.join("state.db"), "copy").unwrap();
        fs::write(old.folder.join("state.db"), "real").unwrap();
        assert_eq!(shown_instead(&old.folder, &old.copies, &STORE_FILES), None);
        fs::remove_file(old.folder.join("state.db")).unwrap();
        fs::hard_link(copy.join("state.db"), old.folder.join("state.db")).unwrap();
        assert_eq!(
            shown_instead(&old.folder, &old.copies, &STORE_FILES),
            Some(copy.clone())
        );
        // The package wrote only the lock: the old store still can't be read as it is.
        fs::remove_file(old.folder.join("state.db")).unwrap();
        fs::write(old.folder.join("state.db"), "real").unwrap();
        fs::write(copy.join("state.lock"), "").unwrap();
        fs::hard_link(copy.join("state.lock"), old.folder.join("state.lock")).unwrap();
        assert_eq!(
            shown_instead(&old.folder, &old.copies, &STORE_FILES),
            Some(copy)
        );
    }

    /// The bug: a terminal and the Claude app each kept a store, and each handed out the same
    /// ports. Both come across into one store, and no two lanes share ports after.
    #[test]
    fn both_stores_come_across_into_one() {
        let t = Temp::new("both");
        let (old, copy, repo, lanes) = layout(&t);
        let terminal = old_store(&old.folder, &repo, &lanes, &["fix-auth", "docs"]);
        let app = old_store(&copy, &repo, &lanes, &["claude-1"]);
        assert_eq!(
            terminal[0].ports, app[0].ports,
            "the bug: the same ports twice"
        );
        fs::write(old.folder.join("lane.log"), "{}\n").unwrap();
        let before_old = fs::read(old.folder.join("state.db")).unwrap();

        let to = t.0.join("home").join(".lane").join("state");
        let moved = move_state(&to, &old);
        assert!(moved.failed.is_empty(), "{:?}", moved.failed);
        assert_eq!(moved.hidden_by, None);
        assert_eq!(moved.from.len(), 2);
        let store = Store::open_at(&to.join("state.db")).unwrap();
        assert_eq!(names(&store), ["claude-1", "docs", "fix-auth"]);
        let all = store.lanes(None).unwrap();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert!(
                    a.ports.end() < b.ports.start as u32 || b.ports.end() < a.ports.start as u32,
                    "{} and {} share ports",
                    a.name,
                    b.name
                );
            }
        }
        let (_, from_app) = &moved.from[1];
        assert_eq!(from_app.new_ports.len(), 1, "the app's lane got new ports");
        assert_eq!(from_app.new_ports[0].1, app[0].ports);
        assert!(to.join("lane.log").exists(), "the log came too");
        assert_eq!(
            fs::read(old.folder.join("state.db")).unwrap(),
            before_old,
            "the old store is only read"
        );
        drop(store);

        // Again: nothing to do, nothing changes.
        let again = move_state(&to, &old);
        assert!(again.from.is_empty() && again.failed.is_empty());
        assert_eq!(
            names(&Store::open_at(&to.join("state.db")).unwrap()),
            ["claude-1", "docs", "fix-auth"]
        );
        assert!(!worth_a_look(&to, &old.folder, store_look));
        assert!(!to.join(format!("moving-{}", std::process::id())).exists());
    }

    /// Inside the package first: the old folder shows the package's store, so only the copy
    /// comes across. Outside later, the old folder itself comes across. The same result as the
    /// other way round.
    #[test]
    fn either_side_can_go_first() {
        let t = Temp::new("either");
        let (old, copy, repo, lanes) = layout(&t);
        old_store(&old.folder, &repo, &lanes, &["fix-auth"]);
        old_store(&copy, &repo, &lanes, &["claude-1"]);
        let to = t.0.join("new");

        // Inside: the package's files are what the old path shows.
        let real = t.0.join("real-lane");
        fs::rename(&old.folder, &real).unwrap();
        fs::create_dir_all(&old.folder).unwrap();
        for f in ["state.db", "state.lock"] {
            fs::hard_link(copy.join(f), old.folder.join(f)).unwrap();
        }
        let inside = move_state(&to, &old);
        assert_eq!(inside.hidden_by, Some(copy.clone()));
        assert_eq!(inside.from.len(), 1);
        assert_eq!(
            names(&Store::open_at(&to.join("state.db")).unwrap()),
            ["claude-1"]
        );
        assert!(
            !worth_a_look(&to, &old.folder, store_look),
            "quick next time"
        );
        let left = {
            let mut out = Leftovers::default();
            leftovers_of(&to, &old, &STORE_FILES, store_look, &mut out);
            out
        };
        assert_eq!(left.hidden_by, Some(copy.clone()));

        // Outside: the old folder is its own again.
        fs::remove_dir_all(&old.folder).unwrap();
        fs::rename(&real, &old.folder).unwrap();
        assert!(worth_a_look(&to, &old.folder, store_look));
        let outside = move_state(&to, &old);
        assert_eq!(outside.hidden_by, None);
        assert_eq!(
            outside.from.len(),
            1,
            "only the old folder: the copy came already"
        );
        assert_eq!(
            names(&Store::open_at(&to.join("state.db")).unwrap()),
            ["claude-1", "fix-auth"]
        );
        let mut left = Leftovers::default();
        leftovers_of(&to, &old, &STORE_FILES, store_look, &mut left);
        assert_eq!(left.across, vec![old.folder.clone(), copy]);
        assert!(left.waiting.is_empty() && left.hidden_by.is_none());
    }

    /// An older Lanes writes to the old store after the move (a terminal app left open across
    /// the upgrade): its new lane comes across next time, and a lane removed here since doesn't
    /// come back.
    #[test]
    fn a_change_after_the_move_brings_only_whats_new() {
        let t = Temp::new("later");
        let (old, _, repo, lanes) = layout(&t);
        old_store(&old.folder, &repo, &lanes, &["fix-auth", "docs"]);
        let to = t.0.join("new");
        // A lane made in the same second as the move is looked at again next time (one made
        // just after mustn't be missed), so this test keeps the seconds apart.
        std::thread::sleep(Duration::from_millis(1100));
        move_state(&to, &old);
        {
            let mut here = Store::open_at(&to.join("state.db")).unwrap();
            here.write(None, |tx| tx.remove_lane(&repo, "docs", None))
                .unwrap();
        }
        std::thread::sleep(Duration::from_millis(1100));
        old_store(&old.folder, &repo, &lanes, &["late"]);
        assert!(worth_a_look(&to, &old.folder, store_look));
        let moved = move_state(&to, &old);
        assert_eq!(moved.from.len(), 1);
        let store = Store::open_at(&to.join("state.db")).unwrap();
        assert_eq!(names(&store), ["fix-auth", "late"], "docs stays removed");
        let record = Record::load(&to).unwrap();
        assert_eq!(record.came(&old.folder).unwrap().lanes, 3);
    }

    #[test]
    fn a_store_that_cant_be_read_is_tried_again() {
        let t = Temp::new("bad");
        let (old, _, _, _) = layout(&t);
        fs::write(old.folder.join("state.db"), "not a database at all").unwrap();
        let to = t.0.join("new");
        let moved = move_state(&to, &old);
        assert_eq!(moved.failed.len(), 1, "{moved:?}");
        assert!(moved.from.is_empty());
        assert!(
            worth_a_look(&to, &old.folder, store_look),
            "tried again next time"
        );
        let mut left = Leftovers::default();
        leftovers_of(&to, &old, &STORE_FILES, store_look, &mut left);
        assert_eq!(left.waiting, vec![old.folder.clone()]);
    }

    #[test]
    fn nothing_old_means_nothing_to_do() {
        let t = Temp::new("none");
        let to = t.0.join("new");
        let old = Old {
            folder: t.0.join("AppData").join("Local").join("lane"),
            copies: Vec::new(),
        };
        assert!(!worth_a_look(&to, &old.folder, store_look));
        let moved = move_state(&to, &old);
        assert!(moved.from.is_empty() && moved.failed.is_empty());
        assert!(!to.exists(), "a fresh machine gets no folder from this");
    }

    #[test]
    fn settings_merge_so_a_switch_turned_off_stays_off() {
        let on = Config {
            onboarded: Some("0.0.9".into()),
            ..Config::default()
        };
        let off = Config {
            onboarded: Some("0.0.10".into()),
            telemetry: false,
            ..Config::default()
        };
        let merged = merge_config(&on, &off);
        assert_eq!(merged, merge_config(&off, &on), "either way round");
        assert!(!merged.telemetry && merged.hints && merged.agents);
        assert_eq!(merged.onboarded.as_deref(), Some("0.0.10"));
        assert_eq!(merge_config(&Config::default(), &off), off);
        assert_eq!(merge_config(&merged, &merged), merged);
    }

    #[test]
    fn settings_come_across_from_every_copy() {
        let t = Temp::new("settings");
        let roaming = t.0.join("AppData").join("Roaming").join("lane");
        let packages = t.0.join("AppData").join("Local").join("Packages");
        let copy = packages
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join("Roaming")
            .join("lane");
        fs::create_dir_all(&roaming).unwrap();
        fs::create_dir_all(&copy).unwrap();
        // In the terminal: the welcome done, the install count switched off.
        Config {
            onboarded: Some("0.0.9".into()),
            telemetry: false,
            ..Config::default()
        }
        .save_to(&roaming.join("config.toml"))
        .unwrap();
        fs::write(roaming.join("install.json"), r#"{"id":"terminal"}"#).unwrap();
        fs::write(
            roaming.join("agent-settings.json"),
            r#"{"C:\\a\\settings.json":{"made_file":true}}"#,
        )
        .unwrap();
        // In the app: the count on, and a note of its own.
        Config::default()
            .save_to(&copy.join("config.toml"))
            .unwrap();
        fs::write(copy.join("install.json"), r#"{"id":"app"}"#).unwrap();
        fs::write(
            copy.join("agent-settings.json"),
            r#"{"C:\\a\\settings.json":{},"C:\\b\\settings.json":{"made_file":true}}"#,
        )
        .unwrap();
        let old = Old {
            folder: roaming.clone(),
            copies: package_copies(&packages, "Roaming"),
        };
        let to = t.0.join("home").join(".lane");
        let moved = move_settings(&to, &old);
        assert!(moved.failed.is_empty(), "{:?}", moved.failed);
        assert_eq!(moved.from, vec![roaming.clone(), copy]);
        let config = Config::load_from(&to.join("config.toml")).unwrap();
        assert!(!config.telemetry, "switched off in one place stays off");
        assert_eq!(config.onboarded.as_deref(), Some("0.0.9"));
        assert!(
            fs::read_to_string(to.join("install.json"))
                .unwrap()
                .contains("terminal"),
            "the old folder's install id"
        );
        let notes: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(to.join("agent-settings.json")).unwrap())
                .unwrap();
        assert_eq!(notes["C:\\a\\settings.json"]["made_file"], true);
        assert_eq!(notes["C:\\b\\settings.json"]["made_file"], true);

        // A setting changed after the move holds: the old folders haven't changed.
        let mut config = config;
        config.hints = false;
        config.save_to(&to.join("config.toml")).unwrap();
        assert!(!worth_a_look(&to, &roaming, settings_look));
        assert!(move_settings(&to, &old).from.is_empty());
        assert!(!Config::load_from(&to.join("config.toml")).unwrap().hints);
    }

    #[test]
    fn a_report_says_what_came_and_what_didnt() {
        assert_eq!(Report::default().summary(), None);
        let report = Report {
            state: Some(StateMoved {
                to: PathBuf::from("C:/Users/you/.lane/state"),
                from: vec![(
                    PathBuf::from("C:/Users/you/AppData/Local/lane"),
                    TakenIn::default(),
                )],
                ..StateMoved::default()
            }),
            settings: Some(SettingsMoved {
                to: PathBuf::from("C:/Users/you/.lane"),
                ..SettingsMoved::default()
            }),
        };
        let line = report.summary().unwrap();
        assert!(line.contains(".lane now, not AppData (0 lanes"), "{line}");
        assert!(!line.contains("state now"), "{line}");
        let failed = Report {
            state: Some(StateMoved {
                to: PathBuf::from("C:/Users/you/.lane/state"),
                failed: vec![(PathBuf::from("old"), "busy".into())],
                ..StateMoved::default()
            }),
            settings: None,
        };
        assert!(failed.summary().unwrap().contains("lane doctor"));
    }
}
