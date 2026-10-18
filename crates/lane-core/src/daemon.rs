//! Talking to `laned`, the daemon each user runs: where it listens, the messages, starting and
//! stopping it, and the client side.
//!
//! It listens on a local socket: a Unix socket next to Lanes' state (a named pipe on Windows), one
//! per state folder, so tests and `LANE_STATE_DIR` get their own. Each side writes one JSON object
//! per line. A client says hello first, with its version and protocol; a daemon of another Lanes
//! version is replaced. On Unix, both ends check the other belongs to the same user.
//!
//! Nothing depends on the daemon: everything it knows is in Lanes' state, so killing it loses
//! nothing, and every caller falls back to reading the state itself.

use crate::VERSION;
use crate::env::Env;
use crate::snapshot::Snapshot;
use crate::state::Lane;
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::traits::RecvHalf as _;
use interprocess::local_socket::{Name, RecvHalf, SendHalf, Stream};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The message format. Bump it when a message changes shape.
pub const PROTOCOL: u32 = 1;

/// What a client asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    /// Always first.
    Hello {
        version: String,
        protocol: u32,
    },
    Status,
    /// This repo's lanes, or every lane.
    Lanes {
        repo: Option<PathBuf>,
    },
    /// What the terminal app shows, once.
    Snapshot {
        repo: Option<PathBuf>,
    },
    /// A snapshot now, then one each time something changes, until the client hangs up.
    Watch {
        repo: Option<PathBuf>,
    },
    /// Stop the daemon (after replying `bye`).
    Shutdown,
}

/// What the daemon answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Hello {
        version: String,
        protocol: u32,
        pid: u32,
    },
    Status(Status),
    Lanes {
        lanes: Vec<Lane>,
    },
    Snapshot(Box<Snapshot>),
    Error {
        message: String,
    },
    Bye,
}

/// How the daemon is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    /// Unix seconds.
    pub started: i64,
    /// Connections open now, this one included.
    pub clients: usize,
    /// Terminal apps (or other clients) following a repo's lanes.
    pub watching: usize,
}

/// Where a daemon for one state folder listens, and the lock that keeps it to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    state_dir: PathBuf,
}

impl Endpoint {
    /// The daemon for Lanes' state (`LANE_STATE_DIR`, or the usual folder).
    pub fn current() -> crate::error::Result<Endpoint> {
        Ok(Endpoint::at(&crate::state::state_dir()?))
    }

    pub fn at(state_dir: &Path) -> Endpoint {
        Endpoint {
            state_dir: state_dir.to_path_buf(),
        }
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Held by the running daemon: a second one sees it taken and leaves.
    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join("laned.lock")
    }

    fn key(&self) -> u64 {
        // FNV-1a: stable across runs and builds, unlike std's hasher.
        self.state_dir
            .to_string_lossy()
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325, |h, b| {
                (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
            })
    }

    /// The Unix socket's path: in the state folder, unless that's too long for a socket address
    /// (about 100 bytes), then in the temporary folder, named after the state folder.
    pub fn socket_path(&self) -> PathBuf {
        let beside = self.state_dir.join("laned.sock");
        if beside.as_os_str().len() <= 100 {
            beside
        } else {
            std::env::temp_dir().join(format!("lanes-{:016x}.sock", self.key()))
        }
    }

    /// The named pipe's name on Windows.
    pub fn pipe_name(&self) -> String {
        format!("lanes-{:016x}", self.key())
    }

    /// Where it listens, the way a person would look for it.
    pub fn describe(&self) -> String {
        if cfg!(windows) {
            format!(r"\\.\pipe\{}", self.pipe_name())
        } else {
            self.socket_path().display().to_string()
        }
    }

    pub fn name(&self) -> io::Result<Name<'static>> {
        #[cfg(windows)]
        {
            use interprocess::local_socket::GenericNamespaced;
            std::ffi::OsString::from(self.pipe_name()).to_ns_name::<GenericNamespaced>()
        }
        #[cfg(not(windows))]
        {
            use interprocess::local_socket::GenericFilePath;
            self.socket_path().to_fs_name::<GenericFilePath>()
        }
    }
}

