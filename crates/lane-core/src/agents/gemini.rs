//! Gemini CLI, set up once per machine in its user settings, `~/.gemini/settings.json`: a
//! session in a worktree Gemini CLI made (`gemini -w`) makes it a lane, a shell command that
//! copies the repo is turned down with the reason shown to the model, and `AGENTS.md` joins the
//! context files it reads, so it reads the note `lane init` writes.
//!
//! From Gemini CLI's docs, read on 2026-10-18:
//! - https://geminicli.com/docs/hooks/ and https://geminicli.com/docs/hooks/reference/: each
//!   event in `hooks` is a list of `{matcher, hooks: [{name, type: "command", command, timeout}]}`,
//!   with timeouts in milliseconds. A tool event's matcher is a regular expression on the tool's
//!   name; a missing one matches everything. A hook reads JSON on stdin (`cwd`, and for BeforeTool
//!   `tool_name` and `tool_input`) and answers with one JSON object on stdout: BeforeTool's
//!   `{"decision": "deny", "reason"}` sends the reason to the model as the tool's error, and
//!   SessionStart's `hookSpecificOutput.additionalContext` goes into the session. User hooks run
//!   without being trusted first.
//! - https://geminicli.com/docs/tools/shell/: the shell tool is `run_shell_command`, with
//!   `command` and `dir_path`. Windows runs it in PowerShell.
//! - https://geminicli.com/docs/reference/configuration/: `context.fileName` is a name or a list
//!   (`GEMINI.md` when it isn't set); `GEMINI_CLI_HOME` moves the user folder; hooks are on unless
//!   `hooksConfig.enabled` is false.
//! - https://geminicli.com/docs/cli/git-worktrees/: `gemini -w <name>` works in
//!   `.gemini/worktrees/<name>`.
//! - https://geminicli.com/docs/cli/skills/: `~/.agents/skills` is read, like `~/.gemini/skills`.
//!
//! Gemini CLI's settings allow comments, so they're changed as text (see `jsonc`).

use super::jsonc::Doc;
use super::record::{self, Set};
use super::{Change, is_ours_group};
use crate::error::Result;
use serde_json::{Value, json};
use std::path::Path;

/// Every Gemini CLI hook Lanes adds starts with this.
pub(crate) const PREFIX: &str = "lane hook gemini";

/// The context file Gemini CLI reads when `context.fileName` isn't set.
const DEFAULT_CONTEXT: &str = "GEMINI.md";

/// The hooks Lanes adds to Gemini CLI's settings. SessionStart has no matcher, so it runs on
/// startup, resume and `/clear` alike: after `/clear` the agent is told again which lane it's in.
pub fn hooks() -> Value {
    json!({
        "SessionStart": [{
            "hooks": [{
                "name": "lanes-session-start",
                "type": "command",
                "command": format!("{PREFIX} session-start"),
                "timeout": 300_000,
                "description": "Lanes: a worktree Gemini CLI made becomes a lane, and the agent learns its ports",
            }],
        }],
        "BeforeTool": [{
            "matcher": "run_shell_command",
            "hooks": [{
                "name": "lanes-before-tool",
                "type": "command",
                "command": format!("{PREFIX} before-tool"),
                "timeout": 10_000,
                "description": "Lanes: turns down copies of the repo, pointing to `lane new`",
            }],
        }],
    })
}

/// Whether Gemini CLI settings have all of Lanes' hooks and read `AGENTS.md`.
pub fn is_set_up(settings: &Value) -> bool {
    super::grouped_present(settings, hooks(), PREFIX) && reads_agents_md(settings)
}

fn reads_agents_md(settings: &Value) -> bool {
    match &settings["context"]["fileName"] {
        Value::String(name) => name == "AGENTS.md",
        Value::Array(names) => names.iter().any(|n| n == "AGENTS.md"),
        _ => false,
    }
}

/// Set up (`on`) or take back (`!on`) Gemini CLI: its hooks and `context.fileName` in
/// `<dir>/settings.json`, and the skill in `skills`. What Lanes set is noted in `record`.
pub(crate) fn setup(dir: &Path, skills: &Path, record: &Path, on: bool) -> Result<Vec<Change>> {
    let file = dir.join("settings.json");
    let changed = record::edit(&file, record, on, add, remove)?;
    let settings = super::read_or_empty(&file)
        .ok()
        .and_then(|t| Doc::parse(&t).ok())
        .map(|d| d.value().clone())
        .unwrap_or(Value::Null);
    let note = (on && settings["hooksConfig"]["enabled"] == false).then(|| {
        format!(
            "Gemini CLI's hooks are switched off (hooksConfig.enabled is false in {}), so its sessions don't get lanes until you switch them back on.",
            file.display()
        )
    });
    let skill = super::user::skill(&skills.join("lanes").join("SKILL.md"), on)?;
    Ok(vec![
        Change {
            path: file,
            changed,
            note,
            ..Change::default()
        },
        skill,
    ])
}

