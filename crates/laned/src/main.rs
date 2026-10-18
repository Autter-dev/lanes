//! `laned`: the Lanes background daemon. One per user (per state folder); the terminal app and
//! `lane daemon start` start it, and it leaves after half an hour with nobody connected.
//!
//! For now it serves what's read most: status, the lanes, and live snapshots for the terminal
//! app, built once per repo however many apps watch, and sent only when something changed.
//! It keeps nothing of its own. Everything it knows is in Lanes' state, so killing it loses
//! nothing. See `lane_core::daemon` for the messages.

use interprocess::local_socket::RecvHalf;
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ListenerOptions, Stream};
use lane_core::daemon::{self, Endpoint, PROTOCOL, Reply, Request, Status};
use lane_core::snapshot::{self, Snapshot, Source};
use lane_core::state::Store;
use lane_core::{VERSION, exit, log};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HELP: &str = "\
laned: the Lanes background daemon

Usage: laned [--version]

One runs per user. The terminal app and `lane daemon start` start it, and it leaves by itself
after half an hour with nobody connected (LANED_IDLE_SECS changes that). `lane daemon status`
shows whether it's running; `lane daemon stop` stops it. Run on its own, it stays in the
foreground until stopped.
";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("-V" | "--version") => {
            println!("laned {VERSION} (by Autter)");
            ExitCode::from(exit::OK)
        }
        Some("-h" | "--help") => {
            print!("{HELP}");
            ExitCode::from(exit::OK)
        }
        Some(other) => {
            eprintln!("laned: unknown option `{other}`. Run `laned --help` to see the options.");
            ExitCode::from(exit::USAGE)
        }
        None => match serve() {
            Ok(()) => ExitCode::from(exit::OK),
            Err(message) => {
                log::error("laned", message.clone()).write();
                eprintln!("laned: {message}");
                ExitCode::from(exit::FAILURE)
            }
        },
    }
}

/// What every connection shares.
struct Daemon {
    started: i64,
    clients: AtomicUsize,
    watching: AtomicUsize,
    last_seen: Mutex<Instant>,
    feeds: Mutex<HashMap<Option<PathBuf>, Arc<Feed>>>,
}

/// One repo's snapshots, refreshed by one thread for every app watching it.
struct Feed {
    last: Mutex<Option<Snapshot>>,
    watchers: Mutex<Vec<(u64, Sender<Snapshot>)>>,
}

static NEXT_WATCHER: AtomicU64 = AtomicU64::new(1);

fn serve() -> Result<(), String> {
    let at = Endpoint::current().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(at.state_dir())
        .map_err(|e| format!("couldn't create {}: {e}", at.state_dir().display()))?;
    // One daemon per state folder: whoever holds the lock serves. A crash lets go of it.
    let lock = File::create(at.lock_path())
        .map_err(|e| format!("couldn't open {}: {e}", at.lock_path().display()))?;
    if lock.try_lock().is_err() {
        eprintln!("laned is already running.");
        return Ok(());
    }
    // The lock is ours, so any socket file left behind is a crashed daemon's: take it over.
    let listener = ListenerOptions::new()
        .name(at.name().map_err(|e| e.to_string())?)
        .try_overwrite(true)
        .create_sync()
        .map_err(|e| format!("couldn't listen at {}: {e}", at.describe()))?;
    log::set_quiet(true);
    log::info(
        "laned",
        format!("started, pid {}, at {}", std::process::id(), at.describe()),
    )
    .write();

    let daemon = Arc::new(Daemon {
        started: unix_now(),
        clients: AtomicUsize::new(0),
        watching: AtomicUsize::new(0),
        last_seen: Mutex::new(Instant::now()),
        feeds: Mutex::new(HashMap::new()),
    });
    let idle = Duration::from_secs(
        std::env::var("LANED_IDLE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30 * 60),
    );
    {
        let daemon = daemon.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500).min(idle));
                let quiet_for = daemon.last_seen.lock().unwrap().elapsed();
                if daemon.clients.load(Ordering::SeqCst) == 0 && quiet_for >= idle {
                    log::info("laned", "nobody connected for a while: stopped").write();
                    std::process::exit(0);
                }
            }
        });
    }
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let daemon = daemon.clone();
        std::thread::spawn(move || serve_one(conn, &daemon));
    }
    drop(lock);
    Ok(())
}

