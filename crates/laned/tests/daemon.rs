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