/// Add Lanes' hooks, and `AGENTS.md` to the context files. Returns the value Lanes set in
/// `context.fileName`, if it did.
fn add(doc: &mut Doc) -> std::result::Result<Vec<Set>, String> {
    let ours = hooks();
    match doc.get(&["hooks"]) {
        None => doc.insert(&[], "hooks", &ours)?,
        Some(Value::Object(_)) => {
            for (event, groups) in ours.as_object().expect("hooks are an object") {
                match doc.get(&["hooks", event]) {
                    None => doc.insert(&["hooks"], event, groups)?,
                    Some(Value::Array(list)) if list.iter().any(|g| is_ours_group(g, PREFIX)) => {}
                    Some(Value::Array(_)) => {
                        for group in groups.as_array().into_iter().flatten() {
                            doc.push(&["hooks", event], group)?;
                        }
                    }
                    Some(_) => return Err(format!("`hooks.{event}` isn't a list")),
                }
            }
        }
        Some(_) => return Err("`hooks` isn't an object".into()),
    }

    let both = json!([DEFAULT_CONTEXT, "AGENTS.md"]);
    let set = |was: Option<String>, set: Value, made_parent: bool| Set {
        key: vec!["context".into(), "fileName".into()],
        was,
        set,
        made_parent,
    };
    let path = ["context", "fileName"];
    let context = doc.get(&["context"]).cloned();
    Ok(match (context, doc.get(&path).cloned()) {
        (None, _) => {
            doc.insert(&[], "context", &json!({ "fileName": both }))?;
            vec![set(None, both, true)]
        }
        (Some(Value::Object(_)), None) => {
            doc.insert(&["context"], "fileName", &both)?;
            vec![set(None, both, false)]
        }
        (Some(Value::Object(_)), Some(Value::String(name))) if name == "AGENTS.md" => vec![],
        (Some(Value::Object(_)), Some(Value::String(name))) => {
            let was = doc.raw(&path).expect("it's there").to_string();
            doc.replace(&path, &format!("[{was}, \"AGENTS.md\"]"))?;
            vec![set(Some(was), json!([name, "AGENTS.md"]), false)]
        }
        (Some(Value::Object(_)), Some(Value::Array(names))) => {
            if names.iter().any(|n| n == "AGENTS.md") {
                vec![]
            } else {
                let was = doc.raw(&path).expect("it's there").to_string();
                doc.push(&path, &json!("AGENTS.md"))?;
                let now = doc.get(&path).expect("it's there").clone();
                vec![set(Some(was), now, false)]
            }
        }
        (Some(Value::Object(_)), Some(_)) => {
            return Err("`context.fileName` isn't a file name or a list of them".into());
        }
        (Some(_), _) => return Err("`context` isn't an object".into()),
    })
}

