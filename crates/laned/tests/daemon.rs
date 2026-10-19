//! `laned` from the outside: started, asked, watched, killed and stopped, the way `lane` and the
//! terminal app talk to it. Each test has its own state folder, so its own daemon and socket.

use lane_core::VERSION;
use lane_core::daemon::{self, Client, Endpoint, Reply, Request};
use lane_core::state::{NewLane, PortBlock, Status, Store};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn state_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("laned-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A daemon this test started. Killed when the test ends, however it ends.
struct Laned(Child);

impl Drop for Laned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(state: &Path, idle_secs: u64) -> Laned {
    Laned(
        Command::new(env!("CARGO_BIN_EXE_laned"))
            .env("LANE_STATE_DIR", state)
            .env("LANED_IDLE_SECS", idle_secs.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("laned starts"),
    )
}

fn connect(at: &Endpoint) -> Client {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match Client::connect(at) {
            Ok(c) => return c,
            Err(e) if Instant::now() > deadline => panic!("laned never answered: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Wait for a process to end by itself; its exit code, or None if it's still running.
fn exits_within(child: &mut Child, limit: Duration) -> Option<i32> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// Put a lane straight into the state store, the way `lane adopt` would record one.
fn add_lane(state: &Path, name: &str, port: u16) {
    let mut store = Store::open_at(&state.join("state.db")).unwrap();
    let repo = state.join("repo");
    let path = state.join(format!("repo.lanes/{name}"));
    store
        .write(None, |tx| {
            tx.insert_lane(NewLane {
                repo: &repo,
                name,
                path: &path,
                base_commit: None,
                ports: PortBlock {
                    start: port,
                    count: 10,
                },
                agent: Some("claude"),
                status: Status::Ready,
            })
        })
        .unwrap();
}

fn lanes(client: &mut Client) -> Vec<String> {
    match client.request(&Request::Lanes { repo: None }).unwrap() {
        Reply::Lanes { lanes } => lanes.into_iter().map(|l| l.name).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn it_says_hello_and_only_one_runs_per_state_folder() {
    let state = state_dir("hello");
    let at = Endpoint::at(&state);
    let mut first = spawn(&state, 60);
    let mut client = connect(&at);
    assert_eq!(client.version, VERSION);
    assert_eq!(client.pid, first.0.id());
    let status = client.status().unwrap();
    assert!(status.clients >= 1, "{status:?}");

    // A second one sees the first and leaves quietly; the first carries on.
    let mut second = spawn(&state, 60);
    assert_eq!(
        exits_within(&mut second.0, Duration::from_secs(10)),
        Some(0)
    );
    assert_eq!(client.status().unwrap().pid, first.0.id());
    assert!(first.0.try_wait().unwrap().is_none());
}

#[test]
fn watching_sends_a_snapshot_now_and_each_change() {
    let state = state_dir("watch");
    let at = Endpoint::at(&state);
    let _laned = spawn(&state, 60);
    let mut updates = connect(&at).watch(None).unwrap();

    let first = updates.next().expect("a snapshot right away");
    assert!(first.rows.is_empty());
    assert_eq!(first.source, lane_core::snapshot::Source::Daemon);

    add_lane(&state, "fix-auth", 4100);
    let second = updates.next().expect("a snapshot once a lane appears");
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].lane.name, "fix-auth");
    assert!(second.rows[0].gone, "its folder was never made");

    // A second app watching the same lanes gets the latest at once.
    let mut again = connect(&at).watch(None).unwrap();
    assert_eq!(again.next().unwrap().rows.len(), 1);
}

#[test]
fn killing_it_loses_nothing() {
    let state = state_dir("kill");
    let at = Endpoint::at(&state);
    add_lane(&state, "fix-auth", 4100);
    let mut laned = spawn(&state, 60);
    assert_eq!(lanes(&mut connect(&at)), ["fix-auth"]);

    laned.0.kill().unwrap();
    laned.0.wait().unwrap();
    assert!(
        Client::connect(&at).is_err(),
        "nothing answers once it's gone"
    );
    let store = Store::open_existing_at(&state.join("state.db"))
        .unwrap()
        .unwrap();
    assert_eq!(
        store.lanes(None).unwrap().len(),
        1,
        "the state is all still there"
    );

    // A new one starts cleanly where the killed one left its socket, and knows the same.
    let _again = spawn(&state, 60);
    assert_eq!(lanes(&mut connect(&at)), ["fix-auth"]);
}

#[test]
fn it_stops_when_asked_and_when_nobody_is_around() {
    let state = state_dir("stop");
    let at = Endpoint::at(&state);
    let mut laned = spawn(&state, 60);
    assert_eq!(
        connect(&at).request(&Request::Shutdown).unwrap(),
        Reply::Bye
    );
    assert_eq!(exits_within(&mut laned.0, Duration::from_secs(10)), Some(0));

    let idle = state_dir("idle");
    let mut laned = spawn(&idle, 1);
    assert_eq!(
        exits_within(&mut laned.0, Duration::from_secs(15)),
        Some(0),
        "an idle daemon leaves by itself"
    );
}

#[test]
fn connect_or_start_starts_it_once_and_stop_stops_it() {
    let state = state_dir("start");
    let at = Endpoint::at(&state);
    let laned = Path::new(env!("CARGO_BIN_EXE_laned"));
    // Stop it however the test ends.
    struct Stop(Endpoint);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = daemon::stop(&self.0);
        }
    }
    let _stop = Stop(at.clone());

    let first = daemon::connect_or_start(&at, laned).unwrap();
    assert!(first.started);
    let again = daemon::connect_or_start(&at, laned).unwrap();
    assert!(!again.started, "it was running already");
    assert_eq!(again.pid, first.pid);
    drop((first, again));

    assert!(daemon::stop(&at).unwrap(), "it was running");
    assert!(Client::connect(&at).is_err());
    assert!(!daemon::stop(&at).unwrap(), "stopping twice is fine");
}

#[test]
fn a_bad_request_gets_an_answer_not_a_hang_up() {
    use interprocess::local_socket::Stream;
    use interprocess::local_socket::prelude::*;
    use std::io::{BufRead, BufReader, Write};

    let state = state_dir("bad");
    let at = Endpoint::at(&state);
    let _laned = spawn(&state, 60);
    connect(&at);
    let raw = Stream::connect(at.name().unwrap()).unwrap();
    let mut reader = BufReader::new(&raw);
    let mut line = String::new();

    (&raw).write_all(b"{\"method\":\"fly\"}\n").unwrap();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains(r#""reply":"error""#), "{line}");
    assert!(line.contains(VERSION), "says which laned it is: {line}");

    // The same connection still works.
    line.clear();
    (&raw)
        .write_all(
            format!("{{\"method\":\"hello\",\"version\":\"{VERSION}\",\"protocol\":1}}\n")
                .as_bytes(),
        )
        .unwrap();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains(r#""reply":"hello""#), "{line}");
}

// ---------- Writes ----------

/// A Git repo with one commit and a lane.toml whose install runs `install`.
fn repo(state: &Path, install: &[&str]) -> PathBuf {
    let dir = state.join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let list = install
        .iter()
        .map(|c| format!("{c:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("lane.toml"),
        format!("version = 1\n[setup]\ninstall = [{list}]\n"),
    )
    .unwrap();
    std::fs::write(dir.join("a.txt"), "hi\n").unwrap();
    let commit: &[&str] = &[
        "-c",
        "user.name=Lanes Test",
        "-c",
        "user.email=test@lanes.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "first",
    ];
    for args in [&["init", "-q"][..], &["add", "-A"], commit] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(ok.success(), "git {args:?}");
    }
    lane_core::engine::Repo::open(&dir).unwrap().root
}

fn spawn_with(state: &Path, extra: &[(&str, &str)]) -> Laned {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_laned"));
    cmd.env("LANE_STATE_DIR", state)
        .env("LANED_IDLE_SECS", "60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in extra {
        cmd.env(k, v);
    }
    Laned(cmd.spawn().expect("laned starts"))
}

#[test]
fn writes_run_in_laned_with_the_callers_environment_and_stream_their_output() {
    use lane_core::engine::Output;
    use lane_core::error::ErrorKind;
    use lane_core::writes::{Done, Write};

    let state = state_dir("writes");
    let at = Endpoint::at(&state);
    // Set only for laned: installs must run with the caller's environment, which lacks it.
    let _laned = spawn_with(&state, &[("LANED_ONLY", "daemon-env")]);
    let var = if cfg!(windows) {
        "%LANED_ONLY%"
    } else {
        "$LANED_ONLY"
    };
    let root = repo(
        &state,
        &[&format!("echo installing {var}"), "echo on stderr 1>&2"],
    );
    let mut client = connect(&at);
    let mut lines: Vec<(Output, String)> = Vec::new();

    let new = Write::New {
        repo: root.clone(),
        name: "fix-auth".into(),
        agent: Some("claude".into()),
        request_id: None,
    };
    let done = client
        .write(&new, &mut |stream, line| {
            lines.push((stream, line.to_string()))
        })
        .unwrap()
        .unwrap();
    let Done::Made(made) = done else {
        panic!("{done:?}")
    };
    assert_eq!(made.lane.name, "fix-auth");
    assert!(made.base.installed);
    assert!(
        lines
            .iter()
            .any(|(s, l)| *s == Output::Stdout && l.starts_with("installing")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|(s, l)| *s == Output::Stderr && l.contains("on stderr")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|(_, l)| l.contains("daemon-env")),
        "installs use the caller's environment, not laned's: {lines:?}"
    );

    // A worktree another tool made becomes a lane, then gets ready.
    let worktree = state.join("wt");
    let ok = Command::new("git")
        .args(["worktree", "add", "--detach", "-q"])
        .arg(&worktree)
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(ok.success());
    let adopt = Write::Adopt {
        repo: root.clone(),
        path: worktree.clone(),
        name: Some("from-git".into()),
        agent: None,
        request_id: None,
    };
    let lane = client
        .write(&adopt, &mut |_, _| {})
        .unwrap()
        .unwrap()
        .lane()
        .unwrap();
    assert_eq!(lane.name, "from-git");
    let ready = Write::Ready {
        repo: root.clone(),
        path: lane.path.clone(),
    };
    let readied = client.write(&ready, &mut |_, _| {}).unwrap().unwrap();
    assert!(readied.ready().is_ok());

    // Removing works, and the engine's own errors come back as they are.
    let remove = Write::Remove {
        repo: root.clone(),
        name: "fix-auth".into(),
        keep_files: false,
        request_id: None,
    };
    let gone = client
        .write(&remove, &mut |_, _| {})
        .unwrap()
        .unwrap()
        .lane()
        .unwrap();
    assert_eq!(gone.name, "fix-auth");
    let again = client.write(&remove, &mut |_, _| {}).unwrap().unwrap_err();
    assert_eq!(again.kind, ErrorKind::NotFound);
    assert!(again.hint.is_some(), "{again}");

    assert_eq!(client.status().unwrap().writes, 5);
}

#[test]
fn a_write_finishes_even_if_the_caller_hangs_up() {
    use interprocess::local_socket::Stream;
    use interprocess::local_socket::prelude::*;
    use lane_core::writes::Write;
    use std::io::Write as _;

    let state = state_dir("hangup");
    let at = Endpoint::at(&state);
    let _laned = spawn(&state, 60);
    let pause = if cfg!(windows) {
        "ping -n 3 127.0.0.1 > nul"
    } else {
        "sleep 2"
    };
    let root = repo(&state, &[pause]);
    connect(&at);
    {
        // Ask for a lane, then leave at once, while its install is still running.
        let raw = Stream::connect(at.name().unwrap()).unwrap();
        let request = Request::Write {
            write: Write::New {
                repo: root.clone(),
                name: "slow".into(),
                agent: None,
                request_id: None,
            },
            env: std::env::vars().collect(),
        };
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        (&raw).write_all(line.as_bytes()).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let lane = Store::open_existing_at(&state.join("state.db"))
            .unwrap()
            .and_then(|s| s.lane(&root, "slow").unwrap());
        if lane.as_ref().is_some_and(|l| l.status == Status::Ready) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the lane never got made: {lane:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ---------- What runs in a lane ----------

/// `argv` that runs for about `secs` seconds and starts a process of its own: a tree, like a
/// dev server's.
fn runs_for(secs: u32) -> Vec<String> {
    if cfg!(windows) {
        vec![
            "cmd".into(),
            "/C".into(),
            format!("ping -n {} 127.0.0.1 > nul", secs + 1),
        ]
    } else {
        vec!["sh".into(), "-c".into(), format!("sleep {secs}; exit 0")]
    }
}

/// Run `argv` in `lane` the way `lane exec` does, on another thread. Its process id comes back
/// once laned has been told of it; the thread ends with the command.
fn exec_in(
    state: &Path,
    lane: &lane_core::state::Lane,
    argv: Vec<String>,
) -> (u32, std::thread::JoinHandle<std::process::ExitStatus>) {
    let (runs, lane) = (lane_core::running::Runs::at(state), lane.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut cmd = lane_core::lane_env::command(&lane, &argv).unwrap();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let exec = runs.start(&lane, &mut cmd, &argv.join(" "), false).unwrap();
        tx.send(exec.pid()).unwrap();
        exec.wait().unwrap()
    });
    (rx.recv().expect("the command starts"), thread)
}

fn pids(view: &daemon::LaneView) -> Vec<u32> {
    view.processes.iter().map(|p| p.pid).collect()
}

#[test]
fn each_lane_has_an_actor_that_knows_what_runs_in_it_and_rm_stops_that_first() {
    use lane_core::error::ErrorKind;
    use lane_core::running::Runs;
    use lane_core::writes::Write;

    let state = state_dir("actor");
    let at = Endpoint::at(&state);
    let _laned = spawn(&state, 60);
    let root = repo(&state, &[]);
    let mut client = connect(&at);
    let new = Write::New {
        repo: root.clone(),
        name: "web".into(),
        agent: None,
        request_id: None,
    };
    let lane = client
        .write(&new, &mut |_, _| {})
        .unwrap()
        .unwrap()
        .made()
        .unwrap()
        .lane;

    // `new` gave the lane an actor, which holds its folder, ports and environment.
    assert_eq!(client.status().unwrap().lanes, 1);
    let view = client.lane(&lane.id).unwrap();
    assert_eq!((&view.lane.path, view.lane.ports), (&lane.path, lane.ports));
    let port = lane.ports.start.to_string();
    assert!(
        view.env.iter().any(|(k, v)| k == "PORT" && *v == port),
        "{:?}",
        view.env
    );
    assert!(view.processes.is_empty());

    // A command run in it is on its list while it runs, and off it once it ends.
    let (pid, short) = exec_in(&state, &lane, runs_for(2));
    assert_eq!(pids(&client.lane(&lane.id).unwrap()), [pid]);
    assert!(short.join().unwrap().success());
    assert!(
        client.lane(&lane.id).unwrap().processes.is_empty(),
        "the actor's list empties when the command exits"
    );

    // `rm` stops a long one, and what it started, before removing the folder.
    let (pid, long) = exec_in(&state, &lane, runs_for(60));
    assert_eq!(pids(&client.lane(&lane.id).unwrap()), [pid]);
    let started = Instant::now();
    let remove = Write::Remove {
        repo: root.clone(),
        name: "web".into(),
        keep_files: false,
        request_id: None,
    };
    let removed = client
        .write(&remove, &mut |_, _| {})
        .unwrap()
        .unwrap()
        .removed()
        .unwrap();
    assert_eq!(removed.lane.id, lane.id);
    assert_eq!(
        removed.stopped.iter().map(|p| p.pid).collect::<Vec<_>>(),
        [pid]
    );
    assert!(!lane.path.exists(), "the folder is gone");
    let status = long.join().unwrap();
    assert!(!status.success(), "stopped, not finished: {status:?}");
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(Runs::at(&state).list(&lane.id).is_empty());

    // The actor went with the lane.
    assert_eq!(client.status().unwrap().lanes, 0);
    let gone = client.lane(&lane.id).unwrap_err();
    assert_eq!(gone.kind(), std::io::ErrorKind::NotFound, "{gone}");

    // Removing it again is safe: the engine's own answer, with what to do next.
    let again = client.write(&remove, &mut |_, _| {}).unwrap().unwrap_err();
    assert_eq!(again.kind, ErrorKind::NotFound);
    assert!(again.hint.is_some(), "{again}");
}

#[test]
fn a_lane_exec_that_leaves_without_saying_is_forgotten() {
    use interprocess::local_socket::Stream;
    use interprocess::local_socket::prelude::*;
    use lane_core::running::Running;
    use std::io::{BufRead, BufReader, Write as _};

    let state = state_dir("forget");
    let at = Endpoint::at(&state);
    add_lane(&state, "web", 4100);
    let id = Store::open_existing_at(&state.join("state.db"))
        .unwrap()
        .unwrap()
        .lanes(None)
        .unwrap()[0]
        .id
        .clone();
    let _laned = spawn(&state, 60);
    let mut client = connect(&at);
    {
        // Say a process started, then go: the way a killed `lane exec` leaves.
        let raw = Stream::connect(at.name().unwrap()).unwrap();
        let request = Request::Started {
            lane: id.clone(),
            process: Running {
                pid: 999_999,
                command: "npm run dev".into(),
                started: 1,
            },
        };
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        (&raw).write_all(line.as_bytes()).unwrap();
        let mut reply = String::new();
        BufReader::new(&raw).read_line(&mut reply).unwrap();
        assert!(reply.contains(r#""reply":"ok""#), "{reply}");
        assert_eq!(pids(&client.lane(&id).unwrap()), [999_999]);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !client.lane(&id).unwrap().processes.is_empty() {
        assert!(Instant::now() < deadline, "never forgotten");
        std::thread::sleep(Duration::from_millis(50));
    }
    // An unknown lane gets an answer, not a hang.
    assert!(client.lane("ln_nope").is_err());
    let unknown = client.started(
        "ln_nope",
        &Running {
            pid: 1,
            command: "x".into(),
            started: 1,
        },
    );
    assert!(unknown.is_err());
}
