//! Putting agents in lanes. Claude Code gets hooks: `claude -w` makes a lane, a session in a lane
//! learns its ports, and `git worktree add`, a clone of the repo or a copy of its folder is
//! turned down with a pointer to `lane new`. Every other tool's worktrees become lanes through a
//! Git `post-checkout` hook (or a line in Husky's, or a command in lefthook's config). `AGENTS.md`
//! gets a short note. `lane init` writes all of it; everything here is safe to run again.

mod copies;
pub mod user;

pub use copies::{RepoCopy, deny_reason, makes_worktree, repo_copies};

use crate::engine::{Repo, same_path};
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Change {
    pub path: PathBuf,
    /// False when it was already set up, so nothing was written.
    pub changed: bool,
    /// The file is checked in, so a change to it should be committed: `lane init` lists it in
    /// its `git add` step.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub check_in: bool,
    /// Why it was skipped or what to know, when there's something to say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Text to add to `path` by hand, when Lanes leaves a file that isn't its own alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
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

/// Why `git worktree add` was turned down, for the agent to read.
pub const WORKTREE_DENIED: &str = "This repo uses Lanes, so make a lane instead of a Git worktree: \
run `lane new <name> --print-path` and work in the folder it prints. A lane is a real Git \
worktree with its own ports and .env, and `lane list` shows it. It starts from the base branch; \
to work on another branch, run `git switch <branch>` (or `git switch -c <branch>`) inside it.";

/// Why a `git clone` of this same repo was turned down.
pub const CLONE_DENIED: &str = "This repo uses Lanes, so make a lane instead of cloning the repo \
again: run `lane new <name> --print-path` and work in the folder it prints. A lane is a Git \
worktree of this same repo, with its own ports and .env: it shares this repo's commits and \
branches, so there's nothing to push or pull between copies, and `lane list` shows it. To work \
on another branch, run `git switch <branch>` (or `git switch -c <branch>`) inside it.";

/// Why a recursive copy of the repo's folder was turned down.
pub const COPY_DENIED: &str = "This repo uses Lanes, so make a lane instead of copying the repo's \
folder: run `lane new <name> --print-path` and work in the folder it prints. A lane is a Git \
worktree of this same repo, with its own ports and .env, and `lane list` shows it; a copied \
folder would share this one's ports and drift away from its branches. If you need the files \
outside Git (an export or a deploy), use `git archive`, or leave `.git` out of the copy.";

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
         `git worktree add`, a clone or a copy of the folder.",
        name = lane.name,
        url = lane_env::url(lane),
        ports = format_args!("{}-{}", port, lane.ports.end()),
    )
}

