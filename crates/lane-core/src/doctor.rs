//! `lane doctor`: everything `lane setup` checks about this machine, and then what can quietly
//! stop lanes working: `lane` missing from PATH (agents' hooks call it by name), agents that
//! aren't set up, this repo's Git hook, where Lanes keeps its state and settings (and, on
//! Windows, what's left in the old AppData folders), lanes whose folders have gone, the state
//! store, the daemon, and errors in the log. Each check says what to do next; nothing is changed.

use crate::agents::copilot::{self, Hooks};
use crate::agents::{self, GitHook, user};
use crate::config::Config;
use crate::engine::{Repo, norm};
use crate::log;
use crate::machine::{self, Check, Status};
use crate::moved;
use crate::state::Store;
use std::path::Path;

/// Every check, in the order to read them.
pub fn run(dir: &Path) -> Vec<Check> {
    let mut checks = machine::run_all(dir);
    checks.push(lane_on_path());
    checks.push(agent_hooks());
    let repo = Repo::open(dir).ok();
    if let Some(repo) = &repo {
        checks.push(git_hook(repo));
    }
    checks.push(folders());
    checks.extend(old_folders());
    checks.push(state(repo.as_ref()));
    checks.push(daemon());
    checks.push(recent_errors());
    checks
}

/// Whether anything here needs fixing before Lanes can work.
pub fn healthy(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.status != Status::Fail)
}

/// Agents' hooks run `lane` by name, so it has to be on PATH, not only where you ran it from.
fn lane_on_path() -> Check {
    if agents::on_path("lane") {
        return Check::new(
            "lane-path",
            "On PATH",
            Status::Ok,
            "agents' hooks can run `lane`",
        );
    }
    let here = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let hint = match here {
        Some(dir) => format!(
            "Add {} to PATH (then open a new terminal), so agents' hooks can run `lane`.",
            dir.display()
        ),
        None => "Add the folder `lane` is in to PATH, so agents' hooks can run it.".to_string(),
    };
    Check::new(
        "lane-path",
        "On PATH",
        Status::Fail,
        "`lane` isn't on PATH, so agents' hooks can't run it",
    )
    .hint(hint)
}

/// The agents found here, and whether each one puts its work in lanes.
fn agent_hooks() -> Check {
    let on = Config::load().map(|c| c.agents).unwrap_or(true);
    let found = user::found();
    if !on {
        return Check::new(
            "agent-hooks",
            "Agents in lanes",
            Status::Info,
            "switched off on this machine",
        )
        .hint("`lane setup --yes --agents on` puts your agents back in lanes.");
    }
    if found.is_empty() {
        return Check::new(
            "agent-hooks",
            "Agents in lanes",
            Status::Info,
            "no Claude Code, Codex, Cursor, Gemini CLI or GitHub Copilot found",
        )
        .hint("Run `lane setup` after you install one, and its worktrees become lanes.");
    }
    let (ready, not): (Vec<_>, Vec<_>) = found.iter().partition(|a| user::is_set_up(**a));
    let names = |xs: &[&user::Agent]| xs.iter().map(|a| a.name).collect::<Vec<_>>().join(", ");
    if not.is_empty() {
        // Every agent found here is set up.
        return Check::new(
            "agent-hooks",
            "Agents in lanes",
            Status::Ok,
            format!("{} put their work in lanes", names(&ready)),
        );
    }
    // Copilot's hooks from Lanes 0.0.8, or ones whose `lane` has gone, get their own words.
    let copilot = not
        .iter()
        .any(|a| **a == user::COPILOT)
        .then(|| user::copilot_dir().ok().map(|d| copilot::hooks_state(&d)))
        .flatten();
    let issue = match &copilot {
        Some(Hooks::Old) => Some(
            "GitHub Copilot has the hooks from Lanes 0.0.8, which on Windows make the Copilot CLI turn down every tool once `lane` can't be found".to_string(),
        ),
        Some(Hooks::Gone(lane)) => Some(format!(
            "GitHub Copilot's hooks run {}, which isn't there any more, so they do nothing",
            norm(lane).display()
        )),
        _ => None,
    };
    if let Some(issue) = issue {
        let rest: Vec<&user::Agent> = not
            .iter()
            .copied()
            .filter(|a| *a != &user::COPILOT)
            .collect();
        return match rest.is_empty() {
            true => Check::new("agent-hooks", "Agents in lanes", Status::Warn, issue)
                .hint("Run `lane setup --yes` to update Copilot's hooks, once for this machine."),
            false => Check::new(
                "agent-hooks",
                "Agents in lanes",
                Status::Warn,
                format!("{issue}; {} not set up", names(&rest)),
            )
            .hint(format!(
                "Run `lane setup --yes` to update Copilot's hooks and put {} in lanes, once for this machine.",
                names(&rest)
            )),
        };
    }
    let summary = match ready.is_empty() {
        true => format!("{} not set up", names(&not)),
        false => format!("{} not set up ({} is)", names(&not), names(&ready)),
    };
    Check::new("agent-hooks", "Agents in lanes", Status::Warn, summary)
        .hint("Run `lane setup` to put them in lanes, once for this machine.")
}

