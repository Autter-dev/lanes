//! `lane hook claude <event>`: what Claude Code's hooks run. Each reads Claude Code's JSON on
//! stdin and answers the way Claude Code expects:
//! - `worktree-create` prints the lane's folder as its only line on stdout (`claude -w`,
//!   subagents with `isolation: worktree`, background sessions);
//! - `worktree-remove` removes the lane;
//! - `session-start` puts the lane's ports in the session's environment and tells the agent
//!   where it is;
//! - `pre-tool-use` turns down `git worktree add` in a Lanes repo, pointing to `lane new`.
//!
//! `lane init` adds these hooks. The last two never get in the way: if anything goes wrong they
//! stay silent and let the session carry on.

use crate::{print_json, report};
use lane_core::agents;
use lane_core::engine::{self, Repo};
use lane_core::error::{Error, ErrorKind, Result};
use lane_core::exit;
use lane_core::state::{Lane, Status, Store};
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

pub const HOOK_HELP: &str = "\
lane hook: what an agent's hooks run (`lane init` sets them up)

Usage: lane hook claude <event> [--json]

Reads Claude Code's hook input on stdin and answers in Claude Code's format.

Events:
  worktree-create   Make a lane for `claude -w` and print its folder (WorktreeCreate)
  worktree-remove   Remove the lane Claude Code is done with (WorktreeRemove)
  session-start     Give the session the lane's ports and tell it where it is (SessionStart)
  pre-tool-use      Turn down `git worktree add`, pointing to `lane new` (PreToolUse)

Options:
  --json       For worktree-create and worktree-remove: print the result as JSON
  -h, --help   Show this help
";

pub fn hook(args: &[String]) -> ExitCode {
    let json = crate::args::wants_json(args);
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();
    match rest.as_slice() {
        [] | ["-h" | "--help"] | [_, "-h" | "--help"] => {
            print!("{HOOK_HELP}");
            ExitCode::from(exit::OK)
        }
        ["claude", "worktree-create"] => done(worktree_create(json), json),
        ["claude", "worktree-remove"] => done(worktree_remove(json), json),
        // These two never stop a session: a problem is noted on stderr and nothing more.
        ["claude", "session-start"] => quiet(session_start()),
        ["claude", "pre-tool-use"] => quiet(pre_tool_use()),
        [agent, ..] if *agent != "claude" => report(
            Some("hook"),
            &Error::usage(format!("no hooks for `{agent}` yet"))
                .hint("Only Claude Code has hooks so far: `lane hook claude <event>`."),
            json,
        ),
        _ => report(
            Some("hook"),
            &Error::usage("unknown hook event").hint(
                "Use worktree-create, worktree-remove, session-start or pre-tool-use. Run `lane hook --help`.",
            ),
            json,
        ),
    }
}

fn done(result: Result<()>, json: bool) -> ExitCode {
    match result {
        Ok(()) => ExitCode::from(exit::OK),
        Err(e) => report(Some("hook"), &e, json),
    }
}

fn quiet(result: Result<()>) -> ExitCode {
    if let Err(e) = result {
        eprintln!("lane hook: {e}");
    }
    ExitCode::from(exit::OK)
}

/// Claude Code's hook input: one JSON object on stdin.
fn input() -> Result<Value> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(Error::usage("`lane hook` reads a hook's input on stdin")
            .hint("Claude Code runs it for you once `lane init` has set up the hooks; there's no need to run it by hand."));
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text).map_err(|e| {
        Error::new(ErrorKind::Io, format!("couldn't read the hook input: {e}"))
            .hint("Run it from a Claude Code hook, which passes its input on stdin.")
    })?;
    serde_json::from_str(&text).map_err(|e| {
        Error::usage(format!("the hook input isn't JSON ({e})"))
            .hint("Run it from a Claude Code hook, which passes JSON on stdin.")
    })
}

