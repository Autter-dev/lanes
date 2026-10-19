//! The `lane` binary from the outside: what scripts, CI and agents see. Output is never a
//! terminal here, so the full-screen app must never open.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn config_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lane-cli-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn lane(dir: &PathBuf, args: &[&str]) -> Output {
    lane_in(dir, &std::env::temp_dir(), args)
}

/// Run `lane` from `cwd`, with settings and state kept in `dir`.
fn lane_in(dir: &PathBuf, cwd: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lane"))
        .args(args)
        .current_dir(cwd)
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        // Agents' user settings go here, never in the real ~/.claude of whoever runs the tests.
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("LANE_REQUEST_ID")
        .env_remove("DO_NOT_TRACK")
        .env_remove("LANE_TELEMETRY")
        .env_remove("CI")
        .env("LANE_TELEMETRY_URL", NO_COUNT)
        .env("LANE_DAEMON", "0")
        .output()
        .expect("lane runs")
}

/// Where tests' install counts go, unless a test listens for them: a closed local port, so a
/// test run never reaches the real endpoint. Tests also run with `LANE_DAEMON=0`, so nothing
/// starts a daemon that outlives them; a test that wants one starts it.
const NO_COUNT: &str = "http://127.0.0.1:9/";

/// Held by a test that times a command, and by one that starts dozens of shells at once, so
/// the shells don't slow the timed command down.
static TIMING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn timing() -> std::sync::MutexGuard<'static, ()> {
    TIMING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `lane` the way a Claude Code hook does: JSON on stdin, with extra environment.
