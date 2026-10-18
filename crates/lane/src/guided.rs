//! Guided setup for a person at a terminal: Lanes does everything it can by itself, asks Y/n
//! only where the choice is theirs, and ends with what's left to do. `--yes` takes every
//! default without asking. Agents, scripts and CI are never asked anything: they get the plain
//! commands, which change nothing outside the repo.

use lane_core::agents::{self, Change, GitHook, user};
use lane_core::config::Config;
use lane_core::engine::{self, Repo};
use lane_core::error::{Error, ErrorKind, Result};
use lane_core::machine::{self, Status};
use lane_core::next::Step;
use lane_core::state::Store;
use serde::Serialize;
use std::io::{BufRead, Write};
use std::path::Path;

/// Something that answers a yes/no question.
pub trait Ask {
    fn ask(&mut self, question: &str, default: bool) -> bool;
}

/// A person at a terminal: `? question [Y/n] `, and Enter takes the default.
pub struct Terminal;

impl Ask for Terminal {
    fn ask(&mut self, question: &str, default: bool) -> bool {
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        let stdin = std::io::stdin();
        for _ in 0..3 {
            print!("? {question} {hint} ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                println!();
                return default;
            }
            // Typed answers end their own line; piped ones aren't echoed, so end it here.
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                println!("{}", line.trim());
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "" => return default,
                "y" | "yes" => return true,
                "n" | "no" => return false,
                _ => println!(
                    "  Type y or n, or press Enter for {}.",
                    if default { "yes" } else { "no" }
                ),
            }
        }
        default
    }
}

/// `--yes`: every default, nothing asked.
pub struct Defaults;

impl Ask for Defaults {
    fn ask(&mut self, _: &str, default: bool) -> bool {
        default
    }
}

/// One thing the guided setup did, or chose not to.
#[derive(Debug, Clone, Serialize)]
pub struct Done {
    /// `lane_toml`, `agents`, `git_hook`, `agents_md`, `claude_repo`, `dependencies`.
    pub what: &'static str,
    /// True when it was done now; false when it was declined or already in place.
    pub done: bool,
    /// What happened, in a few words.
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
}

fn done(what: &'static str, did: bool, summary: impl Into<String>) -> Done {
    Done {
        what,
        done: did,
        summary: summary.into(),
        changes: Vec::new(),
    }
}

