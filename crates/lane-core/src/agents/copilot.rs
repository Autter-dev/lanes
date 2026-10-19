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
//!
//! How each runs a hook on Windows, read on 2026-10-18:
//! - The Copilot CLI runs the `powershell` command in PowerShell, which its Windows examples
//!   need "PowerShell 7.0 or later installed and in your PATH" for
//!   (https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-hooks). A
//!   preToolUse hook that fails denies the tool, "Denied by preToolUse hook (hook errored)",
//!   whatever the exit code, unless it timed out
//!   (https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-hooks-reference).
//!   Neither page says how PowerShell is started.
//! - VS Code's docs don't say which shell either. Its source does (microsoft/vscode,
//!   `src/vs/workbench/contrib/chat/common/promptSyntax/hookSchema.ts` and
//!   `extensions/copilot/src/platform/chat/node/hookExecutor.ts`): `powershell` becomes its
//!   `windows` command, which runs with `powershell.exe -ExecutionPolicy Bypass -NoProfile
//!   -NoLogo -Command` (Windows PowerShell 5.1) where ComSpec is cmd.exe, and in cmd.exe (Node's
//!   `shell: true`) where it isn't. A command that can't start, or exits with anything but 0 or
//!   2, is a warning, and the tool runs; exit 2 blocks it.
//! - `-Command` exits 1 when its last command fails, whatever that command's own exit code was,
//!   so a deny's exit 2 reaches Copilot only through `exit $LASTEXITCODE`.

use super::jsonc::Doc;
use super::record::{self, Set};
use super::{Change, io_error, read_or_empty, write_atomic};
use crate::error::Result;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Every Copilot hook Lanes adds runs this. The bash commands always run `lane` by name, so it's
/// also how a later run knows the file is Lanes'.
pub(crate) const PREFIX: &str = "lane hook copilot";

/// Lanes' own file in Copilot's hooks folder.
pub const HOOKS_FILE: &str = "lanes.json";

/// VS Code's setting for reading `AGENTS.md`.
const AGENTS_MD: &str = "chat.useAgentsMdFile";

/// Lanes' hooks file, for Windows (`windows`) or for macOS and Linux. A preToolUse hook that
/// fails denies every tool in the Copilot CLI, so where `lane` isn't installed any more, both
/// let everything through instead: the bash one when `lane` isn't on PATH, and on Windows, where
/// the PowerShell one runs `lane` (at `lane`, its absolute path, when there is one), when that
/// file has gone. macOS and Linux get the PowerShell commands Lanes 0.0.8 wrote.
pub fn hooks_file(windows: bool, lane: Option<&Path>) -> String {
    let command = |event: &str| match windows {
        true => windows_command(lane, event),
        false => format!("{PREFIX} {event}"),
    };
    let file = json!({
        "version": 1,
        "hooks": {
            "sessionStart": [{
                "type": "command",
                "bash": format!("{PREFIX} session-start"),
                "powershell": command("session-start"),
                "timeoutSec": 300,
            }],
            "preToolUse": [{
                "type": "command",
                "bash": format!("command -v lane >/dev/null 2>&1 || exit 0; {PREFIX} pre-tool-use"),
                "powershell": command("pre-tool-use"),
                "timeoutSec": 10,
            }],
        },
    });
    serde_json::to_string_pretty(&file).expect("hooks serialize") + "\n"
}

/// What the Windows hooks run for `event`: one line for cmd.exe, which stops at the newline, and
/// a second that only PowerShell runs. In both shells cmd's `if exist` runs `lane`, and does
/// nothing (exit 0, no output, so Copilot's own rules apply) when that file has gone; PowerShell
/// then passes on `lane`'s exit code, which `-Command` would turn into 1. With no `lane` path,
/// `lane.exe` is looked for on PATH, and missing it does nothing too.
fn windows_command(lane: Option<&Path>, event: &str) -> String {
    match lane {
        Some(lane) => {
            let lane = lane.display();
            format!(
                "cmd /d /c if exist \"{lane}\" \"{lane}\" hook copilot {event}\nexit $LASTEXITCODE"
            )
        }
        None => format!(
            "cmd /d /c \"where /q lane.exe || exit 0 & lane.exe hook copilot {event}\"\nexit $LASTEXITCODE"
        ),
    }
}

