//! GitHub Copilot, set up once per machine. The Copilot CLI and VS Code's agent mode both read
//! hooks from `~/.copilot/hooks/*.json`, so Lanes keeps a file of its own there, `lanes.json`: a
//! session in a worktree Copilot made makes it a lane, and a shell command that copies the repo is
//! turned down, exiting 2 so the model sees why. VS Code's user settings get
//! `chat.useAgentsMdFile`, so agent mode reads the note `lane init` writes in `AGENTS.md` (the
//! Copilot CLI always does).
//!
//! From the docs, read on 2026-10-18:
//! - https://docs.github.com/en/copilot/reference/hooks-reference (Copilot CLI): user hooks are
//!   `*.json` files in `~/.copilot/hooks`, or `$COPILOT_HOME/hooks`; a file is
//!   `{"version": 1, "hooks": {"<event>": [{"type": "command", "bash", "powershell",
//!   "timeoutSec"}]}}`. preToolUse gets `cwd`, `toolName` (`bash` or `powershell`) and `toolArgs`;
//!   `{"permissionDecision": "deny", "permissionDecisionReason"}` shows the reason to the agent;
//!   exit 2 denies, and so does any other failure of a preToolUse hook (a timeout doesn't).
//!   sessionStart gets `cwd` and takes `additionalContext`.
//! - https://code.visualstudio.com/docs/copilot/customization/hooks and
//!   https://code.visualstudio.com/docs/agents/reference/hooks-reference (VS Code, updated
//!   2026-10-14): `~/.copilot/hooks/*.json` is read too, and Copilot's format is mapped onto
//!   VS Code's, whose payloads have `cwd`, `tool_name` and `tool_input`. Exit 2 gives stderr to
//!   the model as a blocking error. SessionStart takes `hookSpecificOutput.additionalContext`.
//!   `chat.useHooks` (on by default) runs hooks.
//! - https://code.visualstudio.com/docs/copilot/customization/custom-instructions:
//!   `chat.useAgentsMdFile` turns `AGENTS.md` on or off.
//! - https://docs.github.com/en/copilot/concepts/agents/about-agent-skills: personal skills are
//!   read from `~/.copilot/skills` or `~/.agents/skills`.

use super::jsonc::Doc;
use super::record::{self, Set};
use super::{Change, io_error, read_or_empty, write_atomic};
use crate::error::Result;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// Every Copilot hook Lanes adds runs this.
pub(crate) const PREFIX: &str = "lane hook copilot";

/// Lanes' own file in Copilot's hooks folder.
pub const HOOKS_FILE: &str = "lanes.json";

/// VS Code's setting for reading `AGENTS.md`.
const AGENTS_MD: &str = "chat.useAgentsMdFile";

/// Lanes' hooks file. A preToolUse hook that fails denies every tool in the Copilot CLI, so
/// where `lane` isn't installed any more, the bash one lets everything through instead.
pub fn hooks_file() -> String {
    let session = format!("{PREFIX} session-start");
    let tool = format!("{PREFIX} pre-tool-use");
    let file = json!({
        "version": 1,
        "hooks": {
            "sessionStart": [{
                "type": "command",
                "bash": session,
                "powershell": session,
                "timeoutSec": 300,
            }],
            "preToolUse": [{
                "type": "command",
                "bash": format!("command -v lane >/dev/null 2>&1 || exit 0; {tool}"),
                "powershell": tool,
                "timeoutSec": 10,
            }],
        },
    });
    serde_json::to_string_pretty(&file).expect("hooks serialize") + "\n"
}

/// Whether Lanes' hooks file in `dir` (Copilot's user folder) is in place and current.
pub fn is_set_up(dir: &Path) -> bool {
    fs::read_to_string(dir.join("hooks").join(HOOKS_FILE)).is_ok_and(|t| t == hooks_file())
}

/// Set up (`on`) or take back (`!on`) Copilot: Lanes' hooks file in `<dir>/hooks`,
/// `chat.useAgentsMdFile` in each of `vscode` (VS Code's user settings files), and the skill in
/// `skills`. What Lanes set in VS Code's settings is noted in `record`.
pub(crate) fn setup(
    dir: &Path,
    vscode: &[PathBuf],
    skills: &Path,
    record: &Path,
    on: bool,
) -> Result<Vec<Change>> {
    let mut changes = vec![hooks(&dir.join("hooks").join(HOOKS_FILE), on)?];
    for file in vscode {
        let mut off_already = false;
        let changed = record::edit(
            file,
            record,
            on,
            |doc| {
                off_already = doc.get(&[AGENTS_MD]) == Some(&json!(false));
                add_agents_md(doc)
            },
            |_| Ok(()),
        )?;
        changes.push(Change {
            path: file.clone(),
            changed,
            note: (on && off_already).then(|| {
                format!(
                    "{AGENTS_MD} is false in {}, so Copilot in VS Code won't read the note on lanes in AGENTS.md. Lanes left it as you set it.",
                    file.display()
                )
            }),
            ..Change::default()
        });
    }
    changes.push(super::user::skill(&skills.join("lanes/SKILL.md"), on)?);
    Ok(changes)
}

