//! Putting agents in lanes. Claude Code gets hooks: `claude -w` makes a lane, a session in a lane
//! learns its ports, and `git worktree add` is turned down with a pointer to `lane new`. Every
//! other tool's worktrees become lanes through a Git `post-checkout` hook. `AGENTS.md` gets a
//! short note. `lane init` writes all of it; everything here is safe to run again.

use crate::engine::Repo;
use crate::error::{Error, ErrorKind, Result};
use crate::lane_env;
use crate::state::Lane;
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Claude Code's settings that stay on this machine. Hooks go here, not in the shared
/// `.claude/settings.json`, so a teammate without `lane` installed isn't broken by them.
pub const CLAUDE_SETTINGS: &str = ".claude/settings.local.json";

/// Files that aren't checked in but that an agent in a lane needs, copied into each new lane.
pub const LOCAL_AGENT_FILES: [&str; 1] = [CLAUDE_SETTINGS];

/// Every hook Lanes adds starts with this, which is how a second `lane init` finds them.
const HOOK_PREFIX: &str = "lane hook claude";

/// What one setup step did, for `lane init`'s report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub path: PathBuf,
    /// False when it was already set up, so nothing was written.
    pub changed: bool,
    /// Why it was skipped or what to know, when there's something to say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A lane name from whatever a tool called its worktree: lowercase letters, digits and
/// hyphens, at most 40 characters. `Fix Auth_2` becomes `fix-auth-2`.
pub fn lane_name(raw: &str, fallback: &str) -> String {
    let mut out = String::new();
    for c in raw.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut name: String = out.trim_matches('-').chars().take(40).collect();
    name = name.trim_end_matches('-').to_string();
    if name.is_empty() {
        let tail: String = fallback
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect::<String>()
            .to_lowercase();
        name = if tail.is_empty() {
            "claude".into()
        } else {
            format!("claude-{tail}")
        };
    }
    name
}

/// Whether a shell command runs `git worktree add`, in any segment of it and with any of Git's
/// own options in between (`git -C ../app worktree add ...`).
pub fn makes_worktree(command: &str) -> bool {
    if !command.contains("worktree") {
        return false;
    }
    command.split(['\n', ';', '&', '|']).any(|segment| {
        let words = shell_words(segment);
        // The command word: past `NAME=value` assignments and wrappers like `sudo`.
        let Some(at) = words.iter().position(|w| {
            !w.contains('=')
                && !["sudo", "command", "exec", "env", "time", "nohup"].contains(&w.as_str())
        }) else {
            return false;
        };
        let program = words[at].rsplit(['/', '\\']).next().unwrap_or_default();
        if !program.eq_ignore_ascii_case("git") && !program.eq_ignore_ascii_case("git.exe") {
            return false;
        }
        let mut rest = words[at + 1..].iter();
        while let Some(w) = rest.next() {
            match w.as_str() {
                "-C" | "-c" => {
                    rest.next();
                }
                w if w.starts_with('-') => {}
                "worktree" => {
                    return rest.find(|w| !w.starts_with('-')).map(String::as_str) == Some("add");
                }
                _ => return false,
            }
        }
        false
    })
}

/// Split a command into words the way a shell would, near enough: whitespace separates, and
/// single or double quotes keep a word together (`"C:\Program Files\Git\cmd\git.exe"`).
fn shell_words(s: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, '(' | ')') => {}
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// Why `git worktree add` was turned down, for the agent to read.
pub const WORKTREE_DENIED: &str = "This repo uses Lanes, so make a lane instead of a Git worktree: \
run `lane new <name> --print-path` and work in the folder it prints. A lane is a real Git \
worktree with its own ports and .env, and `lane list` shows it. It starts from the base branch; \
to work on another branch, run `git switch <branch>` (or `git switch -c <branch>`) inside it.";

/// Lines for Claude Code's `CLAUDE_ENV_FILE`, so every command in the session has the lane's
/// ports. Only plain `export NAME=value` lines with values a shell takes as written, so Claude
/// Code's PowerShell tool can read them too.
pub fn env_lines(lane: &Lane) -> String {
    let mut out = String::new();
    for (k, v) in lane_env::vars(lane) {
        let v = if k == "LANE_PATH" {
            v.replace('\\', "/")
        } else {
            v
        };
        if v.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._:-".contains(c))
        {
            out.push_str(&format!("export {k}={v}\n"));
        }
    }
    out
}

/// What a Claude Code session in a lane is told when it starts.
pub fn context_in_lane(repo: &Repo, lane: &Lane) -> String {
    let port = lane.ports.start;
    let run = match &repo.project.dev.command {
        Some(dev) => format!(
            " Start the app with `lane exec {} -- {}`.",
            lane.name,
            lane_env::expand(dev, &lane_env::vars(lane))
        ),
        None => String::new(),
    };
    format!(
        "You're working in the lane `{name}`, made by Lanes: a Git worktree of this repo with its \
         own ports, so it doesn't clash with other agents' lanes. PORT={port} and \
         LANE_URL={url} are set; ports {ports} belong to this lane.{run} Stay in this folder. \
         If you need another copy of the repo, run `lane new <name> --print-path` instead of \
         `git worktree add`.",
        name = lane.name,
        url = lane_env::url(lane),
        ports = format_args!("{}-{}", port, lane.ports.end()),
    )
}

