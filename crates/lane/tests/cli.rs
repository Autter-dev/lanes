//! The `lane` binary from the outside: what scripts, CI and agents see. Output is never a
//! terminal here, so the full-screen app must never open.

use std::path::PathBuf;
use std::process::{Command, Output};

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
        .env_remove("LANE_REQUEST_ID")
        .env_remove("DO_NOT_TRACK")
        .env_remove("LANE_TELEMETRY")
        .env_remove("CI")
        .output()
        .expect("lane runs")
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
    let dir = std::env::temp_dir().join(format!("lane-cli-repo-{}-{name}", std::process::id()));
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
fn unbuilt_commands_answer_in_json_with_exit_3() {
    let out = lane(&config_dir("unbuilt"), &["init", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let v = json(&out);
    assert_eq!(v["error"]["code"], "not_implemented");
    assert!(out.stderr.is_empty());
    // Without --json it's a line on stderr, as before.
    let text = lane(&config_dir("unbuilt-text"), &["init"]);
    assert_eq!(text.status.code(), Some(3));
    assert!(text.stdout.is_empty());
    assert!(String::from_utf8_lossy(&text.stderr).contains("isn't built yet"));
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
    assert_eq!(json(&out)["removed"]["name"], "fix-auth");
    assert!(!path.exists());
    let listed = json(&lane_in(&dir, &repo, &["list", "--json"]));
    assert_eq!(
        listed["lanes"].as_array().unwrap().len(),
        1,
        "add-tests is left"
    );

    let out = lane_in(&dir, &repo, &["rm", "fix-auth", "--json"]);
    assert_eq!(json(&out)["error"]["code"], "not_found");
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