/// Write Lanes' hooks file, or remove it. A file of that name that isn't Lanes' is left alone.
fn hooks(path: &Path, on: bool) -> Result<Change> {
    let existing = read_or_empty(path)?;
    if !existing.is_empty() && !existing.contains(PREFIX) {
        return Ok(Change {
            path: path.to_path_buf(),
            note: Some(format!(
                "{} isn't Lanes' file, so Lanes left it alone. Move it, then run `lane setup` again.",
                path.display()
            )),
            ..Change::default()
        });
    }
    let changed = if on {
        let changed = existing != hooks_file();
        if changed {
            write_atomic(path, &hooks_file())?;
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

/// `chat.useAgentsMdFile: true`, unless it's set already. One that's false is yours, and stays.
fn add_agents_md(doc: &mut Doc) -> std::result::Result<Vec<Set>, String> {
    if doc.get(&[AGENTS_MD]).is_some() {
        return Ok(vec![]);
    }
    doc.insert(&[], AGENTS_MD, &json!(true))?;
    Ok(vec![Set {
        key: vec![AGENTS_MD.into()],
        was: None,
        set: json!(true),
        made_parent: false,
    }])
}

/// Whether VS Code has the Copilot extension in `extensions` (`~/.vscode/extensions`).
pub(crate) fn in_vscode(extensions: &Path) -> bool {
    fs::read_dir(extensions).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("github.copilot")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lane-copilot-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// VS Code's settings the way people keep them: four spaces, comments, a trailing comma.
    const VSCODE: &str = "{\n    // Looks\n    \"editor.fontSize\": 14,\n    \"workbench.colorTheme\": \"Default Dark Modern\", // dark\n}\n";

    #[test]
    fn the_hooks_file_is_copilots_documented_format() {
        let v: Value = serde_json::from_str(&hooks_file()).unwrap();
        assert_eq!(v["version"], 1);
        let tool = &v["hooks"]["preToolUse"][0];
        assert_eq!(tool["type"], "command");
        assert_eq!(tool["powershell"], "lane hook copilot pre-tool-use");
        assert!(
            tool["bash"]
                .as_str()
                .unwrap()
                .starts_with("command -v lane >/dev/null 2>&1 || exit 0;"),
            "without lane, the Copilot CLI would deny every tool"
        );
        assert_eq!(tool["timeoutSec"], 10);
        assert_eq!(
            v["hooks"]["sessionStart"][0]["bash"],
            "lane hook copilot session-start"
        );
    }

    #[test]
    fn gets_its_hooks_file_and_agents_md_and_off_gives_every_file_back_byte_for_byte() {
        let dir = temp("on-off");
        let (copilot, skills, record) =
            (dir.join(".copilot"), dir.join("skills"), dir.join("r.json"));
        let code = dir.join("Code/User/settings.json");
        let insiders = dir.join("Code - Insiders/User/settings.json");
        fs::create_dir_all(code.parent().unwrap()).unwrap();
        fs::create_dir_all(insiders.parent().unwrap()).unwrap();
        fs::write(&code, VSCODE).unwrap();
        // Switched off on purpose: Lanes leaves that alone, and says so.
        let off_by_you = "{\"chat.useAgentsMdFile\": false}";
        fs::write(&insiders, off_by_you).unwrap();
        let vscode = [code.clone(), insiders.clone()];

        let on = setup(&copilot, &vscode, &skills, &record, true).unwrap();
        assert!(
            on[0].changed && on[1].changed && !on[2].changed && on[3].changed,
            "{on:?}"
        );
        assert!(
            on[2]
                .note
                .as_deref()
                .unwrap()
                .contains("chat.useAgentsMdFile is false")
        );
        assert!(is_set_up(&copilot));
        let text = fs::read_to_string(&code).unwrap();
        assert!(
            text.contains("// Looks") && text.contains("\"chat.useAgentsMdFile\": true"),
            "{text}"
        );
        assert_eq!(fs::read_to_string(&insiders).unwrap(), off_by_you);
        assert!(skills.join("lanes/SKILL.md").is_file());
        assert!(
            setup(&copilot, &vscode, &skills, &record, true)
                .unwrap()
                .iter()
                .all(|c| !c.changed),
            "a second run changes nothing"
        );

        let off = setup(&copilot, &vscode, &skills, &record, false).unwrap();
        assert!(
            off[0].changed && off[1].changed && !off[2].changed,
            "{off:?}"
        );
        assert!(
            !copilot.join("hooks").exists(),
            "the hooks folder Lanes made goes too"
        );
        assert_eq!(fs::read_to_string(&code).unwrap(), VSCODE);
        assert_eq!(fs::read_to_string(&insiders).unwrap(), off_by_you);
        assert!(!skills.join("lanes").exists());
        assert!(!record.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn someone_elses_hooks_and_settings_are_left_alone() {
        let dir = temp("theirs");
        let (copilot, skills, record) =
            (dir.join(".copilot"), dir.join("skills"), dir.join("r.json"));
        let theirs = copilot.join("hooks").join(HOOKS_FILE);
        fs::create_dir_all(theirs.parent().unwrap()).unwrap();
        fs::write(&theirs, "{\"version\": 1, \"hooks\": {}}").unwrap();
        fs::write(copilot.join("hooks/audit.json"), "{}").unwrap();
        // AGENTS.md on already: nothing to add, so nothing to take back.
        let code = dir.join("settings.json");
        let on_by_you = "{ \"chat.useAgentsMdFile\": true }";
        fs::write(&code, on_by_you).unwrap();

        let on = setup(
            &copilot,
            std::slice::from_ref(&code),
            &skills,
            &record,
            true,
        )
        .unwrap();
        assert!(!on[0].changed && on[0].note.as_deref().unwrap().contains("isn't Lanes'"));
        assert!(!is_set_up(&copilot));
        setup(
            &copilot,
            std::slice::from_ref(&code),
            &skills,
            &record,
            false,
        )
        .unwrap();
        assert!(theirs.is_file() && copilot.join("hooks/audit.json").is_file());
        assert_eq!(fs::read_to_string(&code).unwrap(), on_by_you);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn copilot_in_vscode_is_found_by_its_extension() {
        let dir = temp("extensions");
        assert!(!in_vscode(&dir));
        fs::create_dir_all(dir.join("github.copilot-chat-0.40.0")).unwrap();
        assert!(in_vscode(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
