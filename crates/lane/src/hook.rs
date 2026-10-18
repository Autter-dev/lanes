//! `lane hook claude <event>`: what Claude Code's hooks run. Each reads Claude Code's JSON on
//! stdin and answers the way Claude Code expects:
//! - `worktree-create` prints the lane's folder as its only line on stdout (`claude -w`,
//!   subagents with `isolation: worktree`, background sessions);
//! - `worktree-remove` removes the lane;
//! - `session-start` puts the lane's ports in the session's environment and tells the agent
//!   where it is;
//! - `pre-tool-use` turns down `git worktree add`, a clone of the repo or a copy of its folder in
//!   a Lanes repo, pointing to `lane new`.
//!
//! `lane hook codex|cursor|gemini|copilot <event>` do the same for those agents, in each one's
//! own format: a session that starts in a worktree the agent made makes it a lane, and a copy of
//! the repo is turned down. `lane setup` adds these hooks to each agent's user settings.
//!
//! `lane hook git post-checkout` is what a Git hook that Lanes doesn't own runs (Husky's, or
//! lefthook's): a new worktree becomes a lane.
//!
//! `lane init` adds these hooks. All but the first two never get in the way: if anything goes
//! wrong they stay silent and let the session, or the checkout, carry on.

use crate::{print_json, report};
use lane_core::agents;
use lane_core::engine::{self, Repo};
use lane_core::error::{Error, ErrorKind, Result};
use lane_core::exit;
use lane_core::state::{Lane, Status, Store};
use lane_core::writes::{self, Done, Via, Write};
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

pub const HOOK_HELP: &str = "\
lane hook: what an agent's hooks run (`lane init` sets them up)

Usage: lane hook claude <event> [--json]
       lane hook codex <session-start|pre-tool-use>
       lane hook cursor <session-start|before-shell>
       lane hook gemini <session-start|before-tool>
       lane hook copilot <session-start|pre-tool-use>
       lane hook git post-checkout <previous HEAD> <new HEAD> <flag> [--json]

`lane hook claude` reads Claude Code's hook input on stdin and answers in Claude Code's format.
The others do the same for Codex, Cursor, Gemini CLI and GitHub Copilot (the Copilot CLI and
VS Code), whose hooks `lane setup` adds: session-start makes a worktree the agent made a lane,
and the shell hooks turn down copies of the repo. `lane hook copilot pre-tool-use` exits 2 when
it turns one down, which is how Copilot hooks say no.

Events:
  worktree-create   Make a lane for `claude -w` and print its folder (WorktreeCreate)
  worktree-remove   Remove the lane Claude Code is done with (WorktreeRemove)
  session-start     Give the session the lane's ports and tell it where it is (SessionStart)
  pre-tool-use      Turn down `git worktree add`, a clone of the repo or a copy of its folder,
                    pointing to `lane new` (PreToolUse)

`lane hook git post-checkout` takes Git's post-checkout arguments: a new worktree (checked out
from nothing) becomes a lane. It's the line `lane init` adds to Husky's hook, and the command it
gives for lefthook. It never fails the checkout.

Options:
  --json       For worktree-create, worktree-remove and post-checkout: print the result as JSON
  -h, --help   Show this help
";