/// What a Claude Code session in a Lanes repo, outside any lane, is told when it starts.
pub fn context_in_repo() -> String {
    "This repo uses Lanes. If you need a separate copy of the repo to work in, run \
     `lane new <name> --print-path` and work in the folder it prints, instead of \
     `git worktree add`. Each lane has its own ports and .env; `lane list` shows them."
        .to_string()
}

/// The hooks Lanes adds to Claude Code's settings.
pub fn claude_hooks() -> Value {
    let hook = |event: &str, timeout: u32| json!({ "type": "command", "command": format!("{HOOK_PREFIX} {event}"), "timeout": timeout });
    json!({
        "WorktreeCreate": [{ "hooks": [hook("worktree-create", 600)] }],
        "WorktreeRemove": [{ "hooks": [hook("worktree-remove", 120)] }],
        "SessionStart": [{ "hooks": [hook("session-start", 10)] }],
        "PreToolUse": [{ "matcher": "Bash|PowerShell", "hooks": [hook("pre-tool-use", 10)] }],
    })
}

/// Add Lanes' hooks to Claude Code settings, keeping everything else as it is. Returns whether
/// anything was added: a hook already there (any `lane hook claude` command) is left alone.
pub fn merge_claude_hooks(settings: &mut Value) -> Result<bool> {
    let not_an_object = || {
        Error::new(
            ErrorKind::Config,
            format!("{CLAUDE_SETTINGS} isn't a JSON object"),
        )
        .hint("Fix the file, or move it away, then run `lane init` again.")
    };
    let root = settings.as_object_mut().ok_or_else(not_an_object)?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(not_an_object)?;
    let mut changed = false;
    let Value::Object(ours) = claude_hooks() else {
        unreachable!("claude_hooks is an object")
    };
    for (event, groups) in ours {
        let list = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(not_an_object)?;
        let present = list.iter().any(|group| {
            group["hooks"].as_array().is_some_and(|hs| {
                hs.iter().any(|h| {
                    h["command"]
                        .as_str()
                        .is_some_and(|c| c.trim_start().starts_with(HOOK_PREFIX))
                })
            })
        });
        if !present {
            list.extend(groups.as_array().cloned().unwrap_or_default());
            changed = true;
        }
    }
    Ok(changed)
}

/// Write Lanes' hooks into the repo's `.claude/settings.local.json`, and keep that file out of
/// Git if nothing ignores it yet.
pub fn setup_claude(repo: &Repo) -> Result<Change> {
    let path = repo.root.join(CLAUDE_SETTINGS);
    let mut settings = match fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => json!({}),
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            Error::new(
                ErrorKind::Config,
                format!("{CLAUDE_SETTINGS} isn't valid JSON ({e})"),
            )
            .hint("Fix the file, then run `lane init` again to add the Claude Code hooks.")
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(io_error("read", &path, e)),
    };
    let changed = merge_claude_hooks(&mut settings)?;
    if changed {
        let text = serde_json::to_string_pretty(&settings).expect("settings serialize") + "\n";
        write_atomic(&path, &text)?;
    }
    ignore_locally(repo, CLAUDE_SETTINGS)?;
    Ok(Change {
        path,
        changed,
        note: None,
    })
}

/// Add `rel` to the repo's own `.git/info/exclude` unless Git already ignores it.
fn ignore_locally(repo: &Repo, rel: &str) -> Result<()> {
    let ignored = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args(["check-ignore", "-q", rel])
        .status()
        .is_ok_and(|s| s.success());
    if ignored {
        return Ok(());
    }
    let exclude = crate::git::git_path(&repo.root, "info/exclude")?;
    let mut text = fs::read_to_string(&exclude).unwrap_or_default();
    let line = format!("/{rel}");
    if text.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!(
        "# Added by `lane init`: Claude Code's settings for this machine.\n{line}\n"
    ));
    if let Some(dir) = exclude.parent() {
        fs::create_dir_all(dir).map_err(|e| io_error("create", dir, e))?;
    }
    write_atomic(&exclude, &text)
}

/// The note `lane init` adds to `AGENTS.md`. Its first line is how a second run finds it.
pub const AGENTS_NOTE: &str = "<!-- Added by `lane init`. Edit it freely; keep this line so it isn't added again. -->
## Lanes