/// The `lane` the Windows hooks run, by its absolute path, so an editor whose PATH is from
/// before Lanes was installed still finds it: the `lane.exe` on PATH (winget's and Scoop's links
/// stay put across upgrades), else this program. None when its path can't go in the command as
/// it is; the hooks then look for `lane.exe` on PATH when they run. Under `LANE_AGENTS_HOME`
/// (tests) only this program counts.
pub fn lane_exe() -> Option<PathBuf> {
    let fake_home = std::env::var_os("LANE_AGENTS_HOME").is_some_and(|d| !d.is_empty());
    let on_path = (!fake_home)
        .then(|| super::find_on_path("lane.exe"))
        .flatten();
    let lane = on_path.or_else(|| {
        std::env::current_exe().ok().filter(|p| {
            p.file_stem()
                .is_some_and(|s| s.eq_ignore_ascii_case("lane"))
        })
    })?;
    let text = lane.to_str()?;
    // `\\?\C:\...` is the same file, written so cmd's `if exist` reads it.
    let text = match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest,
        _ => text,
    };
    (Path::new(text).is_absolute() && quotable(text)).then(|| PathBuf::from(text))
}

/// Whether `path` reads the same, in double quotes, to cmd.exe and to PowerShell, which passes it
/// on to cmd: nothing either one expands (`%`, `!`, `$`, a backtick), no wildcards and no quotes
/// (PowerShell takes “ ” „ for quotes too). PowerShell only quotes it again for cmd when it has a
/// space, so without one it can't have cmd's separators either.
fn quotable(path: &str) -> bool {
    let spaced = path.contains(' ');
    !path.is_empty()
        && path.chars().all(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '"' | '%' | '!' | '$' | '`' | '?' | '*' | '\u{201C}' | '\u{201D}' | '\u{201E}'
                )
                && (spaced
                    || !matches!(c, '&' | '|' | '<' | '>' | '^' | '(' | ')' | ',' | ';' | '='))
        })
}

/// What Lanes' hooks file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hooks {
    /// In place and current.
    Ready,
    /// Not there, or not Lanes' file.
    Missing,
    /// From Lanes 0.0.8: on Windows its commands run `lane` by name and fail without it, so the
    /// Copilot CLI turns down every tool once `lane` can't be found.
    Old,
    /// It runs a `lane` that isn't there any more, so it does nothing.
    Gone(PathBuf),
    /// Lanes' file, changed since it was written.
    Changed,
}

/// What Lanes' hooks file in `dir` (Copilot's user folder) holds, here.
pub fn hooks_state(dir: &Path) -> Hooks {
    let text = fs::read_to_string(dir.join("hooks").join(HOOKS_FILE)).unwrap_or_default();
    state(&text, cfg!(windows))
}

/// Whether Lanes' hooks file in `dir` (Copilot's user folder) is in place and current.
pub fn is_set_up(dir: &Path) -> bool {
    hooks_state(dir) == Hooks::Ready
}

/// What a hooks file holding `text` is, on Windows (`windows`) or macOS and Linux. On Windows,
/// one that runs a `lane` other than this one counts as set up while that file is there, so two
/// copies of Lanes don't keep rewriting it.
fn state(text: &str, windows: bool) -> Hooks {
    if !text.contains(PREFIX) {
        return Hooks::Missing;
    }
    if text == hooks_file(windows, None) {
        return Hooks::Ready;
    }
    if !windows {
        return Hooks::Changed;
    }
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let tool = v["hooks"]["preToolUse"][0]["powershell"]
        .as_str()
        .unwrap_or_default();
    if tool == format!("{PREFIX} pre-tool-use") {
        return Hooks::Old;
    }
    let lane = tool
        .strip_prefix("cmd /d /c if exist \"")
        .and_then(|rest| rest.split_once('"'))
        .map(|(lane, _)| PathBuf::from(lane));
    match lane {
        Some(lane) if text == hooks_file(true, Some(&lane)) => match lane.is_file() {
            true => Hooks::Ready,
            false => Hooks::Gone(lane),
        },
        _ => Hooks::Changed,
    }
}

/// Set up (`on`) or take back (`!on`) Copilot: Lanes' hooks file in `<dir>/hooks`, whose Windows
/// commands run `lane` (see [`lane_exe`]), `chat.useAgentsMdFile` in each of `vscode` (VS Code's
/// user settings files), and the skill in `skills`. What Lanes set in VS Code's settings is noted
/// in `record`.
pub(crate) fn setup(
    dir: &Path,
    vscode: &[PathBuf],
    skills: &Path,
    record: &Path,
    lane: Option<&Path>,
    on: bool,
) -> Result<Vec<Change>> {
    let file = hooks_file(cfg!(windows), lane);
    let mut changes = vec![hooks(&dir.join("hooks").join(HOOKS_FILE), &file, on)?];
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
    changes.push(super::user::skill(
        &skills.join("lanes").join("SKILL.md"),
        on,
    )?);
    Ok(changes)
}