pub fn hook(args: &[String]) -> ExitCode {
    // An agent reads whatever a hook prints, so problems and installs go to the log instead.
    lane_core::log::set_quiet(true);
    let json = crate::args::wants_json(args);
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();
    match rest.as_slice() {
        [] | [.., "-h" | "--help"] => {
            print!("{HOOK_HELP}");
            ExitCode::from(exit::OK)
        }
        ["claude", "worktree-create"] => done(worktree_create(json), json),
        ["claude", "worktree-remove"] => done(worktree_remove(json), json),
        // These never stop a session or a checkout: a problem is noted on stderr and nothing more.
        ["claude", "session-start"] => quiet(session_start()),
        ["claude", "pre-tool-use"] => quiet(pre_tool_use()),
        ["cursor", "session-start"] => quiet(cursor_session_start()),
        ["codex", "session-start"] => quiet(codex_session_start()),
        ["codex", "pre-tool-use"] => quiet(codex_pre_tool_use()),
        ["cursor", "before-shell"] => quiet(cursor_before_shell()),
        ["gemini", "session-start"] => quiet(gemini_session_start()),
        ["gemini", "before-tool"] => quiet(gemini_before_tool()),
        ["copilot", "session-start"] => quiet(copilot_session_start()),
        ["copilot", "pre-tool-use"] => match copilot_pre_tool_use() {
            Ok(Some(_)) => ExitCode::from(COPILOT_DENY),
            other => quiet(other.map(|_| ())),
        },
        ["git", "post-checkout", git_args @ ..] => quiet(post_checkout(git_args, json)),
        ["git", ..] => report(
            Some("hook"),
            &Error::usage("unknown Git hook").hint(
                "Lanes has one Git hook: `lane hook git post-checkout <previous HEAD> <new HEAD> <flag>`.",
            ),
            json,
        ),
        [agent, ..] if *agent != "claude" => report(
            Some("hook"),
            &Error::usage(format!("no hooks for `{agent}` yet")).hint(
                "Lanes has hooks for Claude Code, Codex, Cursor, Gemini CLI, GitHub Copilot and Git: `lane hook claude|codex|cursor|gemini|copilot <event>` and `lane hook git post-checkout`. Run `lane hook --help`.",
            ),
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

/// Copilot's code for "turned down": its hooks read exit 2 as a deny, and VS Code gives stderr to
/// the model. It belongs to Copilot's hooks, not to `lane`'s own exit codes.
const COPILOT_DENY: u8 = 2;

fn quiet(result: Result<()>) -> ExitCode {
    if let Err(e) = result {
        lane_core::log::error("hook", e.to_string()).write();
    }
    ExitCode::from(exit::OK)
}

/// Claude Code's hook input: one JSON object on stdin.
fn input() -> Result<Value> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(Error::usage("`lane hook` reads a hook's input on stdin")
            .hint("Your agent runs it for you once `lane setup` or `lane init` has set up its hooks; there's no need to run it by hand."));
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text).map_err(|e| {
        Error::new(ErrorKind::Io, format!("couldn't read the hook input: {e}"))
            .hint("Run it from a Claude Code hook, which passes its input on stdin.")
    })?;
    // Cursor on Windows starts its JSON with a byte-order mark.
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| {
        Error::usage(format!("the hook input isn't JSON ({e})"))
            .hint("Run it from a Claude Code hook, which passes JSON on stdin.")
    })
}

/// Make a change to the lanes from a hook: through `laned` if it's running (hooks never start
/// it), otherwise here. Install output goes to the log, since the agent reads this hook's output.
fn write(write: &Write) -> Result<Done> {
    writes::make(write, Via::DaemonIfRunning, &mut writes::to_stderr_or_log)
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
    let store = Store::open()?;
    let lane = match existing(&store, &repo, &name)? {
        Some(lane) => lane,
        None => {
            let request = format!("claude:{session}:{name}");
            let request = (session.len() + name.len() < 110).then_some(request);
            let new = Write::New {
                repo: repo.root.clone(),
                name: name.clone(),
                agent: Some("claude".into()),
                request_id: request,
            };
            let made = write(&new)?.made()?;
            crate::copied_note(&made);
            made.lane
        }
    };
    lane_core::log::info(
        "hook claude worktree-create",
        format!("Claude Code works in {}", lane.path.display()),
    )
    .lane(&lane.name)
    .repo(&repo.root)
    .write();
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
            let remove = Write::Remove {
                repo: lane.repo.clone(),
                name: lane.name.clone(),
                keep_files: false,
                request_id: None,
            };
            Some(write(&remove)?.lane()?)
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

/// Before a shell command runs: in a Lanes repo, turn down `git worktree add`, a clone of the
/// repo and a recursive copy of its folder, each with its own reason.
fn pre_tool_use() -> Result<()> {
    let input = input()?;
    let command = input["tool_input"]["command"].as_str().unwrap_or_default();
    let powershell = input["tool_name"].as_str() == Some("PowerShell");
    let Some(reason) = denied(command, powershell, &cwd(&input))? else {
        return Ok(());
    };
    print_json(&json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }));
    Ok(())
}

