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
    assert_eq!(steps[0], "lane init");
    assert!(steps[1].starts_with("cd "), "{steps:?}");
    assert!(steps[2].starts_with("lane exec fix-auth -- "), "{steps:?}");

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
    assert_eq!(v["next"][1]["run"], "git add lane.toml");
    let text = std::fs::read_to_string(repo.join("lane.toml")).unwrap();
    assert!(text.contains("template = \".env.example\""), "{text}");

    // Run again: the file is kept as it is.
    std::fs::write(repo.join("lane.toml"), "version = 1\n# mine\n").unwrap();
    let v = json(&lane_in(&dir, &repo, &["init", "--json"]));
    assert_eq!(
        (v["written"].as_bool(), v["kept_existing"].as_bool()),
        (Some(false), Some(true))
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