/// This repo's post-checkout hook: other tools' worktrees become lanes through it.
fn git_hook(repo: &Repo) -> Check {
    match agents::git_hook(repo) {
        Ok(GitHook::Ready(path)) => Check::new(
            "git-hook",
            "Git hook",
            Status::Ok,
            format!("other tools' worktrees become lanes ({})", norm(&path).display()),
        ),
        Ok(GitHook::Old(path)) => Check::new(
            "git-hook",
            "Git hook",
            Status::Warn,
            format!("an older copy of Lanes' hook ({})", norm(&path).display()),
        )
        .hint("Run `lane init` in this repo to update it, so new worktrees get their dependencies too."),
        Ok(GitHook::Missing) => Check::new(
            "git-hook",
            "Git hook",
            Status::Info,
            "not set up: worktrees tools make with Git won't become lanes",
        )
        .hint("Run `lane init` in this repo. Claude Code, Codex, Cursor, Gemini CLI and Copilot don't need it."),
        Err(e) => Check::new("git-hook", "Git hook", Status::Warn, e.to_string())
            .hint(e.hint.unwrap_or_default()),
    }
}

/// Where Lanes keeps its state and its settings.
fn folders() -> Check {
    let state = crate::state::state_dir()
        .map(|p| norm(&p).display().to_string())
        .unwrap_or_else(|e| e.to_string());
    let settings = crate::config::config_dir()
        .map(|p| norm(&p).display().to_string())
        .unwrap_or_else(|e| e.to_string());
    Check::new(
        "folders",
        "Folders",
        Status::Info,
        format!("state in {state}, settings in {settings}"),
    )
}