/// Why a shell command that copies the repo (a worktree, a clone, a recursive copy) should be
/// turned down, in a Lanes repo. Most commands stop at the first check, before any Git or disk
/// work.
fn denied(command: &str, powershell: bool, cwd: &Path) -> Result<Option<&'static str>> {
    let found = agents::repo_copies(command, powershell);
    if found.is_empty() {
        return Ok(None);
    }
    let Ok(repo) = Repo::open(cwd) else {
        return Ok(None);
    };
    if !is_lanes_repo(&repo, Store::open_existing()?.as_ref())? {
        return Ok(None);
    }
    let reason = agents::deny_reason(&found, cwd, &repo.root);
    if reason.is_some() {
        lane_core::log::warn(
            "hook deny",
            format!("turned down a copy of the repo: {command}"),
        )
        .repo(&repo.root)
        .write();
    }
    Ok(reason)
}

/// The folder a Cursor hook is about: the shell command's `cwd`, else the first workspace root.
/// (Cursor runs user-level hooks from `~/.cursor`, so the process's own folder says nothing.)
fn cursor_folder(input: &Value) -> Option<PathBuf> {
    input["cwd"]
        .as_str()
        .or_else(|| input["workspace_roots"][0].as_str())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// A Cursor agent starts. Cursor makes its own worktrees (`~/.cursor/worktrees/...`) and its
/// Git hooks can't be relied on, so a worktree it starts in becomes a lane here, ready to run
/// like one from `lane new`. Then the agent is told which lane it's in.
fn cursor_session_start() -> Result<()> {
    let input = input()?;
    let Some(folder) = cursor_folder(&input) else {
        return Ok(());
    };
    if let Some(context) = adopting_session("cursor", &folder)? {
        print_json(&json!({ "additional_context": context }));
    }
    Ok(())
}

/// A Codex session starts. Codex makes its worktrees with Git hooks off, so one it starts in
/// becomes a lane here, ready to run. Then the agent is told which lane it's in.
fn codex_session_start() -> Result<()> {
    let input = input()?;
    if let Some(context) = adopting_session("codex", &cwd(&input))? {
        print_json(&json!({
            "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context }
        }));
    }
    Ok(())
}

/// Before Codex runs a shell command: turn down copies of the repo. Codex's shell is PowerShell
/// on Windows.
fn codex_pre_tool_use() -> Result<()> {
    let input = input()?;
    let command = input["tool_input"]["command"].as_str().unwrap_or_default();
    let Some(reason) = denied(command, cfg!(windows), &cwd(&input))? else {
        return Ok(());
    };
    print_json(&json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }));
    Ok(())
}

/// For an agent that makes its own worktrees out of Lanes' sight (Cursor, Codex, Gemini CLI,
/// Copilot): when a session starts in one, make it a lane, ready to run like one from `lane new`. Returns what to tell
/// the agent: which lane it's in, or how to get one in a Lanes repo; nothing elsewhere.
fn adopting_session(agent: &str, folder: &Path) -> Result<Option<String>> {
    let Ok(repo) = Repo::open(folder) else {
        return Ok(None);
    };
    let top = engine::norm(&lane_core::repo::top_level(folder)?);
    let store = Store::open()?;
    let lane = match store.lane_at(&top)? {
        Some(lane) => Some(lane),
        None if !engine::same_path(&top, &repo.root) => {
            let name = free_name(&store, &repo, &worktree_name(agent, &top))?;
            let lane = write(&Write::Adopt {
                repo: repo.root.clone(),
                path: top.clone(),
                name: Some(name),
                agent: Some(agent.into()),
                request_id: None,
            })?
            .lane()?;
            let log = |e: lane_core::log::Entry| e.lane(&lane.name).repo(&repo.root).write();
            let ready = Write::Ready {
                repo: repo.root.clone(),
                path: lane.path.clone(),
            };
            match write(&ready) {
                Ok(_) => log(lane_core::log::info(
                    format!("hook {agent} session-start"),
                    format!("{}'s worktree became a lane: {}", agent, top.display()),
                )),
                Err(e) => log(lane_core::log::error(
                    format!("hook {agent} session-start"),
                    format!("a lane, but not ready to run: {e}"),
                )),
            }
            Some(lane)
        }
        None => None,
    };
    Ok(match lane {
        Some(lane) if lane.status == Status::Ready => Some(agents::context_in_lane(&repo, &lane)),
        Some(_) => None,
        None if is_lanes_repo(&repo, Some(&store))? => Some(agents::context_in_repo()),
        None => None,
    })
}

