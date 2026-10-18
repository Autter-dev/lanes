//! `lane mcp` from the outside: an agent's MCP client talking to it over stdin and stdout, in a
//! temporary repo, with settings and state kept in a temporary folder.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// A closed local port, so no test ever sends a real install count.
const NO_COUNT: &str = "http://127.0.0.1:9/";

fn temp(kind: &str, name: &str) -> PathBuf {
    // On CI's Btrfs, XFS and ReFS disks, repos and their lanes live on that disk.
    let base = std::env::var_os("LANE_TEST_CLONE_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("lane-mcp-{kind}-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn lane_cmd(dir: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lane"));
    cmd.current_dir(cwd)
        .env("LANE_CONFIG_DIR", dir)
        .env("LANE_STATE_DIR", dir.join("state"))
        // Agents' user settings go here, never in the real ~/.claude of whoever runs the tests.
        .env("LANE_AGENTS_HOME", dir.join("home"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("LANE_REQUEST_ID")
        .env_remove("DO_NOT_TRACK")
        .env_remove("LANE_TELEMETRY")
        .env_remove("CI")
        .env("LANE_TELEMETRY_URL", NO_COUNT);
    cmd
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Lanes Test",
            "-c",
            "user.email=test@lanes.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repo with Rust and TypeScript files, a long file, a binary one, an ignored one and a new
/// one not committed yet; and a file outside it that no tool may read.
fn repo(name: &str) -> (PathBuf, PathBuf) {
    let repo = temp("repo", name);
    let write = |rel: &str, text: &[u8]| {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    write(
        "src/lib.rs",
        b"//! A library.\n\npub struct Lane;\n\npub fn hello() -> &'static str {\n    \"needle in rust\"\n}\n",
    );
    write(
        "web/app.ts",
        b"import x from 'y';\n\nexport function start() {}\n\nexport class App {}\n",
    );
    write(
        "notes/many.txt",
        b"needle 1\nhay\nneedle 2\nneedle 3\nneedle 4\n",
    );
    let long: String = (1..=2500).map(|n| format!("line {n}\n")).collect();
    write("long.txt", long.as_bytes());
    write("logo.bin", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR");
    write("README.md", b"# Test\n");
    write(".gitignore", b"ignored.txt\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    write("ignored.txt", b"needle that .gitignore hides\n");
    write("fresh.txt", b"needle not committed yet\n");
    let secret = temp("secret", name);
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::write(secret.join("secret.txt"), "needle outside the repo\n").unwrap();
    (repo, secret.join("secret.txt"))
}

/// An MCP client for `lane mcp`, one JSON-RPC message per line.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl Client {
    fn start(dir: &Path, cwd: &Path) -> Client {
        let mut child = lane_cmd(dir, cwd)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("lane mcp starts");
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Client {
            child,
            stdin,
            lines,
            next_id: 1,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// Send a request and wait for its answer. Every line on stdout must be JSON-RPC.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(120))
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carries only JSON-RPC ({e}): {line}"));
            assert_eq!(message["jsonrpc"], "2.0", "{message}");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// Call a tool; the call itself must succeed, though the tool may report an error.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let answer = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        assert!(answer.get("error").is_none(), "{answer}");
        answer["result"].clone()
    }

    /// A call that worked: its text, the note after it (if any), and its JSON.
    fn ok(&mut self, tool: &str, arguments: Value) -> (String, Option<String>, Value) {
        let result = self.call(tool, arguments.clone());
        assert_eq!(
            result["isError"], false,
            "{tool} {arguments} failed: {result}"
        );
        let text = |i: usize| result["content"][i]["text"].as_str().map(String::from);
        (
            text(0).unwrap(),
            text(1),
            result["structuredContent"].clone(),
        )
    }

    /// A call the tool turned down: its text, which says what to do next, and its JSON error.
    fn refused(&mut self, tool: &str, arguments: Value) -> (String, Value) {
        let result = self.call(tool, arguments.clone());
        assert_eq!(
            result["isError"], true,
            "{tool} {arguments} worked: {result}"
        );
        let error = result["structuredContent"]["error"].clone();
        assert!(
            error["hint"].is_string(),
            "every error has a hint: {result}"
        );
        (
            result["content"][0]["text"].as_str().unwrap().to_string(),
            error,
        )
    }

    /// Close stdin, as a client does when it's done, and wait for the server to exit.
    fn finish(mut self) -> Option<i32> {
        drop(self.stdin.take());
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(30) {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("lane mcp didn't exit after stdin closed");
    }
}

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect()
}

#[test]
fn an_agent_reads_the_repo_and_makes_a_lane_over_mcp() {
    let dir = temp("config", "e2e");
    let (repo, secret) = repo("e2e");
    let status_before = git(&repo, &["status", "--porcelain"]);
    let mut mcp = Client::start(&dir, &repo);

    // The handshake.
    let init = mcp.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "lanes-test", "version": "1" }
        }),
    );
    let info = &init["result"];
    assert_eq!(info["protocolVersion"], "2025-06-18", "{init}");
    assert_eq!(info["serverInfo"]["name"], "lanes");
    assert!(info["capabilities"]["tools"].is_object(), "{init}");
    assert!(
        info["instructions"].as_str().unwrap().contains("lane.new"),
        "{init}"
    );
    mcp.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    // The tools, named exactly, described for agents.
    let list = mcp.request("tools/list", json!({}));
    let tools = &list["result"]["tools"];
    assert_eq!(
        names(tools),
        [
            "repo.read",
            "repo.outline",
            "repo.grep",
            "lane.new",
            "lane.list"
        ]
    );
    let describe = |name: &str| {
        tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["description"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(describe("repo.outline").contains("Best effort"));
    assert!(describe("repo.grep").contains(".gitignore"));
    assert!(describe("lane.new").contains("instead of `git worktree add` or copying the repo"));
    assert!(describe("lane.list").contains("lane list --json"));

    // repo.read: a whole file, a range, and a long file in parts.
    let (text, note, data) = mcp.ok("repo.read", json!({ "path": "src/lib.rs" }));
    assert_eq!(
        text,
        std::fs::read_to_string(repo.join("src/lib.rs")).unwrap()
    );
    assert_eq!(note, None);
    assert_eq!(data["total_lines"], 7);
    let (text, _, data) = mcp.ok(
        "repo.read",
        json!({ "path": "./src/../src/lib.rs", "start_line": 3, "end_line": 3 }),
    );
    assert_eq!(text, "pub struct Lane;\n");
    assert_eq!(data["path"], "src/lib.rs");
    assert_eq!(
        (data["start_line"].as_u64(), data["end_line"].as_u64()),
        (Some(3), Some(3))
    );
    let (text, note, data) = mcp.ok("repo.read", json!({ "path": "long.txt" }));
    assert_eq!(text.lines().count(), 2000);
    assert_eq!(data["truncated"], true);
    assert!(
        note.as_deref().unwrap().contains("start_line 2001"),
        "{note:?}"
    );
    let (text, note, _) = mcp.ok(
        "repo.read",
        json!({ "path": "long.txt", "start_line": "2499" }),
    );
    assert_eq!((text.as_str(), note), ("line 2499\nline 2500\n", None));

    // Nothing outside the repo, and errors that say what to do next.
    let (text, error) = mcp.refused("repo.read", json!({ "path": "../secret.txt" }));
    assert!(text.contains("is outside the repo"), "{text}");
    assert!(text.contains("Give a path from there"), "{text}");
    assert_eq!(error["code"], "usage");
    let (text, _) = mcp.refused("repo.read", json!({ "path": secret.to_string_lossy() }));
    assert!(text.contains("is outside the repo"), "{text}");
    let parent = repo.parent().unwrap().to_path_buf();
    let rel_secret = format!(
        "../{}/secret.txt",
        secret
            .parent()
            .unwrap()
            .strip_prefix(&parent)
            .unwrap()
            .display()
    );
    let (text, _) = mcp.refused("repo.read", json!({ "path": rel_secret }));
    assert!(text.contains("is outside the repo"), "{text}");
    // A link in the repo to a folder outside it.
    link_dir(secret.parent().unwrap(), &repo.join("escape"));
    let (text, _) = mcp.refused("repo.read", json!({ "path": "escape/secret.txt" }));
    assert!(text.contains("is a link to"), "{text}");
    assert!(text.contains("outside the repo"), "{text}");
    unlink_dir(&repo.join("escape"));
    let (text, error) = mcp.refused("repo.read", json!({ "path": "src/missing.rs" }));
    assert!(text.contains("no file `src/missing.rs`"), "{text}");
    assert_eq!(error["code"], "not_found");
    let (text, _) = mcp.refused("repo.read", json!({ "path": "logo.bin" }));
    assert!(text.contains("binary"), "{text}");
    let (text, _) = mcp.refused("repo.read", json!({ "path": "src" }));
    assert!(text.contains("is a folder"), "{text}");
    let (text, _) = mcp.refused("repo.read", json!({ "file": "src/lib.rs" }));
    assert_eq!(
        text,
        "repo.read doesn't take `file`. repo.read takes `path`, `start_line`, `end_line`."
    );
    let (text, _) = mcp.refused(
        "repo.read",
        json!({ "path": "src/lib.rs", "start_line": 9 }),
    );
    assert!(text.contains("has 7 lines"), "{text}");

    // repo.outline: top-level definitions with their line numbers.
    let (text, _, data) = mcp.ok("repo.outline", json!({ "path": "src/lib.rs" }));
    assert_eq!(text, "3: struct Lane\n5: fn hello");
    assert_eq!(data["language"], "rust");
    assert_eq!(
        data["items"][1],
        json!({ "line": 5, "kind": "fn", "name": "hello" })
    );
    let (text, _, _) = mcp.ok("repo.outline", json!({ "path": "web/app.ts" }));
    assert_eq!(text, "3: function start\n5: class App");
    let (text, _) = mcp.refused("repo.outline", json!({ "path": "README.md" }));
    assert!(text.contains("Read the file with repo.read"), "{text}");

    // repo.grep: tracked and new files, never ignored ones, capped with a note.
    let (text, note, data) = mcp.ok("repo.grep", json!({ "pattern": "needle [0-9]" }));
    assert_eq!(
        text,
        "notes/many.txt:1:needle 1\nnotes/many.txt:3:needle 2\nnotes/many.txt:4:needle 3\nnotes/many.txt:5:needle 4"
    );
    assert_eq!(note, None);
    assert_eq!(data["truncated"], false);
    let (text, _, _) = mcp.ok("repo.grep", json!({ "pattern": "needle", "literal": true }));
    assert!(
        text.contains("fresh.txt:1:needle not committed yet"),
        "{text}"
    );
    assert!(text.contains("src/lib.rs:6:"), "{text}");
    assert!(!text.contains("ignored.txt"), "{text}");
    assert!(!text.contains("outside the repo"), "{text}");
    let (text, note, data) = mcp.ok(
        "repo.grep",
        json!({ "pattern": "NEEDLE", "ignore_case": true, "path": "notes", "max_results": 2 }),
    );
    assert_eq!(text.lines().count(), 2, "{text}");
    assert_eq!(data["truncated"], true);
    assert!(
        note.as_deref().unwrap().starts_with("Stopped at 2 matches"),
        "{note:?}"
    );
    let (text, _, _) = mcp.ok("repo.grep", json!({ "pattern": "no such text" }));
    assert_eq!(text, "No matches for `no such text`.");
    let (text, _) = mcp.refused("repo.grep", json!({ "pattern": "hello(" }));
    assert!(text.contains("POSIX extended regex"), "{text}");
    let (text, _) = mcp.refused("repo.grep", json!({ "pattern": "x", "path": "../.." }));
    assert!(text.contains("outside"), "{text}");

    // The read tools wrote nothing: not Lanes' state, not the repo.
    assert!(!dir.join("state").exists(), "the read tools wrote state");
    assert_eq!(git(&repo, &["status", "--porcelain"]), status_before);

    // lane.list before any lane: the same as `lane list --json`.
    let (_, _, data) = mcp.ok("lane.list", json!({}));
    assert_eq!(data["lanes"], json!([]));
    assert_eq!(data["next"][0]["run"], "lane new fix-auth");
    assert!(!dir.join("state").exists(), "lane.list wrote state");

    // lane.new: a lane with its folder, ports and URL.
    let (text, _, made) = mcp.ok(
        "lane.new",
        json!({ "name": "fix-auth", "agent": "claude", "request_id": "mcp-test-1" }),
    );
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), made);
    assert_eq!(made["name"], "fix-auth");
    assert_eq!(made["agent"], "claude");
    let path = PathBuf::from(made["path"].as_str().unwrap());
    assert!(path.join("src/lib.rs").is_file(), "{made}");
    let start = made["ports"]["start"].as_u64().unwrap();
    assert!(made["ports"]["end"].as_u64().unwrap() > start, "{made}");
    assert_eq!(made["url"], format!("http://fix-auth.localhost:{start}"));
    assert!(
        made["next"][0]["run"].as_str().unwrap().starts_with("cd "),
        "{made}"
    );

    // Safe to retry with the same request id; the same name without one is a conflict.
    let (_, _, again) = mcp.ok(
        "lane.new",
        json!({ "name": "fix-auth", "request_id": "mcp-test-1" }),
    );
    assert_eq!(again["path"], made["path"]);
    let (text, error) = mcp.refused("lane.new", json!({ "name": "fix-auth" }));
    assert_eq!(
        text,
        "there's already a lane called `fix-auth` in this repo. Pick another name, or remove it first with `lane rm fix-auth`."
    );
    assert_eq!(error["code"], "conflict");
    let (text, error) = mcp.refused("lane.new", json!({ "name": "Fix Auth" }));
    assert!(text.contains("can't be a lane name"), "{text}");
    assert!(text.contains("like `fix-auth`"), "{text}");
    assert_eq!(error["exit"], 2);
    let (text, _) = mcp.refused("lane.new", json!({}));
    assert!(text.contains("lane.new needs `name`"), "{text}");

    // lane.list now: exactly what `lane list --json` prints.
    let (_, _, listed) = mcp.ok("lane.list", json!({}));
    assert_eq!(names(&listed["lanes"]), ["fix-auth"]);
    let cli = lane_cmd(&dir, &repo)
        .args(["list", "--json"])
        .output()
        .unwrap();
    let cli: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(listed, cli);

    // A tool that isn't there is a protocol error, not a tool error.
    let answer = mcp.request(
        "tools/call",
        json!({ "name": "repo.write", "arguments": {} }),
    );
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("repo.read"),
        "{answer}"
    );

    assert_eq!(mcp.finish(), Some(0));
}

/// A link at `at` to the folder `to`: a symlink, or on Windows a junction, which needs no
/// special rights (a symlink there needs Developer Mode).
fn link_dir(to: &Path, at: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(to, at).unwrap();
    #[cfg(windows)]
    {
        let out = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(at)
            .arg(to)
            .output()
            .unwrap();
        assert!(out.status.success(), "mklink /J: {out:?}");
    }
}

/// Remove the link `link_dir` made, leaving what it points to.
fn unlink_dir(at: &Path) {
    #[cfg(unix)]
    std::fs::remove_file(at).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(at).unwrap();
}

#[test]
fn outside_a_repo_the_tools_say_where_to_start_it() {
    let dir = temp("config", "plain");
    let plain = temp("plain", "plain");
    std::fs::create_dir_all(&plain).unwrap();
    let mut mcp = Client::start(&dir, &plain);
    mcp.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "lanes-test", "version": "1" }
        }),
    );
    for (tool, args) in [
        ("repo.read", json!({ "path": "a.txt" })),
        ("lane.list", json!({})),
    ] {
        let (text, error) = mcp.refused(tool, args);
        assert!(text.contains("isn't inside a Git repo"), "{text}");
        assert!(text.contains("claude mcp add lanes -- lane mcp"), "{text}");
        assert_eq!(error["code"], "not_a_repo");
    }
    assert_eq!(mcp.finish(), Some(0));
}

fn run(dir: &Path, args: &[&str]) -> Output {
    lane_cmd(dir, &std::env::temp_dir())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn mcp_help_says_how_to_add_it_and_bad_options_are_usage_errors() {
    let dir = temp("config", "help");
    let out = run(&dir, &["mcp", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("Usage: lane mcp"), "{help}");
    assert!(help.contains("claude mcp add lanes -- lane mcp"), "{help}");
    for tool in [
        "repo.read",
        "repo.outline",
        "repo.grep",
        "lane.new",
        "lane.list",
    ] {
        assert!(help.contains(tool), "{tool}: {help}");
    }
    let out = run(&dir, &["--help"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("\n  mcp "));

    let out = run(&dir, &["mcp", "--port", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["error"]["code"], "usage");
    assert_eq!(
        error["error"]["hint"],
        "Run `lane mcp --help` to see the options."
    );
}