/// On Windows, Lanes before 0.1.0 kept its state and settings in AppData, and an app installed
/// from a package (like the Claude desktop app) had its own copy of them. Each comes across to
/// the new folders by itself (`lane doctor` looks in every package first); this says what's
/// left. Nothing on other systems, or once the old folders are gone.
fn old_folders() -> Option<Check> {
    let left = moved::leftovers();
    let list = |paths: &[std::path::PathBuf]| {
        paths
            .iter()
            .map(|p| norm(p).display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !left.waiting.is_empty() {
        return Some(
            Check::new(
                "old-folders",
                "Old folders",
                Status::Warn,
                format!(
                    "lanes or settings in {} haven't come across yet",
                    list(&left.waiting)
                ),
            )
            .hint("See why with `lane logs`, then run `lane doctor` again: Lanes tries each time. If an old store is damaged and you don't need its lanes, delete that folder."),
        );
    }
    if let Some(copy) = &left.hidden_by {
        return Some(
            Check::new(
                "old-folders",
                "Old folders",
                Status::Warn,
                format!(
                    "this `lane` runs inside the {} app package, where Windows shows the package's own copy of AppData, so lanes made outside that app can't come across from here",
                    moved::package_name(copy)
                ),
            )
            .hint("Run `lane doctor` once in your own terminal (not one this app started), and they come across."),
        );
    }
    if left.across.is_empty() {
        return None;
    }
    let quoted = left
        .across
        .iter()
        .map(|p| format!("'{}'", norm(p).display()))
        .collect::<Vec<_>>()
        .join(", ");
    Some(
        Check::new(
            "old-folders",
            "Old folders",
            Status::Info,
            format!(
                "everything in {} has come across, so Lanes doesn't use them any more",
                list(&left.across)
            ),
        )
        .hint(format!(
            "Delete them when you like, from your own terminal: Remove-Item -Recurse {quoted}"
        )),
    )
}

/// The state store, and this repo's lanes: any whose folder has gone (a tool cleaned up its own
/// worktrees, or someone deleted one), or that were left half-made.
fn state(repo: Option<&Repo>) -> Check {
    let store = match Store::open_existing() {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Check::new("lanes", "Lanes", Status::Info, "no lanes made yet");
        }
        Err(e) => {
            return Check::new("lanes", "Lanes", Status::Fail, e.to_string()).hint(
                e.hint
                    .unwrap_or_else(|| "Run `lane list` to see the error.".into()),
            );
        }
    };
    let lanes = match store.lanes(repo.map(|r| r.root.as_path())) {
        Ok(l) => l,
        Err(e) => {
            return Check::new("lanes", "Lanes", Status::Fail, e.to_string())
                .hint(e.hint.unwrap_or_default());
        }
    };
    let where_ = if repo.is_some() {
        "in this repo"
    } else {
        "on this machine"
    };
    let gone: Vec<&str> = lanes
        .iter()
        .filter(|l| !l.path.exists())
        .map(|l| l.name.as_str())
        .collect();
    let stale: Vec<&str> = lanes
        .iter()
        .filter(|l| l.is_stale())
        .map(|l| l.name.as_str())
        .collect();
    if !gone.is_empty() {
        return Check::new(
            "lanes",
            "Lanes",
            Status::Warn,
            format!(
                "{} with no folder any more: {}",
                count(gone.len()),
                gone.join(", ")
            ),
        )
        .hint(format!(
            "Remove them to free their ports: `lane rm {}`{}.",
            gone[0],
            if gone.len() > 1 {
                ", and the same for each"
            } else {
                ""
            }
        ));
    }
    if !stale.is_empty() {
        return Check::new(
            "lanes",
            "Lanes",
            Status::Warn,
            format!(
                "{} left half-made: {}",
                count(stale.len()),
                stale.join(", ")
            ),
        )
        .hint(format!(
            "Make it again with `lane new {}`, or remove it with `lane rm`.",
            stale[0]
        ));
    }
    match lanes.len() {
        0 => Check::new(
            "lanes",
            "Lanes",
            Status::Info,
            format!("no lanes {where_} yet"),
        ),
        n => Check::new(
            "lanes",
            "Lanes",
            Status::Ok,
            format!("{} {where_}, each with its folder", count(n)),
        ),
    }
}

fn count(n: usize) -> String {
    match n {
        1 => "1 lane".into(),
        n => format!("{n} lanes"),
    }
}

/// `laned` ships with `lane`. Nothing needs it running: the terminal app starts it, and every
/// command works without it. Asking a running one changes nothing.
fn daemon() -> Check {
    let running = crate::daemon::Endpoint::current()
        .ok()
        .and_then(|at| crate::daemon::Client::connect(&at).ok());
    if let Some(client) = running {
        return if client.version == crate::VERSION {
            Check::new(
                "daemon",
                "Daemon",
                Status::Ok,
                format!("laned {} is running (pid {})", client.version, client.pid),
            )
        } else {
            Check::new(
                "daemon",
                "Daemon",
                Status::Warn,
                format!(
                    "laned {} is running, from another Lanes version ({} here)",
                    client.version,
                    crate::VERSION
                ),
            )
            .hint("Run `lane daemon start` to replace it with this version's.")
        };
    }
    if crate::daemon::laned_path().is_some() {
        Check::new(
            "daemon",
            "Daemon",
            Status::Info,
            "laned is installed and not running; the terminal app starts it",
        )
    } else {
        Check::new(
            "daemon",
            "Daemon",
            Status::Warn,
            "laned isn't installed next to lane",
        )
        .hint("Install Lanes again (lanes.autter.dev/download); every way installs both programs.")
    }
}

/// Errors Lanes logged lately: a failed install, a hook that couldn't finish.
fn recent_errors() -> Check {
    let path = log::path()
        .map(|p| norm(&p).display().to_string())
        .unwrap_or_default();
    let errors = log::read(200, None)
        .map(|es| {
            es.into_iter()
                .filter(|e| e.level == log::Level::Error)
                .count()
        })
        .unwrap_or(0);
    match errors {
        0 => Check::new(
            "log",
            "Log",
            Status::Ok,
            format!("no errors lately ({path})"),
        ),
        n => Check::new(
            "log",
            "Log",
            Status::Warn,
            format!("{n} error{} lately", if n == 1 { "" } else { "s" }),
        )
        .hint("See what happened with `lane logs`."),
    }
}