/// On Unix, the other end must be this user: nobody else's process may talk to this daemon, and
/// this client never trusts someone else's socket.
pub fn same_user(stream: &Stream) -> io::Result<()> {
    #[cfg(unix)]
    {
        let theirs = stream.peer_creds()?.euid();
        // SAFETY: geteuid has no preconditions and can't fail.
        let ours = unsafe { libc::geteuid() };
        if theirs != Some(ours) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the other end of laned's socket belongs to another user",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = stream;
    Ok(())
}

/// Write one message as a line.
pub fn write_line<T: Serialize>(to: &mut impl Write, message: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(message).map_err(io::Error::other)?;
    line.push('\n');
    to.write_all(line.as_bytes())?;
    to.flush()
}

/// A connection to the daemon, past the hello.
pub struct Client {
    recv: BufReader<RecvHalf>,
    send: SendHalf,
    /// The daemon's Lanes version and process id.
    pub version: String,
    pub pid: u32,
    /// [`connect_or_start`] started it for this connection.
    pub started: bool,
}

impl Client {
    /// Connect and say hello. Fails when no daemon answers.
    pub fn connect(at: &Endpoint) -> io::Result<Client> {
        let stream = Stream::connect(at.name()?)?;
        same_user(&stream)?;
        let (recv, send) = stream.split();
        // A daemon that stops answering mustn't hang a command; `watch` lifts this. Unix sockets
        // take a timeout; Windows named pipes can't, so there it's best effort.
        let _ = recv.set_timeout(Some(Duration::from_secs(5)));
        let mut client = Client {
            recv: BufReader::new(recv),
            send,
            version: String::new(),
            pid: 0,
            started: false,
        };
        match client.request(&Request::Hello {
            version: VERSION.into(),
            protocol: PROTOCOL,
        })? {
            Reply::Hello {
                version,
                protocol,
                pid,
            } if protocol == PROTOCOL => {
                client.version = version;
                client.pid = pid;
                Ok(client)
            }
            Reply::Hello { protocol, .. } => Err(io::Error::other(format!(
                "laned speaks protocol {protocol}; this lane speaks {PROTOCOL}"
            ))),
            other => Err(unexpected(&other)),
        }
    }

    pub fn send(&mut self, request: &Request) -> io::Result<()> {
        write_line(&mut self.send, request)
    }

    /// The next message. An error once the daemon has gone.
    pub fn receive(&mut self) -> io::Result<Reply> {
        let mut line = String::new();
        if self.recv.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "laned closed the connection",
            ));
        }
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn request(&mut self, request: &Request) -> io::Result<Reply> {
        self.send(request)?;
        self.receive()
    }

    pub fn status(&mut self) -> io::Result<Status> {
        match self.request(&Request::Status)? {
            Reply::Status(s) => Ok(s),
            other => Err(unexpected(&other)),
        }
    }

    /// Follow a repo's snapshots: one now, then one each time something changes. The iterator
    /// ends when the daemon goes away.
    pub fn watch(mut self, repo: Option<PathBuf>) -> io::Result<impl Iterator<Item = Snapshot>> {
        self.send(&Request::Watch { repo })?;
        let _ = self.recv.get_ref().set_timeout(None);
        Ok(std::iter::from_fn(move || {
            loop {
                match self.receive() {
                    Ok(Reply::Snapshot(s)) => return Some(*s),
                    Ok(_) => continue,
                    Err(_) => return None,
                }
            }
        }))
    }
}

fn unexpected(reply: &Reply) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("laned answered something unexpected: {reply:?}"),
    )
}

/// Where `laned` is: next to the running `lane`, as every install puts it, or on PATH.
pub fn laned_path() -> Option<PathBuf> {
    let name = format!("laned{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file());
    beside.or_else(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(&name))
                .find(|p| p.is_file())
        })
    })
}