fn serve_one(conn: Stream, daemon: &Arc<Daemon>) {
    if daemon::same_user(&conn).is_err() {
        return;
    }
    daemon.clients.fetch_add(1, Ordering::SeqCst);
    touch(daemon);
    let (recv, mut send) = conn.split();
    let mut reader = BufReader::new(recv);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        touch(daemon);
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(r) => r,
            Err(e) => {
                let message = format!("couldn't read that request ({e}); this is laned {VERSION}");
                if daemon::write_line(&mut send, &Reply::Error { message }).is_err() {
                    break;
                }
                continue;
            }
        };
        let reply = match request {
            Request::Hello { .. } => Reply::Hello {
                version: VERSION.into(),
                protocol: PROTOCOL,
                pid: std::process::id(),
            },
            Request::Status => Reply::Status(Status {
                version: VERSION.into(),
                pid: std::process::id(),
                started: daemon.started,
                clients: daemon.clients.load(Ordering::SeqCst),
                watching: daemon.watching.load(Ordering::SeqCst),
            }),
            Request::Lanes { repo } => match Store::open_existing() {
                Ok(Some(store)) => match store.lanes(repo.as_deref()) {
                    Ok(lanes) => Reply::Lanes { lanes },
                    Err(e) => Reply::Error {
                        message: e.to_string(),
                    },
                },
                Ok(None) => Reply::Lanes { lanes: Vec::new() },
                Err(e) => Reply::Error {
                    message: e.to_string(),
                },
            },
            Request::Snapshot { repo } => Reply::Snapshot(Box::new(read(repo.as_deref()))),
            Request::Watch { repo } => {
                watch(daemon, repo, &mut send, reader);
                break;
            }
            Request::Shutdown => {
                let _ = daemon::write_line(&mut send, &Reply::Bye);
                log::info("laned", "stopped on request").write();
                std::process::exit(0);
            }
        };
        if daemon::write_line(&mut send, &reply).is_err() {
            break;
        }
    }
    daemon.clients.fetch_sub(1, Ordering::SeqCst);
    touch(daemon);
}

/// Send snapshots of `repo` to this client until it hangs up. A watcher sends nothing more, so
/// its side of the connection reaching its end means it's gone, even when nothing changes.
fn watch(
    daemon: &Arc<Daemon>,
    repo: Option<PathBuf>,
    send: &mut impl std::io::Write,
    mut reader: BufReader<RecvHalf>,
) {
    daemon.watching.fetch_add(1, Ordering::SeqCst);
    let (id, updates) = subscribe(daemon, repo.clone());
    let gone = Arc::new(AtomicBool::new(false));
    {
        let gone = gone.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            while matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
                line.clear();
            }
            gone.store(true, Ordering::SeqCst);
        });
    }
    loop {
        match updates.recv_timeout(Duration::from_millis(250)) {
            Ok(snapshot) => {
                if daemon::write_line(send, &Reply::Snapshot(Box::new(snapshot))).is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) if !gone.load(Ordering::SeqCst) => {}
            Err(_) => break,
        }
    }
    unsubscribe(daemon, &repo, id);
    daemon.watching.fetch_sub(1, Ordering::SeqCst);
}

/// Join `repo`'s feed, starting it if nobody watches that repo yet. The latest snapshot comes at
/// once, then each change.
fn subscribe(daemon: &Arc<Daemon>, repo: Option<PathBuf>) -> (u64, Receiver<Snapshot>) {
    let (tx, rx) = mpsc::channel();
    let mut feeds = daemon.feeds.lock().unwrap();
    let feed = feeds.entry(repo.clone()).or_insert_with(|| {
        let feed = Arc::new(Feed {
            last: Mutex::new(None),
            watchers: Mutex::new(Vec::new()),
        });
        // It waits for this lock, so the feed is in the map by the time it looks.
        let (d, f) = (daemon.clone(), feed.clone());
        std::thread::spawn(move || refresh(&d, &f, repo));
        feed
    });
    if let Some(last) = feed.last.lock().unwrap().clone() {
        let _ = tx.send(last);
    }
    let id = NEXT_WATCHER.fetch_add(1, Ordering::SeqCst);
    feed.watchers.lock().unwrap().push((id, tx));
    (id, rx)
}

/// Leave `repo`'s feed. The last watcher to leave stops it.
fn unsubscribe(daemon: &Daemon, repo: &Option<PathBuf>, id: u64) {
    let mut feeds = daemon.feeds.lock().unwrap();
    if let Some(feed) = feeds.get(repo) {
        let mut watchers = feed.watchers.lock().unwrap();
        watchers.retain(|(w, _)| *w != id);
        if watchers.is_empty() {
            drop(watchers);
            feeds.remove(repo);
        }
    }
}

/// Keep one repo's feed current: read it every second and send it only when it changed. Stops
/// once it's no longer the repo's feed, which happens when its last watcher leaves.
fn refresh(daemon: &Daemon, feed: &Arc<Feed>, repo: Option<PathBuf>) {
    loop {
        let current = daemon
            .feeds
            .lock()
            .unwrap()
            .get(&repo)
            .is_some_and(|f| Arc::ptr_eq(f, feed));
        if !current {
            return;
        }
        let snapshot = read(repo.as_deref());
        let changed = feed.last.lock().unwrap().as_ref() != Some(&snapshot);
        if changed {
            *feed.last.lock().unwrap() = Some(snapshot.clone());
            feed.watchers
                .lock()
                .unwrap()
                .retain(|(_, w)| w.send(snapshot.clone()).is_ok());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn read(repo: Option<&std::path::Path>) -> Snapshot {
    Snapshot {
        source: Source::Daemon,
        ..snapshot::read(repo)
    }
}

fn touch(daemon: &Daemon) {
    *daemon.last_seen.lock().unwrap() = Instant::now();
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