/// What a Claude Code session in a Lanes repo, outside any lane, is told when it starts.
pub fn context_in_repo() -> String {
    "This repo uses Lanes. If you need a separate copy of the repo to work in, run \
     `lane new <name> --print-path` and work in the folder it prints, instead of \
     `git worktree add`, a clone or a copy of the folder. Each lane has its own ports and .env; \
     `lane list` shows them."
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

/// Every Cursor hook Lanes adds starts with this.
const CURSOR_PREFIX: &str = "lane hook cursor";

/// The hooks Lanes adds to Cursor's `hooks.json`: a session in a Cursor worktree makes it a
/// lane, and a shell command that copies the repo is turned down.
pub fn cursor_hooks() -> Value {
    let hook = |event: &str, timeout: u32| json!({ "command": format!("{CURSOR_PREFIX} {event}"), "timeout": timeout });
    json!({
        "sessionStart": [hook("session-start", 300)],
        "beforeShellExecution": [hook("before-shell", 10)],
    })
}

fn is_cursor_hook(h: &Value) -> bool {
    h["command"]
        .as_str()
        .is_some_and(|c| c.trim_start().starts_with(CURSOR_PREFIX))
}

/// Add Lanes' hooks to Cursor's `hooks.json` (`{"version": 1, "hooks": {...}}`), keeping the
/// rest. Returns whether anything was added.
pub fn merge_cursor_hooks(file: &mut Value) -> Result<bool> {
    let not_an_object = || {
        Error::new(ErrorKind::Config, "Cursor's hooks.json isn't a JSON object")
            .hint("Fix ~/.cursor/hooks.json, or move it away, then run `lane setup` again.")
    };
    let root = file.as_object_mut().ok_or_else(not_an_object)?;
    root.entry("version").or_insert_with(|| json!(1));
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(not_an_object)?;
    let mut changed = false;
    let Value::Object(ours) = cursor_hooks() else {
        unreachable!("cursor_hooks is an object")
    };
    for (event, entries) in ours {
        let list = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(not_an_object)?;
        if !list.iter().any(is_cursor_hook) {
            list.extend(entries.as_array().cloned().unwrap_or_default());
            changed = true;
        }
    }
    Ok(changed)
}

/// Whether all of Lanes' Cursor hooks are in this `hooks.json`.
pub fn cursor_hooks_present(file: &Value) -> bool {
    let Value::Object(ours) = cursor_hooks() else {
        unreachable!("cursor_hooks is an object")
    };
    ours.keys().all(|event| {
        file["hooks"][event]
            .as_array()
            .is_some_and(|l| l.iter().any(is_cursor_hook))
    })
}

/// Take Lanes' hooks back out of Cursor's `hooks.json`, and nothing else.
pub fn remove_cursor_hooks(file: &mut Value) -> bool {
    let Some(hooks) = file.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    for list in hooks.values_mut().filter_map(Value::as_array_mut) {
        let before = list.len();
        list.retain(|h| !is_cursor_hook(h));
        changed |= list.len() != before;
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|l| !l.is_empty()));
    changed
}

/// Whether a hook group holds one of Lanes' hooks.
fn is_lanes_group(group: &Value) -> bool {
    group["hooks"].as_array().is_some_and(|hs| {
        hs.iter().any(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.trim_start().starts_with(HOOK_PREFIX))
        })
    })
}

/// Whether every one of Lanes' hooks is in these Claude Code settings.
pub fn claude_hooks_present(settings: &Value) -> bool {
    let Value::Object(ours) = claude_hooks() else {
        unreachable!("claude_hooks is an object")
    };
    ours.keys().all(|event| {
        settings["hooks"][event]
            .as_array()
            .is_some_and(|list| list.iter().any(is_lanes_group))
    })
}

/// Take Lanes' hooks back out of Claude Code settings, and nothing else: an event left with no
/// hooks goes, and so does an empty `hooks`. Returns whether anything was removed.
pub fn remove_claude_hooks(settings: &mut Value) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    for list in hooks.values_mut().filter_map(Value::as_array_mut) {
        let before = list.len();
        list.retain(|g| !is_lanes_group(g));
        changed |= list.len() != before;
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|l| !l.is_empty()));
    if hooks.is_empty()
        && let Some(root) = settings.as_object_mut()
    {
        root.remove("hooks");
    }
    changed
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
        ..Change::default()
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
        check_in: true,
        ..Change::default()
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
            check_in: true,
            ..Change::default()
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
  *) if [ -z "$LANE_GIT" ] && command -v lane >/dev/null 2>&1; then lane hook git post-checkout "$@" >/dev/null 2>&1; fi ;;
esac
before="$(dirname "$0")/post-checkout.before-lanes"
if [ -x "$before" ]; then exec "$before" "$@"; fi
exit 0
"#;

const HOOK_MARKER: &str = "# Added by `lane init`: a worktree any tool makes";

/// The same hook as one line, for a post-checkout script Lanes doesn't own: Husky's, which is
/// checked in, or one in another `core.hooksPath` folder. `lane hook git post-checkout` does the
/// work, so the line never needs to change. Where `lane` isn't installed it does nothing, and it
/// never fails the checkout.
pub const HOOK_LINE: &str = "command -v lane >/dev/null 2>&1 && lane hook git post-checkout \"$@\" >/dev/null 2>&1 || true # Added by `lane init`: a worktree any tool makes becomes a lane";

/// How a second `lane init` finds the line, or the lefthook command, whatever its comment says.
const HOOK_LINE_MARKER: &str = "lane hook git post-checkout";

