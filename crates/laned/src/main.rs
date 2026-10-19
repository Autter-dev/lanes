//! `laned`: the Lanes background daemon. One per user (per state folder); the terminal app and
//! `lane daemon start` start it, and it leaves after half an hour with nobody connected.
//!
//! It's the one writer: `lane new`, `rm` and `adopt` (and the hooks and terminal app that make
//! and remove lanes) send their changes here when it's running, and it makes them one at a time
//! per repo, sending install output back as it happens.
//!
//! Each lane it knows has an actor: a thread with a mailbox that owns the lane's folder, ports,
//! environment and the processes `lane exec` runs in it, and does one thing at a time. Writes to
//! one lane (`rm`, ready) go through its actor, which stops the lane's processes before `rm`
//! removes the folder. `new` and `adopt` start one, so does a lane's first message, and `rm`
//! ends it.
//!
//! It also serves status, the lanes, and live snapshots for the terminal app, built once per
//! repo however many apps watch, and sent only when something changed. It keeps nothing of its
//! own. Everything it knows is in Lanes' state (what runs in each lane included), so killing it
//! loses nothing. See `lane_core::daemon` for the messages.

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ListenerOptions, Stream};
use interprocess::local_socket::{RecvHalf, SendHalf};
use lane_core::daemon::{self, Endpoint, LaneView, PROTOCOL, Reply, Request, Status};
use lane_core::engine::{self, Context, Sink};
use lane_core::error::{Error, ErrorKind};
use lane_core::running::{Running, Runs};
use lane_core::snapshot::{self, Snapshot, Source};
use lane_core::state::{Lane, Store};
use lane_core::writes::{self, Done, Write};
use lane_core::{VERSION, exit, lane_env, log, process};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SendError, Sender};
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
    writes: AtomicUsize,
    last_seen: Mutex<Instant>,
    feeds: Mutex<HashMap<Option<PathBuf>, Arc<Feed>>>,
    /// One write at a time per repo: Git and the lanes folder see a single writer.
    writers: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
    /// Each lane's actor's mailbox, by lane id.
    actors: Mutex<HashMap<String, Sender<Msg>>>,
}

/// One repo's snapshots, refreshed by one thread for every app watching it.
struct Feed {
    last: Mutex<Option<Snapshot>>,
    watchers: Mutex<Vec<(u64, Sender<Snapshot>)>>,
}

/// What a lane's actor is asked to do.
enum Msg {
    /// A write to the lane (`rm`, ready), answered on `reply`.
    Write {
        write: Write,
        env: Vec<(String, String)>,
        send: Arc<Mutex<SendHalf>>,
        reply: Sender<Reply>,
    },
    /// `lane exec` started a process in the lane.
    Started(Running),
    /// That process ended.
    Exited(u32),
    /// What the actor holds.
    Show(Sender<LaneView>),
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
        writes: AtomicUsize::new(0),
        last_seen: Mutex::new(Instant::now()),
        feeds: Mutex::new(HashMap::new()),
        writers: Mutex::new(HashMap::new()),
        actors: Mutex::new(HashMap::new()),
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
    let (recv, send) = conn.split();
    // Shared, so install output can be sent from the threads that read it.
    let send = Arc::new(Mutex::new(send));
    let mut reader = BufReader::new(recv);
    // Processes this client started in lanes. Any it leaves without saying they ended are
    // forgotten: their records are free then, so nothing may signal those process ids any more.
    let mut started: Vec<(String, u32)> = Vec::new();
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
                if daemon::write_line(&mut *send.lock().unwrap(), &Reply::Error { message })
                    .is_err()
                {
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
                writes: daemon.writes.load(Ordering::SeqCst),
                lanes: daemon.actors.lock().unwrap().len(),
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
                watch(daemon, repo, &mut *send.lock().unwrap(), reader);
                break;
            }
            Request::Write { write, env } => write_to(daemon, write, env, &send),
            Request::Started { lane, process } => {
                let pid = process.pid;
                match tell(daemon, &lane, Msg::Started(process), || find(&lane)) {
                    Ok(()) => {
                        started.push((lane, pid));
                        Reply::Ok
                    }
                    Err(message) => Reply::Error { message },
                }
            }
            Request::Exited { lane, pid } => {
                started.retain(|(l, p)| !(*l == lane && *p == pid));
                tell_if_there(daemon, &lane, Msg::Exited(pid));
                Reply::Ok
            }
            Request::Lane { id } => {
                let (tx, rx) = mpsc::channel();
                match tell(daemon, &id, Msg::Show(tx), || find(&id)) {
                    Ok(()) => match rx.recv() {
                        Ok(view) => Reply::Lane(Box::new(view)),
                        Err(_) => Reply::Error {
                            message: format!("no lane with id `{id}`"),
                        },
                    },
                    Err(message) => Reply::Error { message },
                }
            }
            Request::Shutdown => {
                let _ = daemon::write_line(&mut *send.lock().unwrap(), &Reply::Bye);
                log::info("laned", "stopped on request").write();
                std::process::exit(0);
            }
        };
        if daemon::write_line(&mut *send.lock().unwrap(), &reply).is_err() {
            break;
        }
    }
    for (lane, pid) in started {
        tell_if_there(daemon, &lane, Msg::Exited(pid));
    }
    daemon.clients.fetch_sub(1, Ordering::SeqCst);
    touch(daemon);
}