/// Write Lanes' hooks file (`file`), or remove it. One Lanes wrote before, an older one too, is
/// written again; a file of that name that isn't Lanes' is left alone.
fn hooks(path: &Path, file: &str, on: bool) -> Result<Change> {
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
        let changed = existing != file;
        if changed {
            write_atomic(path, file)?;
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

    /// The hooks file Lanes 0.0.8 wrote, on every system.
    const V0_0_8: &str = r#"{
  "version": 1,
  "hooks": {
    "sessionStart": [
      {
        "type": "command",
        "bash": "lane hook copilot session-start",
        "powershell": "lane hook copilot session-start",
        "timeoutSec": 300
      }
    ],
    "preToolUse": [
      {
        "type": "command",
        "bash": "command -v lane >/dev/null 2>&1 || exit 0; lane hook copilot pre-tool-use",
        "powershell": "lane hook copilot pre-tool-use",
        "timeoutSec": 10
      }
    ]
  }
}
"#;

    /// A stand-in for `lane.exe`, in a folder named like the ones people have.
    fn fake_lane(dir: &Path) -> PathBuf {
        let lane = dir
            .join("Program Files (x86)")
            .join("Lanes")
            .join("lane.exe");
        fs::create_dir_all(lane.parent().unwrap()).unwrap();
        fs::write(&lane, "").unwrap();
        lane
    }

    #[test]
    fn the_hooks_file_is_copilots_documented_format() {
        for windows in [false, true] {
            let v: Value =
                serde_json::from_str(&hooks_file(windows, Some(Path::new(r"C:\lane.exe"))))
                    .unwrap();
            assert_eq!(v["version"], 1);
            let tool = &v["hooks"]["preToolUse"][0];
            assert_eq!(tool["type"], "command");
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
            assert_eq!(v["hooks"]["sessionStart"][0]["timeoutSec"], 300);
        }
    }

    #[test]
    fn each_system_gets_its_exact_commands() {
        let commands = |file: &str| {
            let v: Value = serde_json::from_str(file).unwrap();
            ["sessionStart", "preToolUse"].map(|event| {
                let hook = &v["hooks"][event][0];
                (
                    hook["bash"].as_str().unwrap().to_string(),
                    hook["powershell"].as_str().unwrap().to_string(),
                )
            })
        };
        let bash = [
            "lane hook copilot session-start",
            "command -v lane >/dev/null 2>&1 || exit 0; lane hook copilot pre-tool-use",
        ];

        // macOS and Linux: what 0.0.8 wrote, byte for byte, wherever `lane` is.
        assert_eq!(hooks_file(false, None), V0_0_8);
        assert_eq!(
            hooks_file(false, Some(Path::new("/usr/local/bin/lane"))),
            V0_0_8
        );

        // Windows: `lane` by its absolute path, through cmd.exe, and PowerShell passes its exit
        // code on.
        let lane = Path::new(r"C:\Users\Ana María\AppData\Local\Programs\Lanes (x86)\lane.exe");
        let [session, tool] = commands(&hooks_file(true, Some(lane)));
        assert_eq!((session.0.as_str(), tool.0.as_str()), (bash[0], bash[1]));
        assert_eq!(
            session.1,
            "cmd /d /c if exist \"C:\\Users\\Ana María\\AppData\\Local\\Programs\\Lanes (x86)\\lane.exe\" \"C:\\Users\\Ana María\\AppData\\Local\\Programs\\Lanes (x86)\\lane.exe\" hook copilot session-start\nexit $LASTEXITCODE"
        );
        assert_eq!(
            tool.1,
            "cmd /d /c if exist \"C:\\Users\\Ana María\\AppData\\Local\\Programs\\Lanes (x86)\\lane.exe\" \"C:\\Users\\Ana María\\AppData\\Local\\Programs\\Lanes (x86)\\lane.exe\" hook copilot pre-tool-use\nexit $LASTEXITCODE"
        );

        // Windows, with no path it can write: `lane.exe` from PATH, when there is one.
        let [session, tool] = commands(&hooks_file(true, None));
        assert_eq!(
            session.1,
            "cmd /d /c \"where /q lane.exe || exit 0 & lane.exe hook copilot session-start\"\nexit $LASTEXITCODE"
        );
        assert_eq!(
            tool.1,
            "cmd /d /c \"where /q lane.exe || exit 0 & lane.exe hook copilot pre-tool-use\"\nexit $LASTEXITCODE"
        );
    }

    #[test]
    fn only_paths_both_shells_read_the_same_go_in_the_command() {
        for ok in [
            r"C:\Users\ghosh\.cargo\bin\lane.exe",
            r"C:\Program Files (x86)\Lanes\lane.exe",
            r"C:\Users\O'Brien\scoop\shims\lane.exe",
            r"C:\Users\Tom & Jerry\bin\lane.exe",
            r"C:\Users\Zoë\AppData\Roaming\npm\node_modules\@autter\lanes-win32-x64\bin\lane.exe",
            r"\\server\tools\lane.exe",
        ] {
            assert!(quotable(ok), "{ok}");
        }
        for bad in [
            r"C:\Users\$env\lane.exe",
            r"C:\Users\50%\lane.exe",
            r"C:\Users\wow!\lane.exe",
            r"C:\Users\a`b\lane.exe",
            r"C:\Users\R&D\lane.exe",
            r"C:\Users\a(b)\lane.exe",
            r"C:\Users\a;b\lane.exe",
            "C:\\Users\\“quoted”\\lane.exe",
            r"\\?\C:\lane.exe",
            "C:\\new\nline\\lane.exe",
            "",
        ] {
            assert!(!quotable(bad), "{bad}");
        }
    }

    #[test]
    fn a_file_from_0_0_8_or_one_whose_lane_has_gone_isnt_set_up_on_windows() {
        let dir = temp("states");
        let lane = fake_lane(&dir);
        let gone = dir.join("gone").join("lane.exe");

        assert_eq!(state(V0_0_8, false), Hooks::Ready);
        assert_eq!(state(V0_0_8, true), Hooks::Old);
        assert_eq!(state(&hooks_file(true, Some(&lane)), true), Hooks::Ready);
        assert_eq!(state(&hooks_file(true, None), true), Hooks::Ready);
        assert_eq!(
            state(&hooks_file(true, Some(&gone)), true),
            Hooks::Gone(gone.clone())
        );
        let edited = hooks_file(true, Some(&lane)).replace("300", "60");
        assert_eq!(state(&edited, true), Hooks::Changed);
        assert_eq!(state(&V0_0_8.replace("300", "60"), false), Hooks::Changed);
        assert_eq!(state("", true), Hooks::Missing);
        assert_eq!(
            state("{\"version\": 1, \"hooks\": {}}", false),
            Hooks::Missing
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_rewrites_the_hooks_from_0_0_8_on_windows_and_off_still_removes_them() {
        let dir = temp("upgrade");
        let (copilot, skills, record) =
            (dir.join(".copilot"), dir.join("skills"), dir.join("r.json"));
        let lane = fake_lane(&dir);
        let file = copilot.join("hooks").join(HOOKS_FILE);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, V0_0_8).unwrap();
        assert_eq!(is_set_up(&copilot), !cfg!(windows));

        let on = setup(&copilot, &[], &skills, &record, Some(&lane), true).unwrap();
        assert_eq!(on[0].changed, cfg!(windows), "{on:?}");
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            hooks_file(cfg!(windows), Some(&lane))
        );
        assert!(is_set_up(&copilot));

        // The `lane` it runs goes away (moved, or uninstalled): it does nothing, and isn't set up.
        if cfg!(windows) {
            fs::remove_file(&lane).unwrap();
            assert_eq!(hooks_state(&copilot), Hooks::Gone(lane.clone()));
        }

        setup(&copilot, &[], &skills, &record, Some(&lane), false).unwrap();
        assert!(!copilot.join("hooks").exists());
        let _ = fs::remove_dir_all(&dir);
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
        let lane = fake_lane(&dir);

        let on = setup(&copilot, &vscode, &skills, &record, Some(&lane), true).unwrap();
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
        assert!(skills.join("lanes").join("SKILL.md").is_file());
        assert!(
            setup(&copilot, &vscode, &skills, &record, Some(&lane), true)
                .unwrap()
                .iter()
                .all(|c| !c.changed),
            "a second run changes nothing"
        );

        let off = setup(&copilot, &vscode, &skills, &record, Some(&lane), false).unwrap();
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
            None,
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
            None,
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
