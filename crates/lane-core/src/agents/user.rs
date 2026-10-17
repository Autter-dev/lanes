//! Putting agents in lanes once per machine, in each agent's own user-level settings, so every
//! repo gets lanes without anyone running `lane init` in it. `lane setup` (and the first-run
//! welcome) does this for each agent it finds; switching it off takes back exactly what was
//! added and leaves everything else in those files as it was.

use super::{
    Change, claude_hooks_present, codex_hooks_present, cursor_hooks_present, io_error,
    merge_claude_hooks, merge_codex_hooks, merge_cursor_hooks, read_or_empty, remove_claude_hooks,
    remove_codex_hooks, remove_cursor_hooks, write_atomic,
};
use crate::error::{Error, ErrorKind, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// The skill every agent that reads skills gets: when to work in a lane, and how.
pub const SKILL: &str = include_str!("../../assets/SKILL.md");

/// The first line of the skill's body, which is how a later run knows the file is Lanes'.
const SKILL_MARKER: &str = "name: lanes\n";

/// One agent Lanes can set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent {
    pub id: &'static str,
    pub name: &'static str,
}

pub const CLAUDE: Agent = Agent {
    id: "claude",
    name: "Claude Code",
};

pub const CODEX: Agent = Agent {
    id: "codex",
    name: "Codex",
};

pub const CURSOR: Agent = Agent {
    id: "cursor",
    name: "Cursor",
};

/// The agents Lanes knows how to set up, in the order it shows them.
pub const AGENTS: [Agent; 3] = [CLAUDE, CODEX, CURSOR];

/// Where agents keep their user settings: your home folder. `LANE_AGENTS_HOME` points it
/// somewhere else, so tests never touch a real agent's settings.
fn home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("LANE_AGENTS_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    etcetera::home_dir().map_err(|_| {
        Error::new(ErrorKind::Config, "couldn't find your home folder")
            .hint("Set HOME (USERPROFILE on Windows), then run `lane setup` again.")
    })
}

/// Claude Code's user folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_dir() -> Result<PathBuf> {
    if std::env::var_os("LANE_AGENTS_HOME").is_some_and(|d| !d.is_empty()) {
        return Ok(home()?.join(".claude"));
    }
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => Ok(PathBuf::from(d)),
        None => Ok(home()?.join(".claude")),
    }
}

/// The agents installed here: their user folder exists, or the program is on PATH.
pub fn found() -> Vec<Agent> {
    AGENTS
        .into_iter()
        .filter(|a| match a.id {
            "claude" => claude_dir().is_ok_and(|d| d.is_dir()) || super::on_path("claude"),
            "codex" => codex_dir().is_ok_and(|d| d.is_dir()) || super::on_path("codex"),
            "cursor" => {
                cursor_dir().is_ok_and(|d| d.is_dir())
                    || super::on_path("cursor")
                    || super::on_path("cursor-agent")
            }
            _ => false,
        })
        .collect()
}

/// Set up (`on`) or take back (`!on`) every agent found here. Each step is safe to repeat.
pub fn setup(on: bool) -> Result<Vec<(Agent, Vec<Change>)>> {
    let mut out = Vec::new();
    for agent in found() {
        let changes = match agent.id {
            "claude" => claude(&claude_dir()?, on)?,
            "codex" => codex(&codex_dir()?, &shared_skills()?, on)?,
            "cursor" => cursor(&cursor_dir()?, &shared_skills()?, on)?,
            _ => Vec::new(),
        };
        out.push((agent, changes));
    }
    Ok(out)
}

/// Whether an agent is set up here already (all of its pieces in place).
pub fn is_set_up(agent: Agent) -> bool {
    match agent.id {
        "claude" => claude_dir().is_ok_and(|d| {
            claude_hooks_present(&read_json(&d.join("settings.json")).unwrap_or(Value::Null))
                && is_ours(&d.join("skills/lanes/SKILL.md"))
        }),
        "codex" => {
            codex_dir().is_ok_and(|d| {
                codex_hooks_present(&read_json(&d.join("hooks.json")).unwrap_or(Value::Null))
            }) && shared_skills().is_ok_and(|d| is_ours(&d.join("lanes/SKILL.md")))
        }
        "cursor" => {
            cursor_dir().is_ok_and(|d| {
                cursor_hooks_present(&read_json(&d.join("hooks.json")).unwrap_or(Value::Null))
            }) && shared_skills().is_ok_and(|d| is_ours(&d.join("lanes/SKILL.md")))
        }
        _ => false,
    }
}