This repo uses [Lanes](https://lanes.autter.dev) for parallel work. Each agent works in its own lane: a Git worktree with its own ports and `.env`.

- For a separate copy of the repo, run `lane new <name> --print-path` and work in the folder it prints. Don't use `git worktree add` or copy the repo.
- In a lane, `PORT` and `LANE_URL` are set. Start the dev server with `lane exec -- <command>` so it uses the lane's port.
- `lane list` shows every lane with its folder and ports.
";

const AGENTS_MARKER: &str = "<!-- Added by `lane init`.";

/// Add the Lanes note to `AGENTS.md` (made if there isn't one). Claude Code reads `AGENTS.md`
/// only when there's no `CLAUDE.md`, so a `CLAUDE.md` gets an `@AGENTS.md` line too.
pub fn setup_agents_md(repo: &Repo) -> Result<Vec<Change>> {
    let path = repo.root.join("AGENTS.md");
    let text = read_or_empty(&path)?;
    let changed = !text.contains(AGENTS_MARKER);
    if changed {
        let mut out = text.clone();
        if !out.is_empty() {
            out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
        }
        out.push_str(AGENTS_NOTE);
        write_atomic(&path, &out)?;
    }
    let mut changes = vec![Change {
        path,
        changed,
        note: None,
    }];
    let claude_md = repo.root.join("CLAUDE.md");
    if claude_md.is_file() {
        let text = read_or_empty(&claude_md)?;
        let has = text.lines().any(|l| l.trim() == "@AGENTS.md");
        if !has {
            let mut out = text;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("\n@AGENTS.md\n");
            write_atomic(&claude_md, &out)?;
        }
        changes.push(Change {
            path: claude_md,
            changed: !has,
            note: None,
        });
    }
    Ok(changes)
}

/// The Git hook that turns a worktree any tool makes into a lane.
pub const POST_CHECKOUT: &str = r#"#!/bin/sh
# Added by `lane init`: a worktree any tool makes becomes a lane, with its own ports and .env.
# A new worktree is checked out from nothing, so the previous HEAD ($1) is all zeros.
# Lanes' own Git commands set LANE_GIT, so its lanes aren't adopted twice.
case "$1" in
  *[!0]*) ;;
  *) if [ -z "$LANE_GIT" ] && command -v lane >/dev/null 2>&1; then lane adopt >/dev/null 2>&1; fi ;;
esac
before="$(dirname "$0")/post-checkout.before-lanes"
if [ -x "$before" ]; then exec "$before" "$@"; fi
exit 0
"#;

const HOOK_MARKER: &str = "# Added by `lane init`: a worktree any tool makes";

/// Install the `post-checkout` hook in the repo's hooks folder. A hook that's already there is
/// kept and run after ours. With `core.hooksPath` set (Husky, lefthook), nothing is written,
/// because that folder is usually checked in.
pub fn setup_git_hook(repo: &Repo) -> Result<Change> {
    if let Some(custom) = crate::git::try_run(&repo.root, &["config", "core.hooksPath"])
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(Change {
            path: PathBuf::from(custom.trim()),
            changed: false,
            note: Some(format!(
                "Git hooks are in {} (core.hooksPath), so Lanes didn't add its post-checkout hook there. Worktrees other tools make won't become lanes on their own; run `lane adopt <folder>` for them.",
                custom.trim()
            )),
        });
    }
    let path = crate::git::git_path(&repo.root, "hooks/post-checkout")?;
    let existing = read_or_empty(&path)?;
    if existing.contains(HOOK_MARKER) {
        return Ok(Change {
            path,
            changed: false,
            note: None,
        });
    }
    if path.exists() {
        let before = path.with_file_name("post-checkout.before-lanes");
        if !before.exists() {
            fs::rename(&path, &before).map_err(|e| io_error("move", &path, e))?;
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_error("create", dir, e))?;
    }
    write_atomic(&path, POST_CHECKOUT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(|e| io_error("make executable", &path, e))?;
    }
    Ok(Change {
        path,
        changed: true,
        note: None,
    })
}

/// Whether Claude Code is in use here: the repo has Claude Code files, or `claude` is on PATH.
pub fn uses_claude(repo: &Repo) -> bool {
    repo.root.join(".claude").is_dir() || repo.root.join("CLAUDE.md").is_file() || on_path("claude")
}

fn on_path(program: &str) -> bool {
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".ps1", ""]
    } else {
        &[""]
    };
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            exts.iter()
                .any(|e| dir.join(format!("{program}{e}")).is_file())
        })
    })
}

fn read_or_empty(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(io_error("read", path, e)),
    }
}

/// Write through a temporary file and a rename, so a crash never leaves half a file.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_error("create", dir, e))?;
    }
    let tmp = path.with_extension("lanes-tmp");
    fs::write(&tmp, text)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| io_error("write", path, e))
}

fn io_error(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(
        ErrorKind::Io,
        format!("couldn't {what} {}: {e}", path.display()),
    )
    .hint("Check that you can write to the repo's folder, then run `lane init` again.")
}

#[cfg(test)]
mod tests;