/// A write for a client. One to a lane the state knows (`rm`, ready) goes through that lane's
/// actor; a lane that's new (`new`, `adopt`) gets an actor.
fn write_to(
    daemon: &Arc<Daemon>,
    write: Write,
    env: Vec<(String, String)>,
    send: &Arc<Mutex<SendHalf>>,
) -> Reply {
    let Some(lane) = lane_of(&write) else {
        let reply = make(daemon, &write, env, send);
        if let Reply::Done(done) = &reply {
            let made = match &**done {
                Done::Made(made) => Some(&made.lane),
                Done::Adopted { lane } => Some(lane),
                Done::Removed(_) | Done::Ready(_) => None,
            };
            if let Some(lane) = made {
                let mut actors = daemon.actors.lock().unwrap();
                if !actors.contains_key(&lane.id) {
                    actors.insert(lane.id.clone(), start_actor(daemon, lane.clone()));
                }
            }
        }
        return reply;
    };
    let (tx, rx) = mpsc::channel();
    let id = lane.id.clone();
    let msg = Msg::Write {
        write,
        env,
        send: send.clone(),
        reply: tx,
    };
    // The lane is in hand, so this can't fail.
    let _ = tell(daemon, &id, msg, || Ok(lane));
    rx.recv().unwrap_or_else(|_| {
        Reply::Failed(
            Error::new(
                ErrorKind::Failure,
                "laned's actor for this lane stopped before it answered",
            )
            .hint("Run `lane list` to see where it got to, then try again. With `--request-id`, repeating it is safe."),
        )
    })
}

/// The lane a write changes, when the state knows it: the lane of `rm` and of ready.
fn lane_of(write: &Write) -> Option<Lane> {
    let store = Store::open_existing().ok()??;
    match write {
        Write::Remove { repo, name, .. } => store.lane(repo, name).ok()?,
        Write::Ready { path, .. } => store.lane_at(path).ok()?,
        Write::New { .. } | Write::Adopt { .. } => None,
    }
}

/// The lane with id `id`, from the state.
fn find(id: &str) -> Result<Lane, String> {
    let missing = || format!("no lane with id `{id}`; run `lane list` to see the lanes");
    let store = Store::open_existing()
        .map_err(|e| e.to_string())?
        .ok_or_else(missing)?;
    store
        .lanes(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|l| l.id == id)
        .ok_or_else(missing)
}

/// Send `msg` to the actor of the lane with id `id`. A lane without one (or whose actor died)
/// gets a new one, for the lane `lane` finds.
fn tell(
    daemon: &Arc<Daemon>,
    id: &str,
    msg: Msg,
    lane: impl FnOnce() -> Result<Lane, String>,
) -> Result<(), String> {
    let mut actors = daemon.actors.lock().unwrap();
    // An actor leaves only after taking itself out of this map, under this lock, so a mailbox
    // found here is open unless its actor panicked.
    let msg = match actors.get(id) {
        Some(mailbox) => match mailbox.send(msg) {
            Ok(()) => return Ok(()),
            Err(SendError(msg)) => msg,
        },
        None => msg,
    };
    let mailbox = start_actor(daemon, lane()?);
    let _ = mailbox.send(msg);
    actors.insert(id.to_string(), mailbox);
    Ok(())
}

/// Send `msg` to the lane's actor, if it has one.
fn tell_if_there(daemon: &Daemon, id: &str, msg: Msg) {
    if let Some(mailbox) = daemon.actors.lock().unwrap().get(id) {
        let _ = mailbox.send(msg);
    }
}

fn start_actor(daemon: &Arc<Daemon>, lane: Lane) -> Sender<Msg> {
    let (tx, rx) = mpsc::channel();
    let daemon = daemon.clone();
    std::thread::spawn(move || Actor::new(lane).run(&daemon, rx));
    tx
}