/// A lane name for an agent's own worktree. Codex names every worktree folder after the repo,
/// under a short id (`.../a1b2/app`), so its lanes are `codex-a1b2`; Cursor's folder is the id
/// (`.../app/zjm`), so its lanes are `cursor-zjm`; `gemini -w fix-auth` makes
/// `.gemini/worktrees/fix-auth`, so its lane is `gemini-fix-auth`.
fn worktree_name(agent: &str, top: &Path) -> String {
    let file = |p: Option<&Path>| {
        p.and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let folder = file(Some(top));
    let parent = file(top.parent());
    let id =
        if agent == "codex" && parent.len() == 4 && parent.bytes().all(|b| b.is_ascii_hexdigit()) {
            parent
        } else {
            folder
        };
    agents::lane_name(&format!("{agent}-{id}"), agent)
}

/// `name`, or `name-2`, `name-3`... when another lane in the repo has it already.
fn free_name(store: &Store, repo: &Repo, name: &str) -> Result<String> {
    if store.lane(&repo.root, name)?.is_none() {
        return Ok(name.to_string());
    }
    for n in 2..100 {
        let stem: String = name.chars().take(40 - 1 - n.to_string().len()).collect();
        let candidate = format!("{}-{n}", stem.trim_end_matches('-'));
        if store.lane(&repo.root, &candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    Err(Error::new(
        ErrorKind::Conflict,
        format!("every name like `{name}` is taken"),
    )
    .hint("Remove lanes you're done with (`lane list`, then `lane rm <name>`)."))
}

/// Before a Cursor agent runs a shell command: turn down copies of the repo, with a message the
/// agent reads. Anything else gets no answer, so Cursor's own approval rules still apply.
fn cursor_before_shell() -> Result<()> {
    let input = input()?;
    let command = input["command"].as_str().unwrap_or_default();
    let Some(folder) = cursor_folder(&input) else {
        return Ok(());
    };
    let powershell = cfg!(windows);
    let Some(reason) = denied(command, powershell, &folder)? else {
        return Ok(());
    };
    print_json(&json!({
        "permission": "deny",
        "user_message": "Lanes turned this down: this repo uses lanes instead of copies.",
        "agent_message": reason,
    }));
    Ok(())
}

/// A Gemini CLI session starts (or resumes, or is cleared). A worktree `gemini -w` made becomes a
/// lane here, ready to run, and the agent is told which lane it's in.
fn gemini_session_start() -> Result<()> {
    let input = input()?;
    if let Some(context) = adopting_session("gemini", &cwd(&input))? {
        print_json(&json!({
            "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context }
        }));
    }
    Ok(())
}

/// Before Gemini CLI runs a shell command (`run_shell_command`, in PowerShell on Windows): turn
/// down copies of the repo. The reason goes to the model as the tool's error, and a line to you.
fn gemini_before_tool() -> Result<()> {
    let input = input()?;
    if input["tool_name"]
        .as_str()
        .is_some_and(|t| t != "run_shell_command")
    {
        return Ok(());
    }
    let command = input["tool_input"]["command"].as_str().unwrap_or_default();
    // `dir_path` is where the command runs, from the workspace or absolute.
    let folder = match input["tool_input"]["dir_path"].as_str() {
        Some(dir) if !dir.is_empty() => cwd(&input).join(dir),
        _ => cwd(&input),
    };
    let Some(reason) = denied(command, cfg!(windows), &folder)? else {
        return Ok(());
    };
    print_json(&json!({
        "decision": "deny",
        "reason": reason,
        "systemMessage": "Lanes turned this down: this repo uses lanes instead of copies.",
    }));
    Ok(())
}

/// A Copilot session starts, in the Copilot CLI or VS Code. A worktree Copilot made becomes a
/// lane here, ready to run, and the agent is told which lane it's in: the CLI reads
/// `additionalContext`, VS Code `hookSpecificOutput`. (VS Code may leave out `cwd`; it runs hooks
/// in the workspace.)
fn copilot_session_start() -> Result<()> {
    let input = input()?;
    if let Some(context) = adopting_session("copilot", &cwd(&input))? {
        print_json(&json!({
            "additionalContext": context,
            "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context },
        }));
    }
    Ok(())
}

/// Before Copilot runs a tool: turn down a shell command that copies the repo. The Copilot CLI
/// sends `toolName` (`bash` or `powershell`) and `toolArgs`, VS Code `tool_name` and
/// `tool_input`; any tool with a `command` is a shell here. A deny goes out as JSON for the CLI,
/// which shows its reason to the agent, and on stderr for VS Code, which gives that to the model;
/// the caller then exits 2. Returns the reason.
fn copilot_pre_tool_use() -> Result<Option<&'static str>> {
    let input = input()?;
    let tool = input["toolName"]
        .as_str()
        .or(input["tool_name"].as_str())
        .unwrap_or_default();
    let args = match input.get("toolArgs").or(input.get("tool_input")) {
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(Value::Null),
        Some(args) => args.clone(),
        None => Value::Null,
    };
    let Some(command) = args["command"].as_str() else {
        return Ok(None);
    };
    let powershell = match tool.to_ascii_lowercase().as_str() {
        "bash" => false,
        "powershell" => true,
        _ => cfg!(windows),
    };
    let reason = denied(command, powershell, &cwd(&input))?;
    if let Some(reason) = reason {
        print_json(&json!({ "permissionDecision": "deny", "permissionDecisionReason": reason }));
        eprintln!("{reason}");
    }
    Ok(reason)
}

/// Git's post-checkout hook: a new worktree, checked out from nothing so the previous HEAD is
/// all zeros, becomes a lane. Not one Lanes made itself (its Git commands set LANE_GIT), and not
/// the repo's own folder after a `git clone`.
fn post_checkout(git_args: &[&str], json: bool) -> Result<()> {
    let new = git_args
        .first()
        .is_some_and(|head| !head.is_empty() && head.bytes().all(|b| b == b'0'));
    let ours = std::env::var_os("LANE_GIT").is_some_and(|v| !v.is_empty());
    let lane = if new && !ours { adopt_here()? } else { None };
    if json {
        print_json(&json!({ "lane": lane }));
    }
    Ok(())
}

fn adopt_here() -> Result<Option<Lane>> {
    let here = crate::here();
    let repo = Repo::open(&here)?;
    let top = engine::norm(&lane_core::repo::top_level(&here)?);
    if engine::same_path(&top, &repo.root) {
        return Ok(None);
    }
    let lane = write(&Write::Adopt {
        repo: repo.root.clone(),
        path: top.clone(),
        name: None,
        agent: None,
        request_id: None,
    })?
    .lane()?;
    // Ready to run, like a lane from `lane new`. A worktree that can't be readied is still a
    // lane, and the checkout never fails over it.
    let ready = Write::Ready {
        repo: repo.root.clone(),
        path: lane.path.clone(),
    };
    let entry = match write(&ready) {
        Ok(_) => lane_core::log::info(
            "hook git post-checkout",
            format!("a new worktree became a lane: {}", top.display()),
        ),
        Err(e) => lane_core::log::error(
            "hook git post-checkout",
            format!("a lane, but not ready to run: {e}"),
        ),
    };
    entry.lane(&lane.name).repo(&repo.root).write();
    Ok(Some(lane))
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