fn lane_hook(
    dir: &PathBuf,
    cwd: &std::path::Path,
    event: &str,
    input: serde_json::Value,
    env: &[(&str, &std::path::Path)],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lane"));
    cmd.args(["hook", "claude", event])
        .current_dir(cwd)
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("LANE_REQUEST_ID")
        .env_remove("CLAUDE_ENV_FILE")
        .env("LANE_TELEMETRY_URL", NO_COUNT)
        .env("LANE_DAEMON", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("lane runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn bare_lane_in_a_pipe_prints_help() {
    let out = lane(&config_dir("bare"), &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: lane"));
}

#[test]
fn setup_json_reports_every_check_and_changes_nothing() {
    let dir = config_dir("json");
    let out = lane(&dir, &["setup", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let checks = report["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 7);
    assert!(
        checks
            .iter()
            .all(|c| c["status"].is_string() && c["summary"].is_string())
    );
    assert_eq!(report["settings"]["saved"], false);
    assert!(
        !dir.join("config.toml").exists(),
        "--json alone writes nothing"
    );
}

#[test]
fn setup_yes_saves_and_is_safe_to_repeat() {
    let dir = config_dir("yes");
    for _ in 0..2 {
        let out = lane(&dir, &["setup", "--yes", "--telemetry", "off"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(text.contains("telemetry = false"), "{text}");
    assert!(text.contains("onboarded ="), "{text}");
}

#[test]
fn setup_without_a_terminal_prints_plain_text() {
    let out = lane(&config_dir("plain"), &["setup"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Nothing was changed"), "{text}");
    assert!(text.contains("Git"), "{text}");
}

#[test]
fn bad_options_say_what_to_do() {
    let out = lane(
        &config_dir("bad"),
        &["setup", "--telemetry", "maybe", "--yes"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("lane setup --help"));
}

/// A fresh Git repo in a temporary folder.
fn git_repo(name: &str) -> PathBuf {
    // On CI's Btrfs, XFS and ReFS disks, repos and their lanes live on that disk.
    let base = std::env::var_os("LANE_TEST_CLONE_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("lane-cli-repo-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(ok.success(), "git init");
    dir
}

/// A folder that isn't inside any Git repo.
fn not_a_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lane-cli-plain-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout isn't one JSON object ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn list_json_in_an_empty_repo_is_an_empty_list_and_writes_nothing() {
    let dir = config_dir("list-empty");
    let repo = git_repo("list-empty");
    let out = lane_in(&dir, &repo, &["list", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["lanes"], serde_json::json!([]));
    assert!(v["repo"].as_str().unwrap().ends_with("list-empty"), "{v}");
    assert!(!dir.join("state").exists(), "listing creates no state");
}

#[test]
fn list_outside_a_repo_says_what_to_do() {
    let dir = config_dir("list-norepo");
    let plain = not_a_repo("list-norepo");
    let out = lane_in(&dir, &plain, &["list", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert_eq!(v["error"]["code"], "not_a_repo");
    assert_eq!(v["error"]["exit"], 1);
    assert!(v["error"]["hint"].as_str().unwrap().contains("--all"));
    assert!(
        out.stderr.is_empty(),
        "with --json, errors go to stdout only"
    );

    let text = lane_in(&dir, &plain, &["list"]);
    assert_eq!(text.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&text.stderr).contains("lane list:"));

    let all = lane_in(&dir, &plain, &["list", "--all", "--json"]);
    assert_eq!(all.status.code(), Some(0));
    assert_eq!(json(&all)["repo"], serde_json::Value::Null);
}

#[test]
fn unknown_commands_and_options_are_usage_errors() {
    let out = lane(&config_dir("unknown"), &["frobnicate", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["code"], "usage");

    let repo = git_repo("bad-option");
    let out = lane_in(&config_dir("bad-option"), &repo, &["list", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("lane list --help"));

    let out = lane_in(
        &config_dir("bad-id"),
        &repo,
        &["list", "--request-id", "not ok", "--json"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["code"], "usage");
}

/// A repo with one commit on `main`.
fn repo_with_commit(name: &str) -> PathBuf {
    let dir = git_repo(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/app.ts"), "export const app = 1;\n").unwrap();
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=Lanes Test",
            "-c",
            "user.email=test@lanes.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args([
            "-C",
            &dir.to_string_lossy(),
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ])
        .status()
        .unwrap();
    assert!(ok.success());
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=Lanes Test",
            "-c",
            "user.email=test@lanes.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(["-C", &dir.to_string_lossy(), "add", "-A"])
        .status()
        .unwrap();
    assert!(ok.success());
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=Lanes Test",
            "-c",
            "user.email=test@lanes.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(["-C", &dir.to_string_lossy(), "commit", "-q", "-m", "app"])
        .status()
        .unwrap();
    assert!(ok.success());
    dir
}

#[test]
fn new_list_and_rm_work_end_to_end() {
    let dir = config_dir("e2e");
    let repo = repo_with_commit("e2e");

    let out = lane_in(
        &dir,
        &repo,
        &["new", "fix-auth", "--agent", "claude", "--json"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let made = json(&out);
    assert_eq!(made["lane"]["name"], "fix-auth");
    assert_eq!(made["lane"]["status"], "ready");
    assert_eq!(made["lane"]["agent"], "claude");
    let path = PathBuf::from(made["lane"]["path"].as_str().unwrap());
    assert!(path.join("src/app.ts").exists());
    assert!(made["clone"]["files"].as_u64().unwrap() >= 1);
    // What to do next: this repo has no lane.toml, so init comes first, then work in the lane.
    let steps: Vec<&str> = made["next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["run"].as_str().unwrap())
        .collect();
    assert!(steps[0].starts_with("cd "), "{steps:?}");
    assert!(steps[1].starts_with("lane exec fix-auth -- "), "{steps:?}");
    assert_eq!(
        steps[2], "lane init",
        "no lane.toml here, so init comes last"
    );

    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(listed["lanes"].as_array().unwrap().len(), 1);
    // From inside the lane, it's the same repo.
    let inside = json(&lane_in(&dir, &path, &["list", "--json"]));
    assert_eq!(inside["lanes"][0]["name"], "fix-auth");

    // The same name again is a conflict, with a hint.
    let again = lane_in(&dir, &repo, &["new", "fix-auth", "--json"]);
    assert_eq!(again.status.code(), Some(1));
    assert_eq!(json(&again)["error"]["code"], "conflict");

    // --print-path prints just the folder, for agent hooks.
    let out = lane_in(&dir, &repo, &["new", "add-tests", "--print-path"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert_eq!(printed.lines().count(), 1, "{printed}");
    assert!(PathBuf::from(printed.trim()).join("src/app.ts").exists());

    let out = lane_in(&dir, &repo, &["rm", "fix-auth", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let removed = json(&out);
    assert_eq!(removed["removed"]["name"], "fix-auth");
    assert_eq!(removed["next"][0]["run"], "lane list");
    assert!(!path.exists());
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(
        listed["lanes"].as_array().unwrap().len(),
        1,
        "add-tests is left"
    );

    let out = lane_in(&dir, &repo, &["rm", "fix-auth", "--json"]);
    assert_eq!(json(&out)["error"]["code"], "not_found");

    // The last lane gone: the plain output says how to start again.
    let out = lane_in(&dir, &repo, &["rm", "add-tests"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "Next:
"
        ),
        "{text}"
    );
    assert!(text.contains("lane new fix-auth"), "{text}");
}

#[test]
fn five_lanes_at_once_on_a_new_machine_all_get_made() {
    // Five agents asking for lanes at once, before Lanes has any state and with no daemon: each
    // `lane new` opens a store that doesn't exist yet.
    let _quiet = timing();
    for round in 0..3 {
        let dir = config_dir(&format!("five-at-once-{round}"));
        let repo = repo_with_commit(&format!("five-at-once-{round}"));
        let made: Vec<Output> = std::thread::scope(|s| {
            let runs: Vec<_> = (1..=5)
                .map(|n| {
                    let (dir, repo) = (&dir, &repo);
                    s.spawn(move || lane_in(dir, repo, &["new", &format!("gate-{n}"), "--json"]))
                })
                .collect();
            runs.into_iter().map(|r| r.join().unwrap()).collect()
        });
        for out in &made {
            assert_eq!(
                out.status.code(),
                Some(0),
                "round {round}: {}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
        let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
        assert_eq!(listed["lanes"].as_array().unwrap().len(), 5, "{listed}");
    }
}

#[test]
fn savings_count_lanes_after_they_are_removed() {
    let dir = config_dir("savings");
    let repo = repo_with_commit("savings");
    // A tree big enough that what the lane adds is small next to it.
    std::fs::write(repo.join("src/data.bin"), vec![b'd'; 200_000]).unwrap();
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "-q", "-m", "data"]);
    let savings = |cwd: &std::path::Path, extra: &[&str]| {
        let mut args = vec!["savings", "--json"];
        args.extend_from_slice(extra);
        let out = lane_in(&dir, cwd, &args);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        json(&out)
    };

    // Nothing made yet: nothing claimed.
    let none = savings(&repo, &[]);
    assert_eq!(
        (
            none["now"]["lanes"].as_u64(),
            none["total"]["lanes"].as_u64()
        ),
        (Some(0), Some(0))
    );
    let text = String::from_utf8_lossy(&lane_in(&dir, &repo, &["savings"]).stdout).into_owned();
    assert!(text.contains("Nothing yet. Make a lane"), "{text}");

    let made = json(&lane_in(&dir, &repo, &["new", "fix-auth", "--json"]));
    let stats = &made["stats"];
    let bytes = stats["bytes"].as_u64().unwrap();
    assert!(bytes > 0, "{made}");
    let cloned = stats["method"] == "clone";
    // A file the lane adds: on a disk that clones, the only disk it takes.
    let path = PathBuf::from(made["lane"]["path"].as_str().unwrap());
    std::fs::write(path.join("notes.txt"), vec![b'x'; 5_000]).unwrap();

    let here = savings(&repo, &[]);
    assert_eq!(
        here["repo"].as_str().map(PathBuf::from),
        Some(PathBuf::from(made["lane"]["repo"].as_str().unwrap()))
    );
    assert_eq!(here["now"]["measured"], 1);
    assert_eq!(here["now"]["as_worktrees"].as_u64(), Some(bytes));
    // A clone takes what it changed plus each file's own record on the disk.
    let files = stats["files"].as_u64().unwrap();
    let taken = match cloned {
        true => (5_000 + files * lane_core::insight::PER_FILE_BYTES).min(bytes),
        false => bytes,
    };
    assert_eq!(here["now"]["as_lanes"].as_u64(), Some(taken), "{here}");
    assert_eq!(
        (
            here["total"]["lanes"].as_u64(),
            here["total"]["removed"].as_u64()
        ),
        (Some(1), Some(0))
    );

    // Removed, the lane still counts in the total, with the disk it took when it went.
    assert_eq!(
        lane_in(&dir, &repo, &["rm", "fix-auth"]).status.code(),
        Some(0)
    );
    let after = savings(&repo, &[]);
    assert_eq!(after["now"]["lanes"], 0);
    let total = &after["total"];
    assert_eq!(
        (total["lanes"].as_u64(), total["removed"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(total["disk_measured"], 1);
    assert_eq!(total["as_worktrees"].as_u64(), Some(bytes));
    assert_eq!(total["as_lanes"].as_u64(), Some(taken), "{after}");
    assert_eq!(total["not_written"].as_u64(), Some(bytes - taken));

    // Every repo, outside a repo or with --all.
    let elsewhere = not_a_repo("savings-elsewhere");
    let all = savings(&elsewhere, &[]);
    assert!(all["repo"].is_null());
    assert_eq!(all["total"]["lanes"], 1);
    assert_eq!(savings(&repo, &["--all"])["total"]["lanes"], 1);
    let text =
        String::from_utf8_lossy(&lane_in(&dir, &elsewhere, &["savings"]).stdout).into_owned();
    assert!(
        text.starts_with("What lanes saved on this machine"),
        "{text}"
    );
    assert!(text.contains("Since "), "{text}");

    let bad = lane_in(&dir, &repo, &["savings", "--everything"]);
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn new_without_a_name_and_adopt_of_the_repo_are_usage_errors() {
    let dir = config_dir("usage-e2e");
    let repo = repo_with_commit("usage-e2e");
    let out = lane_in(&dir, &repo, &["new", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        json(&out)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("lane new fix-auth")
    );
    let out = lane_in(&dir, &repo, &["adopt", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        json(&out)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("repo itself")
    );
}

/// `argv` that prints the lane's PORT and LANE_NAME, and one that exits with `code`.
fn print_port() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd", "/C", "echo %PORT% %LANE_NAME%"]
    } else {
        vec!["sh", "-c", "echo $PORT $LANE_NAME"]
    }
}
fn exit_with_7() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd", "/C", "exit 7"]
    } else {
        vec!["sh", "-c", "exit 7"]
    }
}

#[test]
fn exec_gives_each_lane_its_own_ports_and_passes_the_exit_code_through() {
    let dir = config_dir("exec");
    let repo = repo_with_commit("exec");
    for name in ["a", "b"] {
        let out = lane_in(&dir, &repo, &["new", name, "--json"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let run = |lane: &str| {
        let mut args = vec!["exec", lane, "--"];
        args.extend(print_port());
        let out = lane_in(&dir, &repo, &args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(run("a"), "4100 a");
    assert_eq!(run("b"), "4110 b");

    let mut args = vec!["exec", "a", "--"];
    args.extend(exit_with_7());
    assert_eq!(
        lane_in(&dir, &repo, &args).status.code(),
        Some(7),
        "the command's own exit code"
    );

    // From inside the lane, no name is needed; --json reports the exit code on stdout.
    let made = json(&lane_in(&dir, &repo, &["list", "--json"]));
    let lane_a = PathBuf::from(made["lanes"][0]["path"].as_str().unwrap());
    let mut args = vec!["exec", "--json", "--"];
    args.extend(exit_with_7());
    let out = lane_in(&dir, &lane_a, &args);
    assert_eq!(out.status.code(), Some(7));
    let v = json(&out);
    assert_eq!(
        (v["lane"].as_str(), v["exit"].as_u64()),
        (Some("a"), Some(7))
    );

    // Outside any lane, without a name, it says what to do.
    let mut args = vec!["exec", "--json", "--"];
    args.extend(print_port());
    let out = lane_in(&dir, &repo, &args);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        json(&out)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("lane exec fix-auth")
    );
}

/// `argv` that runs for a minute and starts a process of its own, like a dev server.
fn a_minute() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd", "/C", "ping -n 61 127.0.0.1 > nul"]
    } else {
        vec!["sh", "-c", "sleep 60; exit 0"]
    }
}

/// Start `lane` from `cwd` without waiting for it, set up the way `lane_in` runs it.
fn lane_started(dir: &PathBuf, cwd: &std::path::Path, args: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_lane"))
        .args(args)
        .current_dir(cwd)
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("LANE_REQUEST_ID")
        .env_remove("DO_NOT_TRACK")
        .env_remove("LANE_TELEMETRY")
        .env_remove("CI")
        .env("LANE_TELEMETRY_URL", NO_COUNT)
        .env("LANE_DAEMON", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("lane runs")
}

/// Poll until `done` holds, or panic with `what` after 15 seconds.
fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn rm_stops_what_lane_exec_runs_in_the_lane_then_removes_it_and_is_safe_to_retry() {
    let dir = config_dir("rm-exec");
    let repo = repo_with_commit("rm-exec");
    let made = json(&lane_in(&dir, &repo, &["new", "web", "--json"]));
    let id = made["lane"]["id"].as_str().unwrap().to_string();
    let path = PathBuf::from(made["lane"]["path"].as_str().unwrap());

    let mut args = vec!["exec", "web", "--"];
    args.extend(a_minute());
    let mut exec = lane_started(&dir, &repo, &args);
    // `lane exec` notes what it runs in Lanes' state while it waits for it.
    let noted = dir.join("state").join("running").join(&id);
    eventually("lane exec never noted its command", || {
        std::fs::read_dir(&noted).is_ok_and(|d| {
            d.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "json"))
        })
    });

    // `rm` stops it, and what it started, before removing the folder. Windows won't delete a
    // folder a process is working in, so the folder going means they've stopped there too.
    let started = std::time::Instant::now();
    let out = lane_in(
        &dir,
        &repo,
        &["rm", "web", "--json", "--request-id", "rm-web"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let removed = json(&out);
    assert_eq!(removed["removed"]["name"], "web");
    let stopped = removed["stopped"].as_array().unwrap();
    assert_eq!(stopped.len(), 1, "{removed}");
    assert!(
        stopped[0]["command"]
            .as_str()
            .unwrap()
            .starts_with(a_minute()[0]),
        "{removed}"
    );
    assert!(!path.exists(), "the folder is gone");
    eventually("lane exec kept waiting", || {
        exec.try_wait().unwrap().is_some()
    });
    assert!(
        !exec.wait().unwrap().success(),
        "its command was stopped, not finished"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    eventually("lane exec left its note behind", || !noted.exists());

    // A retry with the same request id gets the same answer; without one, `rm` says the lane's
    // gone and what to do. Nothing else changes.
    let again = lane_in(
        &dir,
        &repo,
        &["rm", "web", "--json", "--request-id", "rm-web"],
    );
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(json(&again)["removed"]["id"], made["lane"]["id"]);
    let plain = lane_in(&dir, &repo, &["rm", "web", "--json"]);
    assert_eq!(plain.status.code(), Some(1));
    let error = &json(&plain)["error"];
    assert_eq!(error["code"], "not_found");
    assert!(error["hint"].as_str().unwrap().contains("lane list"));
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert!(listed["lanes"].as_array().unwrap().is_empty());
}

#[test]
fn env_prints_the_lanes_variables_for_each_shell() {
    let dir = config_dir("env");
    let repo = repo_with_commit("env");
    assert_eq!(
        lane_in(&dir, &repo, &["new", "web", "--json"])
            .status
            .code(),
        Some(0)
    );
    let v = json(&lane_in(&dir, &repo, &["env", "web", "--json"]));
    assert_eq!(v["vars"]["PORT"], "4100");
    assert_eq!(v["vars"]["LANE_URL"], "http://web.localhost:4100");
    let sh = lane_in(&dir, &repo, &["env", "web", "--shell", "sh"]);
    assert!(String::from_utf8_lossy(&sh.stdout).contains("export PORT='4100'"));
    let bad = lane_in(&dir, &repo, &["env", "web", "--shell", "fish"]);
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn init_finds_the_stack_and_writes_lane_toml_once() {
    let dir = config_dir("init");
    let repo = git_repo("init");
    std::fs::write(repo.join("package-lock.json"), "{}\n").unwrap();
    std::fs::write(
        repo.join("package.json"),
        r#"{"scripts": {"dev": "vite"}, "devDependencies": {"vite": "6"}}"#,
    )
    .unwrap();
    std::fs::write(repo.join(".env.example"), "API_KEY=\n").unwrap();
    // A repo that uses Claude Code, whatever is on this machine's PATH.
    std::fs::create_dir_all(repo.join(".claude")).unwrap();

    let printed = lane_in(&dir, &repo, &["init", "--print"]);
    assert!(String::from_utf8_lossy(&printed.stdout).contains("npm ci"));
    assert!(!repo.join("lane.toml").exists(), "--print writes nothing");

    let out = lane_in(&dir, &repo, &["init", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["written"], true);
    assert_eq!(v["found"]["install"][0], "npm ci");
    assert_eq!(v["found"]["dev"]["command"], "npm run dev -- --port $PORT");
    assert_eq!(v["next"][0]["run"], "lane new fix-auth");
    assert_eq!(v["next"][1]["run"], "claude -w fix-auth");
    assert_eq!(v["next"][2]["run"], "git add lane.toml AGENTS.md");
    let text = std::fs::read_to_string(repo.join("lane.toml")).unwrap();
    assert!(text.contains("template = \".env.example\""), "{text}");
    // The agents: Claude Code's hooks, the Git hook and the AGENTS.md note.
    let set_up: Vec<(&str, bool)> = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["what"].as_str().unwrap(), a["changed"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        set_up,
        [("git_hook", true), ("agents_md", true), ("claude", true)]
    );
    let settings = std::fs::read_to_string(repo.join(".claude/settings.local.json")).unwrap();
    assert!(settings.contains("lane hook claude worktree-create"));
    assert!(
        std::fs::read_to_string(repo.join("AGENTS.md"))
            .unwrap()
            .contains("lane new <name> --print-path")
    );

    // Run again: the file is kept as it is, and the agents are already set up.
    std::fs::write(repo.join("lane.toml"), "version = 1\n# mine\n").unwrap();
    let v = json(&lane_in(&dir, &repo, &["init", "--json"]));
    assert_eq!(
        (v["written"].as_bool(), v["kept_existing"].as_bool()),
        (Some(false), Some(true))
    );
    assert!(
        v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["changed"] == false),
        "{v}"
    );
    assert!(
        std::fs::read_to_string(repo.join("lane.toml"))
            .unwrap()
            .contains("# mine")
    );
    // --force rewrites it.
    let v = json(&lane_in(&dir, &repo, &["init", "--force", "--json"]));
    assert_eq!(v["written"], true);
    assert!(
        !std::fs::read_to_string(repo.join("lane.toml"))
            .unwrap()
            .contains("# mine")
    );
}

#[test]
fn a_lane_gets_a_dotenv_from_the_template() {
    let dir = config_dir("dotenv");
    let repo = repo_with_commit("dotenv");
    std::fs::write(repo.join(".env.example"), "PORT=3000\nAPI_KEY=\n").unwrap();
    std::fs::write(
        repo.join("lane.toml"),
        "version = 1\n[env]\ntemplate = \".env.example\"\n",
    )
    .unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "env"]] {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=Lanes Test",
                "-c",
                "user.email=test@lanes.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .arg("-C")
            .arg(&repo)
            .args(&args)
            .status()
            .unwrap();
        assert!(ok.success());
    }
    let made = json(&lane_in(&dir, &repo, &["new", "api", "--json"]));
    let env_file = PathBuf::from(made["env_file"].as_str().expect("an env file"));
    let text = std::fs::read_to_string(env_file).unwrap();
    assert!(
        text.contains("PORT=4100")
            && text.contains("API_KEY=")
            && text.contains("LANE_URL=http://api.localhost:4100"),
        "{text}"
    );
    assert!(!text.contains("3000"), "{text}");
}

#[test]
fn claude_code_hooks_put_a_session_in_a_lane() {
    let dir = config_dir("claude-hooks");
    let repo = repo_with_commit("claude-hooks");
    std::fs::write(repo.join("lane.toml"), "version = 1\n").unwrap();

    // `claude -w "Fix Auth"`: a lane, and its folder as the only line on stdout.
    let input = serde_json::json!({ "session_id": "s-1", "cwd": repo, "name": "Fix Auth", "hook_event_name": "WorktreeCreate" });
    let out = lane_hook(&dir, &repo, "worktree-create", input.clone(), &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(printed.lines().count(), 1, "{printed}");
    let path = PathBuf::from(printed.trim());
    assert!(path.join("src/app.ts").exists());
    assert!(path.ends_with("fix-auth"));
    // The same name again gets the same lane.
    let again = lane_hook(&dir, &repo, "worktree-create", input, &[]);
    assert_eq!(String::from_utf8_lossy(&again.stdout), printed);
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(listed["lanes"].as_array().unwrap().len(), 1);
    assert_eq!(listed["lanes"][0]["agent"], "claude");

    // The session starts in the lane: its ports go in the env file, and it's told where it is.
    let env_file = dir.join("claude-env.sh");
    let out = lane_hook(
        &dir,
        &path,
        "session-start",
        serde_json::json!({ "cwd": path, "source": "startup", "hook_event_name": "SessionStart" }),
        &[("CLAUDE_ENV_FILE", env_file.as_path())],
    );
    assert_eq!(out.status.code(), Some(0));
    let v = json(&out);
    let context = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("lane `fix-auth`"), "{context}");
    let env = std::fs::read_to_string(&env_file).unwrap();
    assert!(env.contains("export PORT=4100\n"), "{env}");

    // `git worktree add` is turned down; other commands pass without a word.
    let deny = lane_hook(
        &dir,
        &path,
        "pre-tool-use",
        serde_json::json!({ "cwd": path, "tool_name": "Bash", "tool_input": { "command": "git worktree add ../other -b other" } }),
        &[],
    );
    let v = json(&deny);
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("lane new")
    );
    let pass = lane_hook(
        &dir,
        &path,
        "pre-tool-use",
        serde_json::json!({ "cwd": path, "tool_name": "Bash", "tool_input": { "command": "git status" } }),
        &[],
    );
    assert_eq!(pass.status.code(), Some(0));
    assert!(pass.stdout.is_empty());

    // Claude Code is done with it: the lane and its folder go.
    let out = lane_hook(
        &dir,
        &repo,
        "worktree-remove",
        serde_json::json!({ "cwd": repo, "worktree_path": path, "hook_event_name": "WorktreeRemove" }),
        &[],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!path.exists());
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert!(listed["lanes"].as_array().unwrap().is_empty());
}

#[test]
fn hooks_stay_quiet_outside_a_lanes_repo() {
    let dir = config_dir("claude-quiet");
    let repo = repo_with_commit("claude-quiet");
    for (event, input) in [
        (
            "session-start",
            serde_json::json!({ "cwd": repo, "source": "startup" }),
        ),
        (
            "pre-tool-use",
            serde_json::json!({ "cwd": repo, "tool_input": { "command": "git worktree add ../x" } }),
        ),
    ] {
        let out = lane_hook(&dir, &repo, event, input, &[]);
        assert_eq!(out.status.code(), Some(0), "{event}");
        assert!(
            out.stdout.is_empty(),
            "{event}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    // Not JSON: a hook that can't stop the session still exits 0.
    let mut child = Command::new(env!("CARGO_BIN_EXE_lane"))
        .args(["hook", "claude", "pre-tool-use"])
        .env("LANE_CONFIG_DIR", &dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(0));
}

#[test]
fn a_worktree_made_with_git_becomes_a_lane() {
    let dir = config_dir("post-checkout");
    let repo = repo_with_commit("post-checkout");
    let out = lane_in(&dir, &repo, &["init", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Any tool's `git worktree add` runs the post-checkout hook, which runs `lane adopt`.
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_lane"));
    let path = std::env::join_paths(std::iter::once(bin.parent().unwrap().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let other = repo.with_file_name("post-checkout-by-hand");
    let _ = std::fs::remove_dir_all(&other);
    let status = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "worktree",
            "add",
            "--detach",
            "-q",
        ])
        .arg(&other)
        .env("PATH", path)
        .env("LANE_CONFIG_DIR", &dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env_remove("LANE_GIT")
        .status()
        .unwrap();
    assert!(status.success());
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    let names: Vec<&str> = listed["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["post-checkout-by-hand"]);

    // Lanes' own worktrees aren't adopted a second time.
    let out = lane_in(&dir, &repo, &["new", "made-by-lanes", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(listed["lanes"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&other);
}

/// Run Git in `dir`, as a test user.
fn git_in(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=Lanes Test",
            "-c",
            "user.email=test@lanes.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// PATH with the `lane` under test first, so Git hooks find it.
fn path_with_lane() -> std::ffi::OsString {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_lane"));
    std::env::join_paths(std::iter::once(bin.parent().unwrap().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap()
}

#[test]
fn clones_and_copies_of_the_repo_are_turned_down() {
    let dir = config_dir("claude-copies");
    let repo = repo_with_commit("claude-copies");
    std::fs::write(repo.join("lane.toml"), "version = 1\n").unwrap();
    git_in(
        &repo,
        &["remote", "add", "origin", "https://github.com/acme/app.git"],
    );
    let name = repo.file_name().unwrap().to_string_lossy().to_string();
    // The reason a command is turned down, or None when it may run.
    let decide = |tool: &str, command: &str| -> Option<String> {
        let input = serde_json::json!({ "cwd": repo, "tool_name": tool, "tool_input": { "command": command } });
        let out = lane_hook(&dir, &repo, "pre-tool-use", input, &[]);
        assert_eq!(out.status.code(), Some(0), "{command}");
        if out.stdout.is_empty() {
            return None;
        }
        let v = json(&out);
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        Some(
            v["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    };

    // Each kind of copy has its own reason, and each says what to run instead.
    let worktree = decide("Bash", "git worktree add ../x").unwrap();
    let clone = decide("Bash", "git clone . ../app-2").unwrap();
    let copy = decide("Bash", "cp -r . ../app-copy").unwrap();
    for reason in [&worktree, &clone, &copy] {
        assert!(reason.contains("lane new <name> --print-path"), "{reason}");
    }
    assert!(worktree != clone && clone != copy && copy != worktree);

    for (tool, command, want) in [
        (
            "Bash",
            format!("cd .. && git clone {name} {name}-2"),
            &clone,
        ),
        (
            "Bash",
            "git clone git@github.com:acme/app ../app-2".into(),
            &clone,
        ),
        (
            "Bash",
            "git clone \"$(git remote get-url origin)\" ../app-2".into(),
            &clone,
        ),
        ("Bash", format!("cd .. && cp -R {name} {name}-copy"), &copy),
        ("Bash", "robocopy . ../app-copy /E".into(), &copy),
        (
            "PowerShell",
            "Copy-Item -Recurse . ..\\app-copy".into(),
            &copy,
        ),
        ("PowerShell", "cp -r . ..\\app-copy".into(), &copy),
    ] {
        assert_eq!(decide(tool, &command).as_ref(), Some(want), "{command}");
    }
    for command in [
        "git clone https://github.com/acme/other ../other",
        "cp -r src ../src-copy",
        "robocopy . ../app-export /E /XD .git",
        "git status",
    ] {
        assert_eq!(decide("Bash", command), None, "{command}");
    }
}

#[test]
fn with_husky_init_adds_a_line_and_worktrees_still_become_lanes() {
    let dir = config_dir("husky");
    let repo = repo_with_commit("husky");
    // Husky 9's layout: Git runs .husky/_/<hook>, which runs .husky/<hook> with `sh -e`.
    std::fs::create_dir_all(repo.join(".husky/_")).unwrap();
    std::fs::write(repo.join(".husky/_/.gitignore"), "*\n").unwrap();
    std::fs::write(
        repo.join(".husky/_/h"),
        "n=$(basename \"$0\")\ns=$(dirname \"$(dirname \"$0\")\")/$n\n[ ! -f \"$s\" ] && exit 0\nsh -e \"$s\" \"$@\"\n",
    )
    .unwrap();
    let stub = repo.join(".husky/_/post-checkout");
    std::fs::write(&stub, "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/h\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git_in(&repo, &["config", "core.hooksPath", ".husky/_"]);

    // init says it changed a file that's checked in, and to add it.
    let out = lane_in(&dir, &repo, &["init"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("added a line to .husky/post-checkout"),
        "{text}"
    );
    assert!(
        text.contains("git add lane.toml .husky/post-checkout AGENTS.md"),
        "{text}"
    );
    assert!(!repo.join(".git/hooks/post-checkout").exists());
    let hook = std::fs::read_to_string(repo.join(".husky/post-checkout")).unwrap();
    assert!(hook.contains("lane hook git post-checkout"), "{hook}");

    // A second run finds it set up.
    let v = json(&lane_in(&dir, &repo, &["init", "--json"]));
    let git_hook = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["what"] == "git_hook")
        .unwrap();
    assert_eq!(git_hook["changed"], false, "{v}");
    assert_eq!(git_hook["check_in"], true, "{v}");
    assert_eq!(
        std::fs::read_to_string(repo.join(".husky/post-checkout")).unwrap(),
        hook
    );

    // Any tool's `git worktree add` runs Husky's hook, which runs the line, which adopts it.
    let other = repo.with_file_name("husky-by-hand");
    let _ = std::fs::remove_dir_all(&other);
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "add", "--detach", "-q"])
        .arg(&other)
        .env("PATH", path_with_lane())
        .env("LANE_CONFIG_DIR", &dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env_remove("LANE_GIT")
        .status()
        .unwrap();
    assert!(status.success());
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(listed["lanes"][0]["name"], "husky-by-hand", "{listed}");
    let _ = std::fs::remove_dir_all(&other);

    // By hand: a branch switch (a previous HEAD that isn't zeros) or the repo itself is no lane.
    for previous in ["1a2b3c", "0000000000000000000000000000000000000000"] {
        let args = [
            "hook",
            "git",
            "post-checkout",
            previous,
            "4d5e6f",
            "1",
            "--json",
        ];
        let out = lane_in(&dir, &repo, &args);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(json(&out)["lane"], serde_json::Value::Null);
    }
}

#[test]
fn with_lefthook_init_prints_the_command_to_add() {
    let dir = config_dir("lefthook");
    let repo = repo_with_commit("lefthook");
    std::fs::write(
        repo.join("lefthook.yml"),
        "post-checkout:\n  commands:\n    deps:\n      run: npm install\n",
    )
    .unwrap();
    let out = lane_in(&dir, &repo, &["init"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("add the `lanes` command below to post-checkout in lefthook.yml"),
        "{text}"
    );
    assert!(
        text.contains("\n      post-checkout:\n        commands:\n          lanes:\n"),
        "{text}"
    );
    // lefthook owns the hook file, so nothing is written there, and lefthook.yml is untouched.
    assert!(!repo.join(".git/hooks/post-checkout").exists());
    assert!(
        !std::fs::read_to_string(repo.join("lefthook.yml"))
            .unwrap()
            .contains("lanes")
    );

    let v = json(&lane_in(&dir, &repo, &["init", "--json"]));
    let git_hook = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["what"] == "git_hook")
        .unwrap();
    assert_eq!(git_hook["changed"], false);
    assert!(
        git_hook["snippet"]
            .as_str()
            .unwrap()
            .contains("lane hook git post-checkout {0}")
    );
    let add = v["next"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["run"].as_str().is_some_and(|r| r.starts_with("git add")));
    assert!(add.is_none(), "nothing new to check in: {v}");
}

#[test]
fn setup_puts_agents_in_lanes_once_per_machine_and_takes_it_back() {
    let dir = config_dir("agents-setup");
    // An agent's user folder, in a fake home: Claude Code is "installed" here.
    let claude = dir.join("home/.claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("settings.json"),
        "{\"theme\": \"dark\"}
",
    )
    .unwrap();

    let v = json(&lane(&dir, &["setup", "--yes", "--json"]));
    assert_eq!(v["settings"]["agents"], true);
    let found: Vec<(&str, bool)> = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["id"].as_str().unwrap(), a["set_up"].as_bool().unwrap()))
        .collect();
    assert!(found.contains(&("claude", true)), "{v}");
    let settings = std::fs::read_to_string(claude.join("settings.json")).unwrap();
    assert!(settings.contains("lane hook claude worktree-create"));
    assert!(settings.contains("\"theme\": \"dark\""));
    assert!(claude.join("skills/lanes/SKILL.md").is_file());

    // Off: exactly what was added goes, and the setting is remembered.
    let v = json(&lane(
        &dir,
        &["setup", "--yes", "--agents", "off", "--json"],
    ));
    assert_eq!(v["settings"]["agents"], false);
    let settings = std::fs::read_to_string(claude.join("settings.json")).unwrap();
    assert!(!settings.contains("lane hook"), "{settings}");
    assert!(settings.contains("\"theme\": \"dark\""));
    assert!(!claude.join("skills/lanes").exists());

    // --agents needs --yes, like --telemetry.
    assert_eq!(
        lane(&dir, &["setup", "--agents", "on"]).status.code(),
        Some(2)
    );
}

/// Run `lane hook <agent> <event>` with these exact bytes on stdin, from `cwd`.
fn hook_raw(
    dir: &PathBuf,
    cwd: &std::path::Path,
    agent: &str,
    event: &str,
    stdin: &[u8],
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lane"))
        .args(["hook", agent, event])
        .current_dir(cwd)
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env("LANE_TELEMETRY_URL", NO_COUNT)
        .env("LANE_DAEMON", "0")
        .env_remove("LANE_REQUEST_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lane runs");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_cursor_agent_in_its_own_worktree_gets_a_lane() {
    let dir = config_dir("cursor-hooks");
    let repo = repo_with_commit("cursor-hooks");
    std::fs::write(repo.join("lane.toml"), "version = 1\n").unwrap();

    // Cursor makes its worktree with Git, where its hooks may not run, then starts the agent.
    // Cursor puts it at ~/.cursor/worktrees/<repo>/<id>.
    let worktree = repo.with_file_name("cursor-worktrees").join("zjm");
    let _ = std::fs::remove_dir_all(&worktree);
    let ok = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "worktree",
            "add",
            "-q",
            "--detach",
        ])
        .arg(&worktree)
        .env("LANE_GIT", "1")
        .status()
        .unwrap()
        .success();
    assert!(ok);

    // Cursor's sessionStart, from ~/.cursor, with the worktree in workspace_roots. On Windows
    // Cursor starts the JSON with a byte-order mark.
    let input = format!(
        "\u{feff}{}",
        serde_json::json!({ "hook_event_name": "sessionStart", "workspace_roots": [worktree] })
    );
    let out = hook_raw(
        &dir,
        &std::env::temp_dir(),
        "cursor",
        "session-start",
        input.as_bytes(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    let context = v["additional_context"].as_str().unwrap();
    assert!(context.contains("lane `cursor-zjm`"), "{context}");
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(listed["lanes"][0]["agent"], "cursor");

    // A copy of the repo is turned down with a message for the agent; other commands pass.
    let deny = serde_json::json!({ "command": "git worktree add ../another", "cwd": worktree });
    let out = hook_raw(
        &dir,
        &std::env::temp_dir(),
        "cursor",
        "before-shell",
        deny.to_string().as_bytes(),
    );
    let v = json(&out);
    assert_eq!(v["permission"], "deny");
    assert!(v["agent_message"].as_str().unwrap().contains("lane new"));
    let pass = serde_json::json!({ "command": "npm test", "cwd": worktree });
    let out = hook_raw(
        &dir,
        &std::env::temp_dir(),
        "cursor",
        "before-shell",
        pass.to_string().as_bytes(),
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stdout.is_empty(),
        "no answer, so Cursor's own approval rules apply"
    );
    let _ = std::fs::remove_dir_all(worktree.parent().unwrap());
}

#[test]
fn a_codex_session_in_its_own_worktree_gets_a_lane() {
    let dir = config_dir("codex-hooks");
    let repo = repo_with_commit("codex-hooks");
    std::fs::write(
        repo.join("lane.toml"),
        "version = 1
",
    )
    .unwrap();

    // Codex makes `<root>/<4 hex>/<repo folder>` with Git hooks off, twice here: both folders
    // are named after the repo, and each still gets a lane of its own.
    let root = repo.with_file_name("codex-worktrees");
    let _ = std::fs::remove_dir_all(&root);
    let mut names = Vec::new();
    for id in ["a1b2", "c3d4"] {
        let worktree = root.join(id).join("codex-hooks");
        let ok = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-C",
                &repo.to_string_lossy(),
                "worktree",
                "add",
                "-q",
                "--detach",
            ])
            .arg(&worktree)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let input = serde_json::json!({ "hook_event_name": "SessionStart", "source": "startup", "cwd": worktree });
        let out = hook_raw(
            &dir,
            &worktree,
            "codex",
            "session-start",
            input.to_string().as_bytes(),
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json(&out);
        let context = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(context.contains(&format!("lane `codex-{id}`")), "{context}");
        names.push(format!("codex-{id}"));

        // A copy of the repo is turned down; the reason reaches the model.
        let deny = serde_json::json!({ "tool_name": "Bash", "cwd": worktree, "tool_input": { "command": "git worktree add ../x" } });
        let v = json(&hook_raw(
            &dir,
            &worktree,
            "codex",
            "pre-tool-use",
            deny.to_string().as_bytes(),
        ));
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    }
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    let mut got: Vec<String> = listed["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap().to_string())
        .collect();
    got.sort();
    assert_eq!(got, names);
    assert!(
        listed["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["agent"] == "codex")
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A worktree an agent made itself, out of Lanes' sight (its Git hooks don't make it a lane).
fn agent_worktree(repo: &std::path::Path, worktree: &std::path::Path) {
    let _ = std::fs::remove_dir_all(worktree);
    let ok = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "worktree",
            "add",
            "-q",
            "--detach",
        ])
        .arg(worktree)
        .env("LANE_GIT", "1")
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

#[test]
fn setup_puts_gemini_cli_and_copilot_in_lanes_and_off_gives_their_files_back_byte_for_byte() {
    let dir = config_dir("gemini-copilot-setup");
    let home = dir.join("home");
    // Gemini CLI's settings, with a comment, and Copilot's folder: both "installed" here.
    let gemini = home.join(".gemini/settings.json");
    std::fs::create_dir_all(gemini.parent().unwrap()).unwrap();
    let gemini_text = "{\n  // Mine.\n  \"ui\": { \"theme\": \"GitHub\" },\n  \"context\": { \"fileName\": \"CONTEXT.md\" }\n}\n";
    std::fs::write(&gemini, gemini_text).unwrap();
    std::fs::create_dir_all(home.join(".copilot")).unwrap();
    // VS Code's user settings, where this system keeps them.
    let config = if cfg!(windows) {
        home.join("AppData/Roaming")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.join(".config")
    };
    let vscode = config.join("Code/User/settings.json");
    std::fs::create_dir_all(vscode.parent().unwrap()).unwrap();
    let vscode_text = "{\n    \"editor.fontSize\": 14 // big\n}";
    std::fs::write(&vscode, vscode_text).unwrap();

    let v = json(&lane(&dir, &["setup", "--yes", "--json"]));
    let set_up: Vec<(&str, bool)> = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["id"].as_str().unwrap(), a["set_up"].as_bool().unwrap()))
        .collect();
    assert_eq!(set_up, [("gemini", true), ("copilot", true)], "{v}");
    let text = std::fs::read_to_string(&gemini).unwrap();
    assert!(
        text.contains("\"command\": \"lane hook gemini before-tool\""),
        "{text}"
    );
    assert!(text.contains("\"CONTEXT.md\", \"AGENTS.md\"") && text.contains("// Mine."));
    let hooks = std::fs::read_to_string(home.join(".copilot/hooks/lanes.json")).unwrap();
    assert!(hooks.contains("lane hook copilot pre-tool-use"), "{hooks}");
    assert!(
        std::fs::read_to_string(&vscode)
            .unwrap()
            .contains("\"chat.useAgentsMdFile\": true")
    );
    assert!(home.join(".agents/skills/lanes/SKILL.md").is_file());

    // `lane doctor` agrees.
    let doctor = json(&lane(&dir, &["doctor", "--json"]));
    let check = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "agent-hooks")
        .unwrap()
        .clone();
    assert_eq!(check["status"], "ok", "{check}");
    assert!(
        check["summary"]
            .as_str()
            .unwrap()
            .contains("Gemini CLI, GitHub Copilot"),
        "{check}"
    );

    // Off: every file is back exactly as it was, and what Lanes made is gone.
    let v = json(&lane(
        &dir,
        &["setup", "--yes", "--agents", "off", "--json"],
    ));
    assert_eq!(v["settings"]["agents"], false);
    assert_eq!(std::fs::read_to_string(&gemini).unwrap(), gemini_text);
    assert_eq!(std::fs::read_to_string(&vscode).unwrap(), vscode_text);
    assert!(!home.join(".copilot/hooks").exists());
    assert!(!home.join(".agents/skills/lanes").exists());
    assert!(
        !dir.join("agent-settings.json").exists(),
        "nothing left to take back"
    );
}

#[test]
fn a_gemini_cli_session_in_its_own_worktree_gets_a_lane() {
    let dir = config_dir("gemini-hooks");
    let repo = repo_with_commit("gemini-hooks");
    std::fs::write(repo.join("lane.toml"), "version = 1\n").unwrap();
    // `gemini -w fix-auth` works in .gemini/worktrees/fix-auth.
    let worktree = repo.join(".gemini/worktrees/fix-auth");
    agent_worktree(&repo, &worktree);

    // SessionStart, as Gemini CLI's hooks reference documents it.
    let input = serde_json::json!({
        "session_id": "4f1c",
        "transcript_path": "/tmp/gemini/chats/4f1c.json",
        "cwd": worktree,
        "hook_event_name": "SessionStart",
        "timestamp": "2026-10-18T09:30:00.000Z",
        "source": "startup",
    });
    let out = hook_raw(
        &dir,
        &worktree,
        "gemini",
        "session-start",
        input.to_string().as_bytes(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    let context = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("lane `gemini-fix-auth`"), "{context}");
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(listed["lanes"][0]["name"], "gemini-fix-auth");
    assert_eq!(listed["lanes"][0]["agent"], "gemini");

    // BeforeTool on run_shell_command: a copy of the repo is turned down, the reason for the
    // model; anything else gets no answer.
    let tool = |command: &str| {
        serde_json::json!({
            "session_id": "4f1c",
            "transcript_path": "/tmp/gemini/chats/4f1c.json",
            "cwd": worktree,
            "hook_event_name": "BeforeTool",
            "timestamp": "2026-10-18T09:31:00.000Z",
            "tool_name": "run_shell_command",
            "tool_input": { "command": command, "description": "Make a copy" },
        })
        .to_string()
    };
    let out = hook_raw(
        &dir,
        &worktree,
        "gemini",
        "before-tool",
        tool("git worktree add ../x").as_bytes(),
    );
    assert_eq!(out.status.code(), Some(0));
    let v = json(&out);
    assert_eq!(v["decision"], "deny");
    assert!(v["reason"].as_str().unwrap().contains("lane new"), "{v}");
    let out = hook_raw(
        &dir,
        &worktree,
        "gemini",
        "before-tool",
        tool("npm test").as_bytes(),
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "no answer, so Gemini CLI runs it");
}

#[test]
fn a_copilot_session_in_its_own_worktree_gets_a_lane_in_the_cli_and_in_vs_code() {
    let dir = config_dir("copilot-hooks");
    let repo = repo_with_commit("copilot-hooks");
    std::fs::write(repo.join("lane.toml"), "version = 1\n").unwrap();
    let worktree = repo.with_file_name("copilot-worktrees").join("fix-login");
    agent_worktree(&repo, &worktree);

    // The Copilot CLI's sessionStart: camelCase, a timestamp in milliseconds.
    let input = serde_json::json!({
        "sessionId": "c0p1",
        "timestamp": 1_760_779_800_000_u64,
        "cwd": worktree,
        "source": "new",
        "initialPrompt": "Fix the login bug",
    });
    let out = hook_raw(
        &dir,
        &worktree,
        "copilot",
        "session-start",
        input.to_string().as_bytes(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    let context = v["additionalContext"].as_str().unwrap();
    assert!(context.contains("lane `copilot-fix-login`"), "{context}");
    assert_eq!(
        v["hookSpecificOutput"]["additionalContext"], context,
        "VS Code's shape too"
    );

    // The Copilot CLI's preToolUse: `toolArgs` is the tool's arguments as a JSON string. A copy
    // of the repo is turned down with exit 2, the reason for the agent in JSON and on stderr.
    let cli = |command: &str| {
        serde_json::json!({
            "sessionId": "c0p1",
            "timestamp": 1_760_779_860_000_u64,
            "cwd": worktree,
            "toolName": "bash",
            "toolArgs": serde_json::json!({ "command": command, "description": "Copy" }).to_string(),
        })
        .to_string()
    };
    let out = hook_raw(
        &dir,
        &worktree,
        "copilot",
        "pre-tool-use",
        cli("git worktree add ../x").as_bytes(),
    );
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    assert_eq!(v["permissionDecision"], "deny");
    assert!(
        v["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("lane new")
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("lane new"));
    let out = hook_raw(
        &dir,
        &worktree,
        "copilot",
        "pre-tool-use",
        cli("npm test").as_bytes(),
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());

    // VS Code's PreToolUse: snake_case, the terminal tool's input as an object. The model gets
    // stderr on exit 2.
    let vscode = serde_json::json!({
        "timestamp": "2026-10-18T09:32:00.000Z",
        "cwd": worktree,
        "session_id": "vs-1",
        "hook_event_name": "PreToolUse",
        "tool_name": "run_in_terminal",
        "tool_input": { "command": "git worktree add ../y", "explanation": "Copy", "isBackground": false },
        "tool_use_id": "t-1",
    });
    let out = hook_raw(
        &dir,
        &worktree,
        "copilot",
        "pre-tool-use",
        vscode.to_string().as_bytes(),
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("lane new"));
    // A tool that isn't a shell passes without a word.
    let read = serde_json::json!({
        "timestamp": "2026-10-18T09:33:00.000Z",
        "cwd": worktree,
        "hook_event_name": "PreToolUse",
        "tool_name": "read_file",
        "tool_input": { "filePath": "src/app.ts" },
    });
    let out = hook_raw(
        &dir,
        &worktree,
        "copilot",
        "pre-tool-use",
        read.to_string().as_bytes(),
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());

    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(listed["lanes"][0]["agent"], "copilot");
    let _ = std::fs::remove_dir_all(worktree.parent().unwrap());
}

/// The Copilot hooks file Lanes 0.0.8 wrote, on every system.
const COPILOT_0_0_8: &str = r#"{
  "version": 1,
  "hooks": {
    "sessionStart": [
      {
        "type": "command",
        "bash": "lane hook copilot session-start",
        "powershell": "lane hook copilot session-start",
        "timeoutSec": 300
      }
    ],
    "preToolUse": [
      {
        "type": "command",
        "bash": "command -v lane >/dev/null 2>&1 || exit 0; lane hook copilot pre-tool-use",
        "powershell": "lane hook copilot pre-tool-use",
        "timeoutSec": 10
      }
    ]
  }
}
"#;

/// The `agent-hooks` check from `lane doctor --json`.
fn agent_hooks_check(dir: &PathBuf) -> serde_json::Value {
    let doctor = json(&lane(dir, &["doctor", "--json"]));
    doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "agent-hooks")
        .unwrap()
        .clone()
}

/// Whether `lane setup --json` says Copilot is set up.
fn copilot_set_up(dir: &PathBuf, args: &[&str]) -> bool {
    let v = json(&lane(dir, args));
    v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "copilot")
        .unwrap()["set_up"]
        .as_bool()
        .unwrap()
}

#[test]
fn a_machine_set_up_by_0_0_8_gets_copilots_hooks_rewritten_on_windows() {
    let dir = config_dir("copilot-0-0-8");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".copilot")).unwrap();
    assert!(copilot_set_up(&dir, &["setup", "--yes", "--json"]));
    let hooks = home.join(".copilot/hooks/lanes.json");
    std::fs::write(&hooks, COPILOT_0_0_8).unwrap();

    let check = agent_hooks_check(&dir);
    if cfg!(windows) {
        // Its commands run `lane` by name: without it, the Copilot CLI turns down every tool.
        assert!(!copilot_set_up(&dir, &["setup", "--json"]));
        assert_eq!(check["status"], "warn", "{check}");
        assert!(
            check["summary"].as_str().unwrap().contains("Lanes 0.0.8"),
            "{check}"
        );
        assert!(
            check["hint"]
                .as_str()
                .unwrap()
                .contains("`lane setup --yes`"),
            "{check}"
        );
        assert_eq!(std::fs::read_to_string(&hooks).unwrap(), COPILOT_0_0_8);

        assert!(copilot_set_up(&dir, &["setup", "--yes", "--json"]));
        let text = std::fs::read_to_string(&hooks).unwrap();
        assert!(text.contains("cmd /d /c "), "{text}");
        assert_eq!(agent_hooks_check(&dir)["status"], "ok");

        // Hooks whose `lane` has gone do nothing, and doctor says so.
        let gone = dir.join("gone").join("lane.exe");
        std::fs::write(
            &hooks,
            lane_core::agents::copilot::hooks_file(true, Some(&gone)),
        )
        .unwrap();
        let check = agent_hooks_check(&dir);
        assert_eq!(check["status"], "warn", "{check}");
        assert!(
            check["summary"]
                .as_str()
                .unwrap()
                .contains("isn't there any more"),
            "{check}"
        );
        assert!(copilot_set_up(&dir, &["setup", "--yes", "--json"]));
        assert_eq!(agent_hooks_check(&dir)["status"], "ok");
    } else {
        // macOS and Linux: 0.0.8's file is the current one, so nothing changes.
        assert_eq!(check["status"], "ok", "{check}");
        assert!(copilot_set_up(&dir, &["setup", "--yes", "--json"]));
        assert_eq!(std::fs::read_to_string(&hooks).unwrap(), COPILOT_0_0_8);
    }

    lane(&dir, &["setup", "--yes", "--agents", "off", "--json"]);
    assert!(!home.join(".copilot/hooks").exists());
}

/// Run a hook command the way Copilot does on Windows, with `stdin`, in `shell`: `cmd` the way
/// VS Code runs it without cmd.exe as ComSpec (Node's `shell: true`), else a PowerShell, the way
/// VS Code (`powershell.exe`) and the Copilot CLI (`pwsh`) run it.
#[cfg(windows)]
fn run_copilot_hook(dir: &PathBuf, shell: &str, command: &str, stdin: &[u8]) -> Output {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(shell);
    match shell {
        "cmd.exe" => cmd.raw_arg(format!("/d /s /c \"{command}\"")),
        _ => cmd.args([
            "-ExecutionPolicy",
            "Bypass",
            "-NoProfile",
            "-NoLogo",
            "-Command",
            command,
        ]),
    };
    let mut child = cmd
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env("LANE_TELEMETRY_URL", NO_COUNT)
        .env("LANE_DAEMON", "0")
        .env("POWERSHELL_UPDATECHECK", "Off")
        .env_remove("LANE_REQUEST_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shell runs");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

#[cfg(windows)]
#[test]
fn copilots_windows_hooks_do_nothing_without_lane_in_cmd_and_powershell() {
    use lane_core::agents::copilot::hooks_file;
    let _shells = timing();
    let dir = config_dir("copilot-windows-shells");
    let repo = repo_with_commit("copilot-windows-shells");
    std::fs::write(repo.join("lane.toml"), "version = 1\n").unwrap();
    // `lane` where people have it: a space, brackets, an apostrophe.
    let lane = dir
        .join("Program Files (x86)")
        .join("O'Brien Lanes")
        .join("lane.exe");
    std::fs::create_dir_all(lane.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_lane"), &lane).unwrap();
    let gone = dir.join("Uninstalled (x86)").join("lane.exe");

    let commands = |lane: &std::path::Path| {
        let v: serde_json::Value = serde_json::from_str(&hooks_file(true, Some(lane))).unwrap();
        ["sessionStart", "preToolUse"].map(|event| {
            v["hooks"][event][0]["powershell"]
                .as_str()
                .unwrap()
                .to_string()
        })
    };
    // A copy of the repo, which `lane` turns down, from the Copilot CLI and from VS Code.
    let copy = [
        serde_json::json!({
            "cwd": repo,
            "toolName": "powershell",
            "toolArgs": serde_json::json!({ "command": "git worktree add ../x" }).to_string(),
        }),
        serde_json::json!({
            "cwd": repo,
            "hook_event_name": "PreToolUse",
            "tool_name": "run_in_terminal",
            "tool_input": { "command": "git worktree add ../x" },
        }),
    ];
    let session = serde_json::json!({ "cwd": repo, "source": "new" });

    let mut shells = vec!["cmd.exe".to_string()];
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    shells.push(format!(
        r"{system}\System32\WindowsPowerShell\v1.0\powershell.exe"
    ));
    let pwsh = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("pwsh.exe"))
        .find(|p| p.is_file());
    // GitHub's Windows runners have PowerShell 7, so CI never skips it without saying so.
    assert!(
        pwsh.is_some() || std::env::var_os("GITHUB_ACTIONS").is_none(),
        "pwsh.exe isn't on PATH, so PowerShell 7 wasn't tested"
    );
    shells.extend(pwsh.map(|p| p.display().to_string()));

    for shell in &shells {
        // `lane` has gone: exit 0 and not a word, for both hooks, so Copilot's own rules apply.
        let [start, tool] = commands(&gone);
        for (command, input) in [(&start, &session), (&tool, &copy[0]), (&tool, &copy[1])] {
            let out = run_copilot_hook(&dir, shell, command, input.to_string().as_bytes());
            assert_eq!(out.status.code(), Some(0), "{shell}: {out:?}");
            assert!(
                out.stdout.is_empty() && out.stderr.is_empty(),
                "{shell}: {out:?}"
            );
        }

        // `lane` is there: it gets the input, and its deny, exit 2, reaches Copilot.
        let [start, tool] = commands(&lane);
        for input in &copy {
            let out = run_copilot_hook(&dir, shell, &tool, input.to_string().as_bytes());
            assert_eq!(out.status.code(), Some(2), "{shell}: {out:?}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("lane new"),
                "{shell}: {out:?}"
            );
        }
        assert_eq!(
            json(&run_copilot_hook(
                &dir,
                shell,
                &tool,
                copy[0].to_string().as_bytes()
            ))["permissionDecision"],
            "deny"
        );
        let ok = serde_json::json!({
            "cwd": repo,
            "toolName": "powershell",
            "toolArgs": serde_json::json!({ "command": "npm test" }).to_string(),
        });
        let out = run_copilot_hook(&dir, shell, &tool, ok.to_string().as_bytes());
        assert_eq!(out.status.code(), Some(0), "{shell}: {out:?}");
        assert!(out.stdout.is_empty(), "{shell}: {out:?}");
        let out = run_copilot_hook(&dir, shell, &start, session.to_string().as_bytes());
        assert_eq!(out.status.code(), Some(0), "{shell}: {out:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn logs_keep_what_changed_lanes() {
    let dir = config_dir("logs");
    let repo = repo_with_commit("logs");

    let out = lane_in(&dir, &repo, &["logs"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Nothing logged yet"));

    assert_eq!(
        lane_in(&dir, &repo, &["new", "fix-auth", "--json"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        lane_in(&dir, &repo, &["new", "other", "--json"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        lane_in(&dir, &repo, &["rm", "fix-auth", "--json"])
            .status
            .code(),
        Some(0)
    );
    // A failure is logged too (not a usage mistake).
    assert_eq!(
        lane_in(&dir, &repo, &["rm", "no-such-lane"]).status.code(),
        Some(1)
    );

    let v = json(&lane_in(&dir, &repo, &["logs", "--json"]));
    let events: Vec<(&str, &str)> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["event"].as_str().unwrap(), e["level"].as_str().unwrap()))
        .collect();
    assert_eq!(
        events,
        [
            ("new", "info"),
            ("new", "info"),
            ("rm", "info"),
            ("rm", "error")
        ],
        "{v}"
    );
    assert!(v["path"].as_str().unwrap().ends_with("lane.log"));
    assert!(
        dir.join("state").join("lane.log").is_file(),
        "the log sits with the state"
    );

    // One lane only, and the last N.
    let v = json(&lane_in(&dir, &repo, &["logs", "fix-auth", "--json"]));
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    let v = json(&lane_in(&dir, &repo, &["logs", "-n", "1", "--json"]));
    assert_eq!(v["entries"][0]["level"], "error");

    // The plain listing marks the error, and says where the file is.
    let text = String::from_utf8_lossy(&lane_in(&dir, &repo, &["logs"]).stdout).to_string();
    assert!(text.contains("✗"), "{text}");
    assert!(
        text.contains("fix-auth: removed the lane and its folder"),
        "{text}"
    );
    assert!(text.contains("(UTC)"), "{text}");
    assert_eq!(
        lane_in(&dir, &repo, &["logs", "-n", "many"]).status.code(),
        Some(2)
    );
}

#[test]
fn doctor_checks_what_could_stop_lanes_working() {
    let dir = config_dir("doctor");
    let repo = repo_with_commit("doctor");
    assert_eq!(
        lane_in(&dir, &repo, &["new", "fix-auth", "--json"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        lane_in(&dir, &repo, &["new", "gone", "--json"])
            .status
            .code(),
        Some(0)
    );
    // A tool cleans up its own worktree folder behind Lanes' back.
    let gone = repo
        .with_file_name(format!(
            "{}.lanes",
            repo.file_name().unwrap().to_string_lossy()
        ))
        .join("gone");
    std::fs::remove_dir_all(&gone).unwrap();

    let doctor = |path: &std::ffi::OsStr| {
        Command::new(env!("CARGO_BIN_EXE_lane"))
            .args(["doctor", "--json"])
            .current_dir(&repo)
            .env("PATH", path)
            .env("LANE_TELEMETRY_URL", NO_COUNT)
            .env("LANE_DAEMON", "0")
            .env("LANE_CONFIG_DIR", &dir)
            .env("LANE_STATE_DIR", dir.join("state"))
            .env("LANE_AGENTS_HOME", dir.join("home"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap()
    };
    let out = doctor(&path_with_lane());
    let v = json(&out);
    let check = |id: &str| {
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no {id} check in {v}"))
            .clone()
    };
    for id in [
        "git",
        "copies",
        "lane-path",
        "agent-hooks",
        "git-hook",
        "lanes",
        "daemon",
        "log",
    ] {
        check(id);
    }
    assert_eq!(check("lane-path")["status"], "ok");
    let lanes = check("lanes");
    assert_eq!(lanes["status"], "warn");
    assert!(
        lanes["summary"].as_str().unwrap().contains("gone"),
        "{lanes}"
    );
    assert!(
        lanes["hint"].as_str().unwrap().contains("lane rm gone"),
        "{lanes}"
    );
    assert_eq!(
        check("git-hook")["status"],
        "info",
        "no lane init in this repo"
    );

    // Following the hint clears it.
    assert_eq!(lane_in(&dir, &repo, &["rm", "gone"]).status.code(), Some(0));
    let v = json(&doctor(&path_with_lane()));
    let lanes = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "lanes")
        .unwrap()
        .clone();
    assert_eq!(lanes["status"], "ok", "{lanes}");

    // Without `lane` on PATH, agents' hooks can't run it: that must be fixed, so exit 1.
    let bare = std::env::join_paths(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|p| {
            !p.join(format!("lane{}", std::env::consts::EXE_SUFFIX))
                .is_file()
        }),
    )
    .unwrap();
    let out = doctor(&bare);
    let v = json(&out);
    let path_check = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "lane-path")
        .unwrap()
        .clone();
    assert_eq!(path_check["status"], "fail", "{path_check}");
    assert_eq!(v["healthy"], false);
    assert_eq!(out.status.code(), Some(1));
}

/// Run `lane` as a person at a terminal would (LANE_INTERACTIVE=1), typing `answers`.
fn lane_typed(dir: &PathBuf, cwd: &std::path::Path, args: &[&str], answers: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lane"))
        .args(args)
        .current_dir(cwd)
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env("LANE_INTERACTIVE", "1")
        .env("LANE_TELEMETRY_URL", NO_COUNT)
        .env("LANE_DAEMON", "0")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("LANE_REQUEST_ID")
        .env_remove("CI")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lane runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answers.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// A repo whose install step is harmless and quick, with Claude Code "installed" in a fake home.
fn guided_repo(name: &str) -> (PathBuf, PathBuf) {
    let dir = config_dir(name);
    let repo = repo_with_commit(name);
    std::fs::write(
        repo.join("lane.toml"),
        "version = 1\n[setup]\ninstall = [\"git --version\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("home/.claude")).unwrap();
    (dir, repo)
}

#[test]
fn init_asks_before_each_step_and_no_changes_nothing() {
    let (dir, repo) = guided_repo("init-no");
    let out = lane_typed(&dir, &repo, &["init"], "n\nn\nn\nn\nn\nn\n");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for q in [
        "? Put Claude Code in lanes on this machine?",
        "? Add the Git hook",
        "? Add a short note on working in lanes to AGENTS.md",
        "? Install dependencies now (git --version)",
    ] {
        assert!(text.contains(q), "missing {q:?} in:\n{text}");
    }
    assert!(text.contains("[Y/n]"));
    assert!(
        !dir.join("home/.claude/settings.json").exists(),
        "nothing on the machine"
    );
    assert!(!repo.join("AGENTS.md").exists());
    assert!(!repo.join(".git/hooks/post-checkout").exists());
    assert!(
        !repo
            .with_file_name(format!(
                "{}.lanes",
                repo.file_name().unwrap().to_string_lossy()
            ))
            .join("_base")
            .exists()
    );
    assert!(text.contains("Next:"), "{text}");
}

#[test]
fn init_with_every_yes_sets_it_all_up_and_says_what_is_left() {
    let (dir, repo) = guided_repo("init-yes");
    let out = lane_typed(&dir, &repo, &["init"], "\n\n\n\n\n\n");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("✓ Claude Code put their work in lanes now"),
        "{text}"
    );
    assert!(
        dir.join("home/.claude/settings.json").is_file(),
        "the machine's agents"
    );
    assert!(repo.join("AGENTS.md").is_file());
    assert!(repo.join(".git/hooks/post-checkout").is_file());
    assert!(
        text.contains("dependencies installed in a clean copy"),
        "{text}"
    );
    assert!(text.contains("git add AGENTS.md"), "{text}");
    assert!(text.contains("claude -w fix-auth"), "{text}");

    // Run again: everything is in place, so nothing is asked.
    let out = lane_typed(&dir, &repo, &["init"], "");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(!text.contains("[Y/n]"), "nothing left to ask:\n{text}");
    assert!(
        text.contains("dependencies are installed in the clean copy"),
        "{text}"
    );
}

#[test]
fn init_yes_takes_every_default_without_asking() {
    let (dir, repo) = guided_repo("init-yes-flag");
    let v = json(&lane_in(&dir, &repo, &["init", "--yes", "--json"]));
    let done: Vec<(&str, bool)> = v["done"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["what"].as_str().unwrap(), d["done"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        done,
        [
            ("lane_toml", false),
            ("agents", true),
            ("git_hook", true),
            ("agents_md", true),
            ("dependencies", true),
        ],
        "{v}"
    );
    assert!(
        v["next"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["run"] == "claude -w fix-auth")
    );

    // Agents, scripts and CI (no terminal, no --yes) get the plain init: nothing outside the repo.
    let (dir2, repo2) = guided_repo("init-plain");
    let v = json(&lane_in(&dir2, &repo2, &["init", "--json"]));
    assert!(v.get("done").is_none(), "the plain init: {v}");
    assert!(!dir2.join("home/.claude/settings.json").exists());
}

#[test]
fn doctor_fix_makes_the_fixes_lanes_can_make() {
    let (dir, repo) = guided_repo("doctor-fix");
    assert_eq!(
        lane_in(&dir, &repo, &["new", "gone", "--json"])
            .status
            .code(),
        Some(0)
    );
    let lanes_dir = repo.with_file_name(format!(
        "{}.lanes",
        repo.file_name().unwrap().to_string_lossy()
    ));
    std::fs::remove_dir_all(lanes_dir.join("gone")).unwrap();

    // In a script, --fix needs --yes; and --yes needs --fix.
    assert_eq!(
        lane_in(&dir, &repo, &["doctor", "--fix"]).status.code(),
        Some(2)
    );
    assert_eq!(
        lane_in(&dir, &repo, &["doctor", "--yes"]).status.code(),
        Some(2)
    );

    let v = json(&lane_in(
        &dir,
        &repo,
        &["doctor", "--fix", "--yes", "--json"],
    ));
    let fixed: Vec<&str> = v["fixed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["check"].as_str().unwrap())
        .collect();
    assert_eq!(fixed, ["agent-hooks", "git-hook", "lanes"], "{v}");
    let status = |id: &str| {
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()["status"]
            .clone()
    };
    assert_eq!(status("agent-hooks"), "ok");
    assert_eq!(status("git-hook"), "ok");
    assert_eq!(status("lanes"), "info", "no lanes left in this repo");
    assert!(dir.join("home/.claude/settings.json").is_file());
}

// ---------- The install count ----------

/// A stand-in for the install count's endpoint on a local port: it keeps the body of every
/// request. With `answer` false it never replies, like a server that hangs.
struct CountServer {
    url: String,
    bodies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

fn count_server(answer: bool) -> CountServer {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/count", listener.local_addr().unwrap());
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = bodies.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let kept = kept.clone();
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if buf.len() < end + 4 + len {
                        continue;
                    }
                    let body = String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                    kept.lock().unwrap().push(body);
                    if answer {
                        let _ = stream
                            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
                    } else {
                        std::thread::sleep(std::time::Duration::from_secs(10));
                    }
                    break;
                }
            });
        }
    });
    CountServer { url, bodies }
}

impl CountServer {
    /// What arrived once `n` requests have, or after `within`, whichever is first.
    fn wait_for(&self, n: usize, within: std::time::Duration) -> Vec<String> {
        let start = std::time::Instant::now();
        loop {
            let got = self.bodies.lock().unwrap().clone();
            if got.len() >= n || start.elapsed() > within {
                return got;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Run `lane` as on a person's machine (no CI, no opt-outs), counting installs to `url`.
fn lane_counting(dir: &PathBuf, url: &str, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lane"));
    cmd.args(args)
        .current_dir(std::env::temp_dir())
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env("LANE_TELEMETRY_URL", url)
        .env("LANE_DAEMON", "0")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("DO_NOT_TRACK")
        .env_remove("LANE_TELEMETRY");
    for ci in ["CI", "GITHUB_ACTIONS", "GITLAB_CI", "BUILDKITE", "TF_BUILD"] {
        cmd.env_remove(ci);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("lane runs")
}

#[test]
fn the_install_count_goes_once_after_setup_with_only_the_listed_fields() {
    let server = count_server(true);
    let dir = config_dir("count-once");

    // Nothing before the setting is chosen.
    assert!(
        lane_counting(&dir, &server.url, &[], &["--version"])
            .status
            .success()
    );
    assert!(
        server
            .wait_for(1, std::time::Duration::from_millis(1500))
            .is_empty()
    );

    let out = lane_counting(&dir, &server.url, &[], &["setup", "--yes", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = server.wait_for(1, std::time::Duration::from_secs(10));
    assert_eq!(got.len(), 1, "{got:?}");
    let ping: serde_json::Value = serde_json::from_str(&got[0]).unwrap();
    let mut keys: Vec<&str> = ping
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["arch", "channel", "id", "os", "version"],
        "only what the welcome lists"
    );
    assert_eq!(ping["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ping["channel"], "source", "a test binary lives in target/");
    let id = ping["id"].as_str().unwrap();
    assert_eq!(id.len(), 32);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));

    // Counted once per version: later commands send nothing more.
    assert!(
        lane_counting(&dir, &server.url, &[], &["--version"])
            .status
            .success()
    );
    assert!(
        lane_counting(&dir, &server.url, &[], &["telemetry"])
            .status
            .success()
    );
    assert_eq!(
        server
            .wait_for(2, std::time::Duration::from_millis(1500))
            .len(),
        1
    );

    // `lane telemetry` shows the same id, and that this version was counted.
    let status = json(&lane_counting(
        &dir,
        &server.url,
        &[],
        &["telemetry", "--json"],
    ));
    assert_eq!(status["on"], true);
    assert_eq!(status["sends"]["id"], id);
    assert_eq!(status["counted"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn opt_outs_ci_and_the_off_switch_send_nothing() {
    let server = count_server(true);
    // (settings folder, extra environment, arguments)
    type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);
    let cases: [Case; 5] = [
        (
            "count-dnt",
            &[("DO_NOT_TRACK", "1")],
            &["setup", "--yes", "--json"],
        ),
        (
            "count-env",
            &[("LANE_TELEMETRY", "0")],
            &["setup", "--yes", "--json"],
        ),
        ("count-ci", &[("CI", "true")], &["setup", "--yes", "--json"]),
        (
            "count-gha",
            &[("GITHUB_ACTIONS", "true")],
            &["setup", "--yes", "--json"],
        ),
        (
            "count-off",
            &[],
            &["setup", "--yes", "--telemetry", "off", "--json"],
        ),
    ];
    for (name, env, args) in cases {
        let out = lane_counting(&config_dir(name), &server.url, env, args);
        assert!(
            out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // Switched off after setup: the next command sends nothing either.
    let dir = config_dir("count-switched");
    let off = json(&lane_counting(
        &dir,
        &server.url,
        &[("DO_NOT_TRACK", "1")],
        &["setup", "--yes", "--json"],
    ));
    assert_eq!(
        off["settings"]["telemetry_forced_off"],
        "DO_NOT_TRACK is set"
    );
    let switched = json(&lane_counting(
        &dir,
        &server.url,
        &[],
        &["telemetry", "off", "--json"],
    ));
    assert_eq!(switched["on"], false);
    assert_eq!(switched["saved"], true);
    assert!(
        lane_counting(&dir, &server.url, &[], &["--version"])
            .status
            .success()
    );

    assert!(
        server
            .wait_for(1, std::time::Duration::from_secs(2))
            .is_empty(),
        "nothing may be sent"
    );
}

#[test]
fn a_hanging_endpoint_never_slows_a_command() {
    let _timed = timing();
    let server = count_server(false);
    let dir = config_dir("count-hang");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "onboarded = \"0.0.1\"\ntelemetry = true\n",
    )
    .unwrap();
    let timed = |env: &[(&str, &str)]| {
        let started = std::time::Instant::now();
        let out = lane_counting(&dir, &server.url, env, &["--version"]);
        assert!(out.status.success());
        started.elapsed()
    };
    // Unmeasured first: a first launch of a new binary can be slow on its own (Windows scans it).
    timed(&[("LANE_TELEMETRY", "0")]);
    let without = timed(&[("LANE_TELEMETRY", "0")]);
    let with = timed(&[]);
    // curl gives up after 1 s. `lane` doesn't wait for it, and nor does whoever reads its output,
    // so sending costs well under that over the same command sending nothing.
    assert!(
        with < without + std::time::Duration::from_millis(700),
        "took {with:?}, against {without:?} without the count"
    );
    assert_eq!(
        server.wait_for(1, std::time::Duration::from_secs(10)).len(),
        1,
        "it was still sent"
    );
}

#[test]
fn telemetry_switches_by_value_and_says_what_it_sends() {
    let dir = config_dir("telemetry-cmd");
    // As on a person's machine: CI forces the count off, and these runs shouldn't look like CI.
    let lane = |dir: &PathBuf, args: &[&str]| lane_counting(dir, NO_COUNT, &[], args);
    let on = json(&lane(&dir, &["telemetry", "on", "--json"]));
    assert_eq!(
        (on["on"].clone(), on["saved"].clone()),
        (true.into(), true.into())
    );
    assert!(on["sends"]["id"].is_null(), "no id until the first send");
    // Setting a value is safe to retry: twice off is off.
    for _ in 0..2 {
        assert_eq!(
            json(&lane(&dir, &["telemetry", "off", "--json"]))["setting"],
            false
        );
    }
    let text = String::from_utf8_lossy(&lane(&dir, &["telemetry"]).stdout).into_owned();
    assert!(text.contains("Install count: off."), "{text}");
    assert!(
        text.contains("never sends code, file paths, repo names"),
        "{text}"
    );
    assert!(text.contains("lane telemetry on"), "{text}");
    let bad = lane(&dir, &["telemetry", "maybe"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("lane telemetry off"));
}

// ---------- The daemon ----------

#[test]
fn lane_daemon_starts_reports_and_stops_laned() {
    let dir = config_dir("daemon");
    let laned = PathBuf::from(env!("CARGO_BIN_EXE_lane"))
        .with_file_name(format!("laned{}", std::env::consts::EXE_SUFFIX));
    if !laned.is_file() {
        eprintln!(
            "skipped: laned isn't built next to lane (run `cargo build` or `cargo test --all`)"
        );
        return;
    }
    // Stop it however the test ends.
    struct Stop(PathBuf);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = lane(&self.0, &["daemon", "stop"]);
        }
    }
    let _stop = Stop(dir.clone());

    let before = json(&lane(&dir, &["daemon", "status", "--json"]));
    assert_eq!(before["running"], false);
    let text = String::from_utf8_lossy(&lane(&dir, &["daemon"]).stdout).into_owned();
    assert!(text.contains("laned isn't running"), "{text}");
    assert!(text.contains("lane daemon start"), "{text}");

    let started = json(&lane(&dir, &["daemon", "start", "--json"]));
    assert_eq!(started["running"], true);
    assert_eq!(started["version"], env!("CARGO_PKG_VERSION"));
    let again = json(&lane(&dir, &["daemon", "start", "--json"]));
    assert_eq!(again["pid"], started["pid"], "a second start leaves it be");

    let doctor = json(&lane(&dir, &["doctor", "--json"]));
    let check = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "daemon")
        .unwrap()
        .clone();
    assert_eq!(check["status"], "ok", "{check}");

    // With laned running, writes go through it: same results, install output on stderr.
    let repo = repo_with_commit("daemon-writes");
    std::fs::write(
        repo.join("lane.toml"),
        "version = 1\n[setup]\ninstall = [\"echo installing from lane.toml\"]\n",
    )
    .unwrap();
    let made = lane_in(&dir, &repo, &["new", "fix-auth", "--json"]);
    let stderr = String::from_utf8_lossy(&made.stderr).into_owned();
    assert!(made.status.success(), "{stderr}");
    assert_eq!(json(&made)["lane"]["name"], "fix-auth");
    assert!(stderr.contains("installing from lane.toml"), "{stderr}");
    let writes = || json(&lane(&dir, &["daemon", "status", "--json"]))["writes"].clone();
    assert_eq!(writes(), 1, "laned made it");
    assert_eq!(
        json(&lane(&dir, &["daemon", "status", "--json"]))["lanes"],
        1,
        "the new lane has an actor"
    );
    // `lane exec` tells laned what it runs, and `rm` through laned stops that first.
    let path = PathBuf::from(json(&made)["lane"]["path"].as_str().unwrap());
    let mut args = vec!["exec", "fix-auth", "--"];
    args.extend(a_minute());
    let mut exec = lane_started(&dir, &repo, &args);
    let noted = dir.join("state").join("running");
    eventually("lane exec never noted its command", || {
        std::fs::read_dir(&noted).is_ok_and(|mut d| d.next().is_some())
    });
    let removed = lane_in(&dir, &repo, &["rm", "fix-auth", "--json"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_eq!(json(&removed)["removed"]["name"], "fix-auth");
    assert_eq!(json(&removed)["stopped"].as_array().unwrap().len(), 1);
    assert!(!path.exists());
    eventually("lane exec kept waiting", || {
        exec.try_wait().unwrap().is_some()
    });
    let missing = lane_in(&dir, &repo, &["rm", "fix-auth", "--json"]);
    assert_eq!(
        missing.status.code(),
        Some(1),
        "the engine's own error, through laned"
    );
    assert_eq!(json(&missing)["error"]["code"], "not_found");
    assert_eq!(writes(), 3);

    let stopped = json(&lane(&dir, &["daemon", "stop", "--json"]));
    assert_eq!(
        (stopped["running"].clone(), stopped["stopped"].clone()),
        (false.into(), true.into())
    );
    let twice = lane(&dir, &["daemon", "stop"]);
    assert!(twice.status.success());
    assert!(String::from_utf8_lossy(&twice.stdout).contains("wasn't running"));

    let bad = lane(&dir, &["daemon", "restart"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("lane daemon start"));
}

/// Windows gives an app installed from a package (the Claude desktop app) its own copy of
/// AppData, and Lanes 0.0.9 kept its state and settings there. So lanes made in a terminal and
/// lanes made by Claude Code in that app lived in two stores, with the same ports. Here both
/// old stores are made the way 0.0.9 had them (in a stand-in home, never the real one), with an
/// older daemon running for the old folder. The first `lane` after the upgrade stops that
/// daemon and brings both stores into one in `%USERPROFILE%\.lane`, five at once.
#[cfg(windows)]
#[test]
fn old_appdata_stores_come_across_into_one() {
    let dir = config_dir("moved");
    let home = dir.join("home");
    let local = home.join("AppData").join("Local");
    let roaming = home.join("AppData").join("Roaming");
    let copy = |under: &str| {
        local
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join(under)
            .join("lane")
    };
    let repo = repo_with_commit("moved");
    let bin = env!("CARGO_BIN_EXE_lane");
    let before = |state: &std::path::Path, config: &std::path::Path, args: &[&str]| {
        Command::new(bin)
            .args(args)
            .current_dir(&repo)
            .env("LANE_STATE_DIR", state)
            .env("LANE_CONFIG_DIR", config)
            .env("LANE_AGENTS_HOME", &home)
            .env("LANE_TELEMETRY_URL", NO_COUNT)
            .env("LANE_DAEMON", "0")
            .env_remove("LANE_REQUEST_ID")
            .env_remove("CI")
            .output()
            .unwrap()
    };
    let terminal = json(&before(
        &local.join("lane"),
        &roaming.join("lane"),
        &["new", "terminal", "--json"],
    ));
    let app = json(&before(
        &copy("Local"),
        &copy("Roaming"),
        &["new", "app", "--json"],
    ));
    assert_eq!(
        terminal["lane"]["ports"], app["lane"]["ports"],
        "the bug: two stores, the same ports"
    );
    std::fs::create_dir_all(roaming.join("lane")).unwrap();
    std::fs::write(
        roaming.join("lane").join("config.toml"),
        "version = 1\nonboarded = \"0.0.9\"\ntelemetry = false\n",
    )
    .unwrap();

    // An older daemon still serving the old folder.
    let laned = PathBuf::from(bin).with_file_name("laned.exe");
    let mut old_daemon = laned.is_file().then(|| {
        Command::new(&laned)
            .env("LANE_STATE_DIR", local.join("lane"))
            .env("LANED_IDLE_SECS", "120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    });
    if old_daemon.is_some() {
        eventually("the old daemon never started", || {
            json(&before(
                &local.join("lane"),
                &roaming.join("lane"),
                &["daemon", "status", "--json"],
            ))["running"]
                == true
        });
    }

    // The upgraded `lane`: no overrides, so its folders come from the (stand-in) home.
    let after = |args: &[&str]| {
        Command::new(bin)
            .args(args)
            .current_dir(&repo)
            .env_remove("LANE_STATE_DIR")
            .env_remove("LANE_CONFIG_DIR")
            .env_remove("LANE_AGENTS_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("LANE_REQUEST_ID")
            .env_remove("CI")
            .env("USERPROFILE", &home)
            .env("LOCALAPPDATA", &local)
            .env("APPDATA", &roaming)
            .env("LANE_TELEMETRY_URL", NO_COUNT)
            .env("LANE_DAEMON", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let five: Vec<_> = (0..5)
        .map(|_| after(&["list", "--all", "--json"]))
        .collect();
    for child in five {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json(&out);
        let lanes = v["lanes"].as_array().unwrap();
        let mut names: Vec<&str> = lanes.iter().map(|l| l["name"].as_str().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["app", "terminal"], "{v}");
        assert_ne!(
            lanes[0]["ports"], lanes[1]["ports"],
            "one store, no shared ports"
        );
    }
    let new = home.join(".lane");
    assert!(new.join("state").join("state.db").is_file());
    let config = std::fs::read_to_string(new.join("config.toml")).unwrap();
    assert!(config.contains("telemetry = false"), "{config}");
    assert!(
        local.join("lane").join("state.db").is_file() && copy("Local").join("state.db").is_file(),
        "the old stores are left for you to delete"
    );
    if let Some(daemon) = &mut old_daemon {
        eventually("the old daemon kept running", || {
            daemon.try_wait().unwrap().is_some()
        });
    }

    // `lane doctor` says where things are now, and that the old folders aren't used.
    let doctor = after(&["doctor", "--json"]).wait_with_output().unwrap();
    let v = json(&doctor);
    let check = |id: &str| {
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no {id} check in {v}"))
            .clone()
    };
    let folders = check("folders");
    assert!(
        folders["summary"]
            .as_str()
            .unwrap()
            .contains(&new.join("state").display().to_string()),
        "{folders}"
    );
    let old = check("old-folders");
    assert_eq!(old["status"], "info", "{old}");
    let summary = old["summary"].as_str().unwrap();
    assert!(
        summary.contains(&local.join("lane").display().to_string())
            && summary.contains("Claude_pzs8sxrjxfjjc"),
        "{old}"
    );
    assert!(
        old["hint"].as_str().unwrap().contains("Remove-Item"),
        "{old}"
    );

    // Lanes made after the move are in the new store only.
    let made = after(&["new", "later", "--json"])
        .wait_with_output()
        .unwrap();
    assert!(made.status.success());
    let list = json(&after(&["list", "--json"]).wait_with_output().unwrap());
    assert_eq!(list["lanes"].as_array().unwrap().len(), 3);
    let old_list = json(&before(
        &local.join("lane"),
        &roaming.join("lane"),
        &["list", "--json"],
    ));
    assert_eq!(old_list["lanes"].as_array().unwrap().len(), 1);
}