/// The guided `lane init`: this repo, and this machine's agents, ready for lanes.
pub fn init(
    repo: &Repo,
    text: &str,
    toml: &Path,
    force: bool,
    ask: &mut dyn Ask,
    say: bool,
) -> Result<(Vec<Done>, Vec<Step>)> {
    let mut out = Vec::new();
    let note = |d: &Done| {
        if say {
            let mark = if d.done { "✓" } else { "·" };
            println!("  {mark} {}", d.summary);
        }
    };

    // 1. lane.toml: what Lanes found, written down for teammates.
    let exists = toml.exists();
    let d = if exists && !force {
        done(
            "lane_toml",
            false,
            "lane.toml is already there, so it was left as is",
        )
    } else if ask.ask(
        "Write lane.toml with what Lanes found, so teammates get the same lanes?",
        true,
    ) {
        write_toml(toml, text)?;
        done("lane_toml", true, format!("wrote {}", toml.display()))
    } else {
        done(
            "lane_toml",
            false,
            "no lane.toml: Lanes finds your stack each time instead",
        )
    };
    note(&d);
    out.push(d);

    // 2. This machine's agents: their worktrees become lanes in every repo.
    let config = Config::load().unwrap_or_default();
    let found_agents = user::found();
    let unset: Vec<user::Agent> = found_agents
        .iter()
        .copied()
        .filter(|a| !user::is_set_up(*a))
        .collect();
    let names = |xs: &[user::Agent]| and_list(&xs.iter().map(|a| a.name).collect::<Vec<_>>());
    let d = if !config.agents {
        done(
            "agents",
            false,
            "agents in lanes is switched off for this machine",
        )
    } else if found_agents.is_empty() {
        done(
            "agents",
            false,
            "no Claude Code, Codex, Cursor, Gemini CLI or GitHub Copilot found on this machine",
        )
    } else if unset.is_empty() {
        done(
            "agents",
            false,
            format!("{} already put their work in lanes", names(&found_agents)),
        )
    } else if ask.ask(
        &format!(
            "Put {} in lanes on this machine? Their worktrees become lanes in every repo.",
            names(&unset)
        ),
        true,
    ) {
        let results = user::setup(true)?;
        let mut d = done(
            "agents",
            true,
            format!("{} put their work in lanes now", names(&found_agents)),
        );
        d.changes = results
            .into_iter()
            .flat_map(|(_, c)| c)
            .filter(|c| c.changed)
            .collect();
        d
    } else {
        done(
            "agents",
            false,
            format!("{} left as they are", names(&unset)),
        )
    };
    let agents_declined = !d.done && config.agents && !unset.is_empty();
    let codex_now = d.done
        && d.changes
            .iter()
            .any(|c| c.path.components().any(|p| p.as_os_str() == ".codex"));
    note(&d);
    out.push(d);

    // 3. The Git hook: worktrees other tools make with Git become lanes.
    let d = match agents::git_hook(repo)? {
        GitHook::Ready(_) => done("git_hook", false, "the Git hook is in place"),
        state => {
            let q = match state {
                GitHook::Old(_) => {
                    "Update the Git hook from an older Lanes, so new worktrees get their dependencies too?"
                }
                _ => "Add the Git hook, so worktrees other tools make with Git become lanes too?",
            };
            if ask.ask(q, true) {
                let c = agents::setup_git_hook(repo)?;
                let summary = match (&c.note, c.changed) {
                    (Some(n), _) => n.clone(),
                    (None, _) => "Git hook: worktrees other tools make become lanes".into(),
                };
                let mut d = done("git_hook", c.changed, summary);
                if say {
                    for line in c.snippet.iter().flat_map(|s| s.lines()) {
                        println!("      {line}");
                    }
                }
                d.changes.push(c);
                d
            } else {
                done(
                    "git_hook",
                    false,
                    "no Git hook: other tools' worktrees stay plain worktrees",
                )
            }
        }
    };
    note(&d);
    out.push(d);

    // 4. AGENTS.md: a short note every agent that reads it follows.
    let d = if agents::has_agents_note(repo) {
        done("agents_md", false, "AGENTS.md has the note on lanes")
    } else if ask.ask(
        "Add a short note on working in lanes to AGENTS.md, for every agent that reads it?",
        true,
    ) {
        let changes = agents::setup_agents_md(repo)?;
        let mut d = done("agents_md", true, "AGENTS.md: a note on working in lanes");
        d.changes = changes;
        d
    } else {
        done("agents_md", false, "AGENTS.md left as it is")
    };
    note(&d);
    out.push(d);

    // 5. Claude Code's hooks for this repo alone, when they aren't set up for the machine.
    if agents::uses_claude(repo) && !user::is_set_up(user::CLAUDE) {
        let d = if ask.ask(
            "Set up Claude Code's hooks for this repo only? They stay on this machine, out of Git.",
            true,
        ) {
            let c = agents::setup_claude(repo)?;
            let mut d = done(
                "claude_repo",
                c.changed,
                "Claude Code: hooks for this repo, kept out of Git",
            );
            d.changes.push(c);
            d
        } else {
            done(
                "claude_repo",
                false,
                "Claude Code left as it is in this repo",
            )
        };
        note(&d);
        out.push(d);
    }

    // 6. Dependencies, installed once in a clean copy now, so the first lane is quick.
    // The steps lanes will use: lane.toml's, or what Lanes found when there's none.
    let install = &repo.project.setup.install;
    if !install.is_empty() {
        let mut store = Store::open()?;
        let d = if engine::base_is_ready(&store, repo)? {
            done(
                "dependencies",
                false,
                "dependencies are installed in the clean copy",
            )
        } else if ask.ask(
            &format!(
                "Install dependencies now ({}), so the first lane is ready in seconds?",
                install.join(" && ")
            ),
            true,
        ) {
            if say {
                println!("  Installing in a clean copy of the repo (it can take a minute)…");
            }
            let started = std::time::Instant::now();
            let base = engine::prepare(&mut store, repo)?;
            done(
                "dependencies",
                true,
                format!(
                    "dependencies installed in a clean copy of {} ({:.1} s)",
                    base.branch,
                    started.elapsed().as_secs_f64()
                ),
            )
        } else {
            done(
                "dependencies",
                false,
                "dependencies install with the first lane instead",
            )
        };
        note(&d);
        out.push(d);
    }

    // What's left for the person to do.
    let mut steps = Vec::new();
    if codex_now {
        steps.push(Step {
            run: "/hooks".into(),
            why: "in Codex, once: trust Lanes' two hooks, so Codex sessions get lanes".into(),
        });
    }
    let mut to_add = Vec::new();
    for d in out.iter().filter(|d| d.done) {
        if d.what == "lane_toml" {
            to_add.push(lane_core::project::FILE_NAME.to_string());
        }
        for c in d.changes.iter().filter(|c| c.changed && c.check_in) {
            let rel = c.path.strip_prefix(&repo.root).unwrap_or(&c.path);
            to_add.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    if !to_add.is_empty() {
        steps.push(Step {
            run: format!("git add {}", to_add.join(" ")),
            why: "check it in, so teammates get the same lanes".into(),
        });
    }
    if user::is_set_up(user::CLAUDE) || agents::uses_claude(repo) {
        steps.push(Step {
            run: "claude -w fix-auth".into(),
            why: "start Claude Code in a lane of its own".into(),
        });
    } else {
        steps.push(Step {
            run: "lane new fix-auth".into(),
            why: "make a lane".into(),
        });
    }
    if machine::run_check("copies", &repo.root).status == Status::Warn {
        steps.push(Step {
            run: "lane doctor".into(),
            why:
                "this disk copies lanes instead of cloning them; it shows how to get instant lanes"
                    .into(),
        });
    }
    if !config.agents {
        steps.push(Step {
            run: "lane setup --yes --agents on".into(),
            why: "put your agents in lanes, when you want to".into(),
        });
    } else if agents_declined {
        steps.push(Step {
            run: "lane setup".into(),
            why: format!("put {} in lanes, when you want to", names(&unset)),
        });
    }
    Ok((out, steps))
}

/// What `lane doctor --fix` fixed.
#[derive(Debug, Clone, Serialize)]
pub struct Fixed {
    pub check: &'static str,
    pub summary: String,
}

/// Fix what `lane doctor` found that Lanes can fix by itself, asking first. Things only you can
/// do (install Git, move a repo to a Dev Drive, change PATH) keep their printed steps.
pub fn fix(dir: &Path, ask: &mut dyn Ask) -> Result<Vec<Fixed>> {
    let mut fixed = Vec::new();
    let config = Config::load().unwrap_or_default();

    // Agents that aren't in lanes yet.
    let unset: Vec<user::Agent> = user::found()
        .into_iter()
        .filter(|a| !user::is_set_up(*a))
        .collect();
    if config.agents && !unset.is_empty() {
        let names = and_list(&unset.iter().map(|a| a.name).collect::<Vec<_>>());
        if ask.ask(&format!("Put {names} in lanes on this machine?"), true) {
            user::setup(true)?;
            fixed.push(Fixed {
                check: "agent-hooks",
                summary: format!("{names} put their work in lanes"),
            });
        }
    }

    let Ok(repo) = Repo::open(dir) else {
        return Ok(fixed);
    };

    // The Git hook, missing or from an older Lanes.
    match agents::git_hook(&repo)? {
        GitHook::Ready(_) => {}
        state => {
            let q = match state {
                GitHook::Old(_) => "Update this repo's Git hook from an older Lanes?",
                _ => "Add this repo's Git hook, so worktrees other tools make become lanes?",
            };
            if ask.ask(q, true) {
                let c = agents::setup_git_hook(&repo)?;
                fixed.push(Fixed {
                    check: "git-hook",
                    summary: c
                        .note
                        .clone()
                        .unwrap_or_else(|| "the Git hook is in place".into()),
                });
            }
        }
    }

    // Lanes whose folders have gone, or that were left half-made.
    if let Some(mut store) = Store::open_existing()? {
        let lanes = store.lanes(Some(&repo.root))?;
        let gone: Vec<String> = lanes
            .iter()
            .filter(|l| !l.path.exists() || l.is_stale())
            .map(|l| l.name.clone())
            .collect();
        if !gone.is_empty() {
            let q = format!(
                "Remove {} whose folder{} gone or that {} left half-made ({}), freeing {} ports?",
                match gone.len() {
                    1 => "1 lane".to_string(),
                    n => format!("{n} lanes"),
                },
                if gone.len() == 1 { " has" } else { "s have" },
                if gone.len() == 1 { "was" } else { "were" },
                gone.join(", "),
                if gone.len() == 1 { "its" } else { "their" },
            );
            if ask.ask(&q, true) {
                for name in &gone {
                    engine::remove(&mut store, &repo, name, false, None)?;
                    lane_core::log::info("doctor", "removed a lane whose folder had gone")
                        .lane(name)
                        .repo(&repo.root)
                        .write();
                }
                fixed.push(Fixed {
                    check: "lanes",
                    summary: format!("removed {}", gone.join(", ")),
                });
            }
        }
    }
    Ok(fixed)
}

fn write_toml(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| {
            Error::new(
                ErrorKind::Io,
                format!("couldn't write {}: {e}", path.display()),
            )
            .hint("Check that you can write to the repo's folder.")
        })
}

/// "A", "A and B", "A, B and C".
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted answers, recording each question.
    struct Scripted(VecDeque<bool>, Vec<String>);
    impl Ask for Scripted {
        fn ask(&mut self, question: &str, default: bool) -> bool {
            self.1.push(question.to_string());
            self.0.pop_front().unwrap_or(default)
        }
    }

    #[test]
    fn defaults_take_the_default_and_lists_read_well() {
        assert!(Defaults.ask("anything?", true));
        assert!(!Defaults.ask("anything?", false));
        assert_eq!(and_list(&["Claude Code"]), "Claude Code");
        assert_eq!(
            and_list(&["Claude Code", "Codex", "Cursor"]),
            "Claude Code, Codex and Cursor"
        );
    }

    #[test]
    fn scripted_answers_record_the_questions() {
        let mut s = Scripted(VecDeque::from([false]), Vec::new());
        assert!(!s.ask("first?", true));
        assert!(s.ask("second?", true), "out of answers: the default");
        assert_eq!(s.1, ["first?", "second?"]);
    }
}