/// Take Lanes' hook groups back out. An event, or `hooks`, that only Lanes' hooks were in goes too.
fn remove(doc: &mut Doc) -> std::result::Result<(), String> {
    let events: Vec<String> = match doc.get(&["hooks"]) {
        Some(Value::Object(hooks)) => hooks.keys().cloned().collect(),
        _ => return Ok(()),
    };
    let mut removed = 0;
    for event in &events {
        let path = ["hooks", event.as_str()];
        let n = doc.remove_items(&path, |g| is_ours_group(g, PREFIX))?;
        if n > 0 && doc.get(&path) == Some(&json!([])) {
            doc.remove(&path)?;
        }
        removed += n;
    }
    if removed > 0 && doc.get(&["hooks"]) == Some(&json!({})) {
        doc.remove(&["hooks"])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lane-gemini-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn settings(dir: &Path) -> Value {
        let text = fs::read_to_string(dir.join("settings.json")).unwrap();
        Doc::parse(&text).unwrap().value().clone()
    }

    /// Settings the way people keep them: comments, their own hooks and context file.
    const MINE: &str = r#"{
  // Mine.
  "ui": { "theme": "GitHub" },
  "context": {
    "fileName": "CONTEXT.md"
  },
  "hooks": {
    "BeforeTool": [
      {
        "matcher": "write_file|replace",
        "hooks": [{ "name": "lint", "type": "command", "command": "./lint.sh" }]
      }
    ]
  }
}
"#;

    #[test]
    fn gets_its_hooks_and_agents_md_and_off_gives_the_file_back_byte_for_byte() {
        let dir = temp("mine");
        let (skills, record) = (dir.join("skills"), dir.join("record.json"));
        fs::write(dir.join("settings.json"), MINE).unwrap();

        let on = setup(&dir, &skills, &record, true).unwrap();
        assert!(on.iter().all(|c| c.changed), "{on:?}");
        let v = settings(&dir);
        assert!(is_set_up(&v), "{v}");
        assert_eq!(v["context"]["fileName"], json!(["CONTEXT.md", "AGENTS.md"]));
        assert_eq!(
            v["hooks"]["BeforeTool"][0]["hooks"][0]["command"],
            "./lint.sh"
        );
        assert_eq!(v["hooks"]["BeforeTool"][1]["matcher"], "run_shell_command");
        assert_eq!(
            v["hooks"]["BeforeTool"][1]["hooks"][0]["command"],
            "lane hook gemini before-tool"
        );
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 300_000,
            "milliseconds"
        );
        assert!(
            fs::read_to_string(dir.join("settings.json"))
                .unwrap()
                .contains("// Mine.")
        );
        assert!(skills.join("lanes").join("SKILL.md").is_file());
        assert!(
            setup(&dir, &skills, &record, true)
                .unwrap()
                .iter()
                .all(|c| !c.changed),
            "a second run changes nothing"
        );

        let off = setup(&dir, &skills, &record, false).unwrap();
        assert!(off.iter().all(|c| c.changed), "{off:?}");
        assert_eq!(fs::read_to_string(dir.join("settings.json")).unwrap(), MINE);
        assert!(!skills.join("lanes").exists());
        assert!(!record.exists(), "nothing left to take back");
        assert!(
            setup(&dir, &skills, &record, false)
                .unwrap()
                .iter()
                .all(|c| !c.changed)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_shape_of_context_file_name_comes_back_as_it_was() {
        for (n, text) in [
            "{\n    \"general\": { \"vimMode\": true }\n}\n",
            "{ \"context\": { \"includeDirectories\": [\"docs\"] } }",
            "{\r\n  \"context\": {\r\n    \"fileName\": [\"GEMINI.md\", \"TEAM.md\"]\r\n  }\r\n}\r\n",
            "{}",
        ]
        .into_iter()
        .enumerate()
        {
            let dir = temp(&format!("shape-{n}"));
            let (skills, record) = (dir.join("skills"), dir.join("record.json"));
            fs::write(dir.join("settings.json"), text).unwrap();
            setup(&dir, &skills, &record, true).unwrap();
            let v = settings(&dir);
            assert!(is_set_up(&v), "{v}");
            let names = v["context"]["fileName"].as_array().unwrap();
            assert!(names.contains(&json!("GEMINI.md")), "GEMINI.md still read: {v}");
            setup(&dir, &skills, &record, false).unwrap();
            assert_eq!(fs::read_to_string(dir.join("settings.json")).unwrap(), text);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn agents_md_already_read_is_left_in_and_a_changed_value_is_yours() {
        // AGENTS.md was there before: off leaves it.
        let dir = temp("already");
        let (skills, record) = (dir.join("skills"), dir.join("record.json"));
        let text = "{\"context\": {\"fileName\": [\"GEMINI.md\", \"AGENTS.md\"]}}\n";
        fs::write(dir.join("settings.json"), text).unwrap();
        setup(&dir, &skills, &record, true).unwrap();
        setup(&dir, &skills, &record, false).unwrap();
        assert_eq!(fs::read_to_string(dir.join("settings.json")).unwrap(), text);

        // Lanes set it, then you changed it: off leaves your value.
        fs::write(dir.join("settings.json"), "{}\n").unwrap();
        setup(&dir, &skills, &record, true).unwrap();
        let path = dir.join("settings.json");
        let changed = fs::read_to_string(&path)
            .unwrap()
            .replace("\"AGENTS.md\"", "\"AGENTS.md\", \"MORE.md\"");
        fs::write(&path, changed).unwrap();
        setup(&dir, &skills, &record, false).unwrap();
        let v = settings(&dir);
        assert_eq!(
            v["context"]["fileName"],
            json!(["GEMINI.md", "AGENTS.md", "MORE.md"])
        );
        assert!(v.get("hooks").is_none(), "the hooks still go: {v}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_file_lanes_made_goes_again_and_hooks_switched_off_are_noted() {
        let dir = temp("made");
        let (skills, record) = (dir.join("skills"), dir.join("record.json"));
        setup(&dir, &skills, &record, true).unwrap();
        assert!(is_set_up(&settings(&dir)));
        setup(&dir, &skills, &record, false).unwrap();
        assert!(!dir.join("settings.json").exists());

        fs::write(
            dir.join("settings.json"),
            "{\"hooksConfig\": {\"enabled\": false}}",
        )
        .unwrap();
        let on = setup(&dir, &skills, &record, true).unwrap();
        assert!(
            on[0]
                .note
                .as_deref()
                .unwrap()
                .contains("hooksConfig.enabled")
        );

        fs::write(dir.join("settings.json"), "{\"hooks\": [] /* no */}").unwrap();
        let e = setup(&dir, &skills, &record, true).unwrap_err();
        assert!(e.to_string().contains("isn't valid"), "{e}");
        let _ = fs::remove_dir_all(&dir);
    }
}