fn cwd(input: &Value) -> PathBuf {
    input["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(crate::here)
}

/// `claude -w <name>` and friends: make the lane (or find the one with that name) and print its
/// folder, the only thing on stdout. A second copy of this hook running at once, or a retry,
/// gets the same lane.
fn worktree_create(json: bool) -> Result<()> {
    let input = input()?;
    let session = input["session_id"].as_str().unwrap_or_default();
    let name = agents::lane_name(input["name"].as_str().unwrap_or_default(), session);
    let repo = Repo::open(&cwd(&input))?;
    let mut store = Store::open()?;
    let lane = match existing(&store, &repo, &name)? {
        Some(lane) => lane,
        None => {
            let request = format!("claude:{session}:{name}");
            let request = (session.len() + name.len() < 110).then_some(request);
            let made =
                engine::new_lane(&mut store, &repo, &name, Some("claude"), request.as_deref())?;
            crate::copied_note(&made);
            made.lane
        }
    };
    if json {
        print_json(&json!({ "lane": lane }));
    } else {
        println!("{}", lane.path.display());
    }
    Ok(())
}

/// The ready lane called `name`, waiting while another `lane new` of it finishes.
fn existing(store: &Store, repo: &Repo, name: &str) -> Result<Option<Lane>> {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        match store.lane(&repo.root, name)? {
            Some(l) if l.status == Status::Ready => return Ok(Some(l)),
            Some(l) if l.is_stale() => return Ok(None),
            Some(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            Some(l) => {
                return Err(Error::new(
                    ErrorKind::Conflict,
                    format!("the lane `{}` is still being created", l.name),
                )
                .hint("Wait for it to finish, or pick another name with `claude -w <name>`."));
            }
            None => return Ok(None),
        }
    }
}

/// Claude Code is done with a worktree: remove its lane. A worktree Lanes doesn't know is
/// removed the way Claude Code would have, with Git.
fn worktree_remove(json: bool) -> Result<()> {
    let input = input()?;
    let path = PathBuf::from(input["worktree_path"].as_str().ok_or_else(|| {
        Error::usage("the hook input has no `worktree_path`")
            .hint("Run it from Claude Code's WorktreeRemove hook.")
    })?);
    let lane = match Store::open_existing()? {
        Some(store) => store.lane_at(&engine::norm(&path))?,
        None => None,
    };
    let removed = match lane {
        Some(lane) => {
            let repo = Repo::open(&lane.repo)?;
            let mut store = Store::open()?;
            Some(engine::remove(&mut store, &repo, &lane.name, false, None)?)
        }
        None => {
            if path.exists() {
                let repo = Repo::open(&path)?;
                lane_core::git::worktree_remove(&repo.root, &path)?;
            }
            None
        }
    };
    if path.exists() {
        return Err(Error::new(
            ErrorKind::Failure,
            format!("{} is still there", path.display()),
        )
        .hint("Close any program using files in it, then remove it with `lane rm <name>`."));
    }
    if json {
        print_json(&json!({ "removed": removed, "path": path }));
    }
    Ok(())
}

/// A session starts: in a lane, put its ports in the session's environment and say where it
/// is; in a Lanes repo outside a lane, say how to get one. Elsewhere, nothing.
fn session_start() -> Result<()> {
    let input = input()?;
    let cwd = cwd(&input);
    let Ok(repo) = Repo::open(&cwd) else {
        return Ok(());
    };
    let store = Store::open_existing()?;
    let top = engine::norm(&lane_core::repo::top_level(&cwd)?);
    let lane = match &store {
        Some(s) => s.lane_at(&top)?,
        None => None,
    };
    let context = match lane {
        Some(lane) if lane.status == Status::Ready => {
            if let Some(file) = std::env::var_os("CLAUDE_ENV_FILE").filter(|f| !f.is_empty()) {
                write_env(Path::new(&file), &lane)?;
            }
            agents::context_in_lane(&repo, &lane)
        }
        Some(_) => return Ok(()),
        None if is_lanes_repo(&repo, store.as_ref())? => agents::context_in_repo(),
        None => return Ok(()),
    };
    print_json(&json!({
        "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context }
    }));
    Ok(())
}

/// Append the lane's variables to Claude Code's env file, once.
fn write_env(file: &Path, lane: &Lane) -> Result<()> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    if text.contains(&format!("export LANE_ID={}\n", lane.id)) {
        return Ok(());
    }
    let mut out = text;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&agents::env_lines(lane));
    std::fs::write(file, out).map_err(|e| {
        Error::new(
            ErrorKind::Io,
            format!("couldn't write {}: {e}", file.display()),
        )
        .hint("The session still works; `lane env` prints the lane's variables.")
    })
}

/// Before a shell command runs: turn down `git worktree add` in a Lanes repo.
fn pre_tool_use() -> Result<()> {
    let input = input()?;
    let command = input["tool_input"]["command"].as_str().unwrap_or_default();
    // Most commands stop here, before any Git or disk work.
    if !agents::makes_worktree(command) {
        return Ok(());
    }
    let Ok(repo) = Repo::open(&cwd(&input)) else {
        return Ok(());
    };
    if !is_lanes_repo(&repo, Store::open_existing()?.as_ref())? {
        return Ok(());
    }
    print_json(&json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": agents::WORKTREE_DENIED,
        }
    }));
    Ok(())
}

/// A repo set up for Lanes: it has a lane.toml, or lanes.
fn is_lanes_repo(repo: &Repo, store: Option<&Store>) -> Result<bool> {
    if repo.has_lane_toml() {
        return Ok(true);
    }
    Ok(match store {
        Some(s) => !s.lanes(Some(&repo.root))?.is_empty(),
        None => false,
    })
}