/// Whether this process may start `laned` by itself. Never in CI, never from an agent's session
/// (hooks must stay quick and leave nothing running), never with `LANE_DAEMON=0`.
pub fn may_start(env: &Env) -> bool {
    let off = env.var("LANE_DAEMON").is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    });
    !off && !env.is_ci() && env.agent_session().is_none()
}

/// Start `laned` for `at` in the background. It leaves by itself if one is already running.
pub fn start(at: &Endpoint, laned: &Path) -> io::Result<()> {
    let mut cmd = Command::new(laned);
    cmd.env("LANE_STATE_DIR", at.state_dir());
    crate::process::detach(&mut cmd);
    cmd.spawn().map(drop)
}

/// Connect, starting `laned` first when it isn't running. A daemon from another Lanes version
/// is stopped and replaced, so an upgrade takes effect at once.
pub fn connect_or_start(at: &Endpoint, laned: &Path) -> io::Result<Client> {
    match Client::connect(at) {
        Ok(client) if client.version == VERSION => return Ok(client),
        Ok(mut old) => {
            let _ = old.request(&Request::Shutdown);
            drop(old);
            wait_until_gone(at, Duration::from_secs(3));
        }
        Err(_) => {}
    }
    start(at, laned)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match Client::connect(at) {
            Ok(client) => {
                return Ok(Client {
                    started: true,
                    ..client
                });
            }
            Err(e) if Instant::now() > deadline => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("laned didn't start in time ({e})"),
                ));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Stop the daemon if it's running. True when one was.
pub fn stop(at: &Endpoint) -> io::Result<bool> {
    let Ok(mut client) = Client::connect(at) else {
        return Ok(false);
    };
    match client.request(&Request::Shutdown) {
        Ok(Reply::Bye) | Err(_) => {}
        Ok(other) => return Err(unexpected(&other)),
    }
    drop(client);
    if wait_until_gone(at, Duration::from_secs(5)) {
        Ok(true)
    } else {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "laned didn't stop within 5 seconds",
        ))
    }
}

/// Wait until nothing answers at `at`. False if something still does after `timeout`.
fn wait_until_gone(at: &Endpoint, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if Client::connect(at).is_err() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_keep_their_shape() {
        let hello = Request::Hello {
            version: "0.0.7".into(),
            protocol: 1,
        };
        assert_eq!(
            serde_json::to_string(&hello).unwrap(),
            r#"{"method":"hello","version":"0.0.7","protocol":1}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::Watch { repo: None }).unwrap(),
            r#"{"method":"watch","repo":null}"#
        );
        let status = Reply::Status(Status {
            version: "0.0.7".into(),
            pid: 42,
            started: 1,
            clients: 1,
            watching: 0,
        });
        let text = serde_json::to_string(&status).unwrap();
        assert!(
            text.starts_with(r#"{"reply":"status","version":"0.0.7""#),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), status);
        let snap = Reply::Snapshot(Box::default());
        let back: Reply = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn each_state_folder_gets_its_own_daemon() {
        let a = Endpoint::at(Path::new("/home/me/.local/state/lane"));
        let b = Endpoint::at(Path::new("/tmp/lane-test-1/state"));
        assert_ne!(a.pipe_name(), b.pipe_name());
        assert_eq!(
            a.pipe_name(),
            Endpoint::at(Path::new("/home/me/.local/state/lane")).pipe_name()
        );
        assert_eq!(
            a.socket_path(),
            Path::new("/home/me/.local/state/lane").join("laned.sock")
        );
        let deep = Endpoint::at(&Path::new("/tmp").join("x".repeat(120)));
        let path = deep.socket_path();
        assert!(path.as_os_str().len() < 110, "{}", path.display());
        assert!(path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn only_people_start_it() {
        assert!(may_start(&Env::fake(&[], true)));
        assert!(!may_start(&Env::fake(&[("CI", "true")], true)));
        assert!(!may_start(&Env::fake(&[("CLAUDECODE", "1")], true)));
        assert!(!may_start(&Env::fake(&[("LANE_DAEMON", "0")], true)));
        assert!(may_start(&Env::fake(&[("LANE_DAEMON", "1")], true)));
    }
}