/// The command for lefthook's config; lefthook puts the hook's arguments in `{0}`.
const LEFTHOOK_RUN: &str =
    "command -v lane >/dev/null 2>&1 && lane hook git post-checkout {0} >/dev/null 2>&1 || true";

/// Where lefthook looks for its config, in order. Each has a `lefthook-local` twin.
const LEFTHOOK_CONFIGS: [&str; 15] = [
    "lefthook.yml",
    ".lefthook.yml",
    "lefthook.yaml",
    ".lefthook.yaml",
    ".config/lefthook.yml",
    ".config/lefthook.yaml",
    "lefthook.toml",
    ".lefthook.toml",
    ".config/lefthook.toml",
    "lefthook.json",
    ".lefthook.json",
    ".config/lefthook.json",
    "lefthook.jsonc",
    ".lefthook.jsonc",
    ".config/lefthook.jsonc",
];

/// Set up the `post-checkout` hook that turns a worktree any tool makes into a lane:
/// - lefthook running post-checkout: nothing is written, since lefthook moves aside hooks it
///   didn't write; the note gives the command to add to its config.
/// - Husky 9 (`core.hooksPath` is `.husky/_`): one line goes first in `.husky/post-checkout`,
///   which is checked in.
/// - Any other `core.hooksPath`: nothing is written; the note gives the line to add.
/// - Otherwise the hook goes in the repo's own hooks folder. A hook that's already there is kept
///   and run after ours.
pub fn setup_git_hook(repo: &Repo) -> Result<Change> {
    if let Some(change) = lefthook(&repo.root)? {
        return Ok(change);
    }
    if let Some(custom) = hooks_path(repo)? {
        return custom_hooks(repo, &custom);
    }
    let path = crate::git::git_path(&repo.root, "hooks/post-checkout")?;
    let existing = read_or_empty(&path)?;
    if existing == POST_CHECKOUT {
        return Ok(Change {
            path,
            ..Change::default()
        });
    }
    // An older copy of ours (0.0.4 ran `lane adopt`) is replaced; anyone else's is kept below.
    if existing.contains(HOOK_MARKER) {
        write_atomic(&path, POST_CHECKOUT)?;
        return Ok(Change {
            path,
            changed: true,
            ..Change::default()
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
        ..Change::default()
    })
}

/// lefthook writes the hook files for the hooks in its config, and moves any other file in the
/// way aside as `<hook>.old`, which it never runs. So when it runs post-checkout here, Lanes
/// writes nothing and says what to add to lefthook's config. `None` when lefthook doesn't run
/// post-checkout here, so the hook file is free.
fn lefthook(root: &Path) -> Result<Option<Change>> {
    let mut configs = Vec::new();
    for local in [false, true] {
        let found = LEFTHOOK_CONFIGS
            .iter()
            .map(|n| match local {
                true => n.replace("lefthook", "lefthook-local"),
                false => n.to_string(),
            })
            .find(|n| root.join(n).is_file());
        if let Some(name) = found {
            let text = read_or_empty(&root.join(&name))?;
            configs.push((name, text));
        }
    }
    let runs_it = |text: &str| {
        text.lines().any(|l| {
            l.trim_start()
                .trim_start_matches(['{', '[', '"', '\''])
                .starts_with("post-checkout")
        })
    };
    let Some((name, _)) = configs.iter().find(|(_, text)| runs_it(text)) else {
        return Ok(None);
    };
    let path = in_repo(root, name);
    if let Some((name, _)) = configs.iter().find(|(_, t)| t.contains(HOOK_LINE_MARKER)) {
        return Ok(Some(Change {
            path: in_repo(root, name),
            note: Some(format!(
                "a command in {name} (lefthook): worktrees other tools make become lanes (already set up)"
            )),
            ..Change::default()
        }));
    }
    let snippet = match name.rsplit('.').next() {
        Some("toml") => format!("[post-checkout.commands.lanes]\nrun = \"{LEFTHOOK_RUN}\"\n"),
        Some("json" | "jsonc") => format!(
            "\"post-checkout\": {{ \"commands\": {{ \"lanes\": {{ \"run\": \"{LEFTHOOK_RUN}\" }} }} }}\n"
        ),
        _ => format!("post-checkout:\n  commands:\n    lanes:\n      run: {LEFTHOOK_RUN}\n"),
    };
    Ok(Some(Change {
        path,
        note: Some(format!(
            "lefthook runs post-checkout here and moves aside hooks it didn't write, so Lanes left the hooks alone. For worktrees other tools make to become lanes, add the `lanes` command below to post-checkout in {name} and check it in:"
        )),
        snippet: Some(snippet),
        ..Change::default()
    }))
}

/// `core.hooksPath`, unless it's unset or names the repo's own hooks folder anyway.
fn hooks_path(repo: &Repo) -> Result<Option<String>> {
    let Some(custom) = crate::git::try_run(&repo.root, &["config", "--path", "core.hooksPath"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let own = crate::git::common_dir(&repo.root)?.join("hooks");
    Ok((!same_path(&repo.root.join(&custom), &own)).then_some(custom))
}

/// Hooks in a `core.hooksPath` folder. Husky 9's gets a line in its post-checkout script. Any
/// other is left alone, with the line to add: it may be checked in, or shared by every repo.
fn custom_hooks(repo: &Repo, custom: &str) -> Result<Change> {
    let folder = custom.replace('\\', "/");
    let folder = folder.trim_end_matches('/');
    // Husky 9 points Git at `.husky/_`, whose scripts run the ones in `.husky`.
    if let Some(husky) = folder.strip_suffix("/_")
        && husky.rsplit('/').next() == Some(".husky")
        && !Path::new(husky).has_root()
        && !husky.split('/').any(|c| c == "..")
    {
        return setup_husky(repo, husky);
    }
    Ok(Change {
        path: match Path::new(custom).has_root() {
            true => PathBuf::from(custom).join("post-checkout"),
            false => in_repo(&repo.root, &format!("{folder}/post-checkout")),
        },
        note: Some(format!(
            "Git hooks are in {custom} (core.hooksPath), so Lanes didn't change them. For worktrees other tools make to become lanes, add the line below to {folder}/post-checkout, an executable sh script:"
        )),
        snippet: Some(format!("{HOOK_LINE}\n")),
        ..Change::default()
    })
}

/// Husky 9's `.husky/post-checkout`: a plain sh script, checked in, made if it isn't there.
/// Lanes' line goes first (after a `#!` line), so an `exit` further down can't skip it and the
/// script's own commands still decide how it ends.
fn setup_husky(repo: &Repo, husky: &str) -> Result<Change> {
    let rel = format!("{husky}/post-checkout");
    let path = in_repo(&repo.root, &rel);
    let text = read_or_empty(&path)?;
    let changed = !text.contains(HOOK_LINE_MARKER);
    if changed {
        let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let at = match text.starts_with("#!") {
            true => text.find('\n').map_or(text.len(), |i| i + 1),
            false => 0,
        };
        let mut out = text[..at].to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(nl);
        }
        out.push_str(HOOK_LINE);
        out.push_str(nl);
        out.push_str(&text[at..]);
        let permissions = fs::metadata(&path).ok().map(|m| m.permissions());
        write_atomic(&path, &out)?;
        if let Some(p) = permissions {
            fs::set_permissions(&path, p).map_err(|e| io_error("keep the mode of", &path, e))?;
        }
    }
    let note = match changed {
        true => format!(
            "added a line to {rel} (Husky's hook, checked in): worktrees other tools make become lanes. It does nothing where `lane` isn't installed."
        ),
        false => format!(
            "a line in {rel} (Husky): worktrees other tools make become lanes (already set up)"
        ),
    };
    Ok(Change {
        path,
        changed,
        check_in: true,
        note: Some(note),
        ..Change::default()
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

/// A path in the repo from a `/`-separated one, spelled the system's way.
fn in_repo(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .filter(|c| !c.is_empty())
        .fold(root.to_path_buf(), |p, c| p.join(c))
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