/// Claude Code: the hooks in `~/.claude/settings.json`, and the skill in
/// `~/.claude/skills/lanes/SKILL.md`.
pub(crate) fn claude(dir: &Path, on: bool) -> Result<Vec<Change>> {
    let settings = dir.join("settings.json");
    let mut value = read_json(&settings)?;
    let changed = if on {
        merge_claude_hooks(&mut value)?
    } else {
        remove_claude_hooks(&mut value)
    };
    if changed {
        let text = serde_json::to_string_pretty(&value).expect("settings serialize") + "\n";
        write_atomic(&settings, &text)?;
    }
    let skill = skill(&dir.join("skills/lanes/SKILL.md"), on)?;
    Ok(vec![
        Change {
            path: settings,
            changed,
            ..Change::default()
        },
        skill,
    ])
}

/// Cursor's user folder: `CURSOR_CONFIG_DIR`, else `~/.cursor`.
pub fn cursor_dir() -> Result<PathBuf> {
    if std::env::var_os("LANE_AGENTS_HOME").is_some_and(|d| !d.is_empty()) {
        return Ok(home()?.join(".cursor"));
    }
    match std::env::var_os("CURSOR_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => Ok(PathBuf::from(d)),
        None => Ok(home()?.join(".cursor")),
    }
}

/// The skills folder every agent following the Agent Skills standard reads: `~/.agents/skills`.
/// Codex and Cursor both read it, so they share one copy of the skill.
pub fn shared_skills() -> Result<PathBuf> {
    Ok(home()?.join(".agents").join("skills"))
}

/// Codex's user folder: `CODEX_HOME`, else `~/.codex`.
pub fn codex_dir() -> Result<PathBuf> {
    if std::env::var_os("LANE_AGENTS_HOME").is_some_and(|d| !d.is_empty()) {
        return Ok(home()?.join(".codex"));
    }
    match std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        Some(d) => Ok(PathBuf::from(d)),
        None => Ok(home()?.join(".codex")),
    }
}

/// Codex: its hooks in `~/.codex/hooks.json` (a session in a Codex worktree makes it a lane;
/// copies of the repo are turned down), and the skill in `~/.agents/skills/lanes`. Codex runs
/// a hook only once you've trusted it, so the change says to open `/hooks` in Codex.
pub(crate) fn codex(dir: &Path, skills: &Path, on: bool) -> Result<Vec<Change>> {
    let file = dir.join("hooks.json");
    let mut value = read_json(&file)?;
    let changed = if on {
        merge_codex_hooks(&mut value)?
    } else {
        remove_codex_hooks(&mut value)
    };
    if changed {
        let text = serde_json::to_string_pretty(&value).expect("hooks serialize") + "\n";
        write_atomic(&file, &text)?;
    }
    let skill = skill(&skills.join("lanes/SKILL.md"), on)?;
    Ok(vec![
        Change {
            path: file,
            changed,
            note: (on && changed).then(|| {
                "Codex runs new hooks only once you trust them: open /hooks in Codex and trust Lanes' two hooks.".to_string()
            }),
            ..Change::default()
        },
        skill,
    ])
}

/// Cursor: its hooks in `~/.cursor/hooks.json` (a session in a Cursor worktree makes it a
/// lane; copies of the repo are turned down), and the skill in `~/.agents/skills/lanes`.
pub(crate) fn cursor(dir: &Path, skills: &Path, on: bool) -> Result<Vec<Change>> {
    let file = dir.join("hooks.json");
    let mut value = read_json(&file)?;
    let changed = if on {
        merge_cursor_hooks(&mut value)?
    } else {
        remove_cursor_hooks(&mut value)
    };
    if changed {
        let text = serde_json::to_string_pretty(&value).expect("hooks serialize") + "\n";
        write_atomic(&file, &text)?;
    }
    let skill = skill(&skills.join("lanes/SKILL.md"), on)?;
    Ok(vec![
        Change {
            path: file,
            changed,
            ..Change::default()
        },
        skill,
    ])
}

/// Write the skill at `path` (or remove it, if it's Lanes'). A file someone else wrote there is
/// left alone, with a note.
pub(crate) fn skill(path: &Path, on: bool) -> Result<Change> {
    let existing = read_or_empty(path)?;
    let theirs = !existing.is_empty() && !existing.contains(SKILL_MARKER);
    if theirs {
        return Ok(Change {
            path: path.to_path_buf(),
            note: Some(format!(
                "{} is someone else's skill, so Lanes left it alone.",
                path.display()
            )),
            ..Change::default()
        });
    }
    let changed = if on {
        let changed = existing != SKILL;
        if changed {
            write_atomic(path, SKILL)?;
        }
        changed
    } else if path.exists() {
        fs::remove_file(path).map_err(|e| io_error("remove", path, e))?;
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir(dir); // only if it's empty now
        }
        true
    } else {
        false
    };
    Ok(Change {
        path: path.to_path_buf(),
        changed,
        ..Change::default()
    })
}

fn is_ours(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|t| t.contains(SKILL_MARKER))
}