/// A lane's actor: the lane's folder and ports, the environment its commands get, and the
/// processes running in it. It does one thing at a time.
struct Actor {
    lane: Lane,
    env: Vec<(String, String)>,
    /// What a `lane exec` that's still connected said it started: safe to signal.
    processes: Vec<Running>,
    /// The records `lane exec` keeps, which also cover commands started while laned wasn't
    /// running.
    runs: Option<Runs>,
    /// The lane has been removed: the actor answers what's left in its mailbox, then ends.
    gone: bool,
}

impl Actor {
    fn new(lane: Lane) -> Actor {
        let env = lane_env::vars(&lane)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        Actor {
            lane,
            env,
            processes: Vec::new(),
            runs: Runs::current().ok(),
            gone: false,
        }
    }

    fn run(mut self, daemon: &Arc<Daemon>, mailbox: Receiver<Msg>) {
        while let Ok(msg) = mailbox.recv() {
            self.handle(daemon, msg);
            if self.gone {
                // It left the map, under its lock, before answering the `rm`: nothing is sent
                // here after that, and whatever was sent before is answered now.
                while let Ok(msg) = mailbox.try_recv() {
                    self.handle(daemon, msg);
                }
                return;
            }
        }
    }

    /// Do one thing.
    fn handle(&mut self, daemon: &Arc<Daemon>, msg: Msg) {
        match msg {
            Msg::Started(process) => {
                self.processes.retain(|p| p.pid != process.pid);
                self.processes.push(process);
            }
            Msg::Exited(pid) => self.processes.retain(|p| p.pid != pid),
            Msg::Show(reply) => {
                let _ = reply.send(self.view());
            }
            Msg::Write {
                write,
                env,
                send,
                reply,
            } => {
                // The lane's processes stop before its folder goes.
                let stopped = match write {
                    Write::Remove { .. } => self.stop(),
                    _ => Vec::new(),
                };
                let mut answer = make(daemon, &write, env, &send);
                if let Reply::Done(done) = &mut answer
                    && let Done::Removed(r) = &mut **done
                {
                    for p in stopped {
                        if !r.stopped.iter().any(|q| q.pid == p.pid) {
                            r.stopped.push(p);
                        }
                    }
                    if !self.gone {
                        // Out of the map before answering, so whoever asks next finds no actor.
                        self.gone = true;
                        daemon.actors.lock().unwrap().remove(&self.lane.id);
                    }
                }
                let _ = reply.send(answer);
            }
        }
    }

    /// What runs in the lane: what `lane exec` said, and what its records say.
    fn running(&self) -> Vec<Running> {
        let mut all = self.processes.clone();
        for p in self.runs.iter().flat_map(|r| r.list(&self.lane.id)) {
            if !all.iter().any(|q| q.pid == p.pid) {
                all.push(p);
            }
        }
        all
    }

    fn view(&self) -> LaneView {
        LaneView {
            lane: self.lane.clone(),
            env: self.env.clone(),
            processes: self.running(),
        }
    }

    /// Stop everything running in the lane: asked first, forced after a short wait. Returns
    /// what was running.
    fn stop(&mut self) -> Vec<Running> {
        let all = self.running();
        let told: Vec<u32> = self.processes.drain(..).map(|p| p.pid).collect();
        match &self.runs {
            Some(runs) => {
                runs.stop(&self.lane.id, &told);
            }
            None => {
                process::system().stop(&told, process::GRACE);
            }
        }
        all
    }
}

/// Make a write for a client: one at a time per repo, with the client's environment for installs,
/// and their output sent back as it comes. It finishes even if the client goes away, so a lane
/// is never left half made.
fn make(
    daemon: &Daemon,
    write: &Write,
    env: Vec<(String, String)>,
    send: &Arc<Mutex<SendHalf>>,
) -> Reply {
    let writer = daemon
        .writers
        .lock()
        .unwrap()
        .entry(write.repo().to_path_buf())
        .or_default()
        .clone();
    let _one_at_a_time = writer.lock().unwrap();
    let to_client = send.clone();
    let sink: Sink = Arc::new(move |stream, line| {
        let reply = Reply::Output {
            stream,
            line: line.to_string(),
        };
        let _ = daemon::write_line(&mut *to_client.lock().unwrap(), &reply);
    });
    let context = Context {
        env: Some(env),
        output: Some(sink),
    };
    daemon.writes.fetch_add(1, Ordering::SeqCst);
    match engine::with_context(context, || writes::run(write)) {
        Ok(done) => Reply::Done(Box::new(done)),
        Err(error) => Reply::Failed(error),
    }
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