/// A JSON settings file, or `{}` when there isn't one.
pub(crate) fn read_json(path: &Path) -> Result<Value> {
    let text = read_or_empty(path)?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| {
        Error::new(
            ErrorKind::Config,
            format!("{} isn't valid JSON ({e})", path.display()),
        )
        .hint("Fix the file, then run `lane setup` again to put your agents in lanes.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lane-user-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_gets_hooks_and_the_skill_and_off_takes_back_only_those() {
        let dir = temp("claude");
        fs::write(
            dir.join("settings.json"),
            r#"{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}}"#,
        )
        .unwrap();

        let on = claude(&dir, true).unwrap();
        assert!(on.iter().all(|c| c.changed), "{on:?}");
        let settings = read_json(&dir.join("settings.json")).unwrap();
        assert!(super::super::claude_hooks_present(&settings));
        assert_eq!(settings["model"], "opus");
        assert_eq!(
            fs::read_to_string(dir.join("skills/lanes/SKILL.md")).unwrap(),
            SKILL
        );
        assert!(
            claude(&dir, true).unwrap().iter().all(|c| !c.changed),
            "a second run changes nothing"
        );

        let off = claude(&dir, false).unwrap();
        assert!(off.iter().all(|c| c.changed));
        let settings = read_json(&dir.join("settings.json")).unwrap();
        assert_eq!(settings["model"], "opus");
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"], "say done",
            "the user's own hook stays"
        );
        assert!(settings["hooks"].get("WorktreeCreate").is_none());
        assert!(!dir.join("skills/lanes").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn someone_elses_skill_is_left_alone() {
        let dir = temp("theirs");
        let path = dir.join("skills/lanes/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "---
name: my-lanes
---
Mine.
",
        )
        .unwrap();
        let change = skill(&path, true).unwrap();
        assert!(!change.changed);
        assert!(change.note.unwrap().contains("someone else's"));
        assert!(!skill(&path, false).unwrap().changed);
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursor_gets_its_hooks_and_the_skill_and_off_takes_back_only_those() {
        let dir = temp("cursor");
        fs::write(
            dir.join("hooks.json"),
            r#"{"version": 1, "hooks": {"afterFileEdit": [{"command": "./format.sh"}]}}"#,
        )
        .unwrap();
        let skills = dir.join("agents-skills");
        assert!(
            cursor(&dir, &skills, true)
                .unwrap()
                .iter()
                .all(|c| c.changed)
        );
        let file = read_json(&dir.join("hooks.json")).unwrap();
        assert!(super::super::cursor_hooks_present(&file));
        assert_eq!(
            file["hooks"]["sessionStart"][0]["command"],
            "lane hook cursor session-start"
        );
        assert_eq!(file["hooks"]["afterFileEdit"][0]["command"], "./format.sh");
        assert!(skills.join("lanes/SKILL.md").is_file());
        assert!(
            cursor(&dir, &skills, true)
                .unwrap()
                .iter()
                .all(|c| !c.changed)
        );

        assert!(
            cursor(&dir, &skills, false)
                .unwrap()
                .iter()
                .all(|c| c.changed)
        );
        let file = read_json(&dir.join("hooks.json")).unwrap();
        assert!(file["hooks"].get("sessionStart").is_none());
        assert_eq!(file["hooks"]["afterFileEdit"][0]["command"], "./format.sh");
        assert_eq!(file["version"], 1);
        assert!(!skills.join("lanes").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_gets_its_hooks_and_the_shared_skill_and_off_takes_back_only_those() {
        let dir = temp("codex");
        let skills = dir.join("agents-skills");
        fs::write(
            dir.join("hooks.json"),
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify"}]}]}}"#,
        )
        .unwrap();
        let on = codex(&dir, &skills, true).unwrap();
        assert!(on.iter().all(|c| c.changed));
        assert!(
            on[0].note.as_deref().unwrap().contains("/hooks"),
            "trusting the hooks is the user's step"
        );
        let file = read_json(&dir.join("hooks.json")).unwrap();
        assert!(super::super::codex_hooks_present(&file));
        assert_eq!(
            file["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "lane hook codex session-start"
        );
        assert_eq!(file["hooks"]["PreToolUse"][0]["matcher"], "^Bash$");
        assert_eq!(
            fs::read_to_string(skills.join("lanes/SKILL.md")).unwrap(),
            SKILL
        );
        assert!(
            codex(&dir, &skills, true)
                .unwrap()
                .iter()
                .all(|c| !c.changed)
        );

        assert!(
            codex(&dir, &skills, false)
                .unwrap()
                .iter()
                .all(|c| c.changed)
        );
        let file = read_json(&dir.join("hooks.json")).unwrap();
        assert!(file["hooks"].get("SessionStart").is_none());
        assert_eq!(file["hooks"]["Stop"][0]["hooks"][0]["command"], "notify");
        assert!(!skills.join("lanes").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plugin_carries_the_same_skill() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/claude-code/skills/lanes/SKILL.md");
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            SKILL,
            "copy assets/SKILL.md to {}",
            file.display()
        );
    }
}
