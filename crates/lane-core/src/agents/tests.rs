use super::*;
use crate::project::Project;
use crate::state::{PortBlock, Status};
use std::process::Command;

fn lane(name: &str, path: &str) -> Lane {
    Lane {
        id: "l_7".into(),
        repo: PathBuf::from("/work/app"),
        name: name.into(),
        path: PathBuf::from(path),
        base_commit: None,
        ports: PortBlock {
            start: 4100,
            count: 10,
        },
        agent: Some("claude".into()),
        outcome: None,
        status: Status::Ready,
        created_at: 0,
    }
}

/// A real Git repo in a temporary folder, removed when dropped.
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(name: &str) -> TempRepo {
        let dir = std::env::temp_dir().join(format!("lane-agents-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ok = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git init");
        TempRepo(dir)
    }
    fn repo(&self) -> Repo {
        Repo {
            root: self.0.clone(),
            project: Project::default(),
            lanes_dir: self.0.join("lanes"),
        }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn worktree_names_become_lane_names() {
    assert_eq!(lane_name("feature-auth", "s"), "feature-auth");
    assert_eq!(lane_name("Fix Auth_2", "s"), "fix-auth-2");
    assert_eq!(lane_name("bold-oak-a3f2", "s"), "bold-oak-a3f2");
    assert_eq!(lane_name("feat/login--page", "s"), "feat-login-page");
    assert_eq!(lane_name(&"x".repeat(60), "s").len(), 40);
    assert_eq!(
        lane_name(&format!("{}-tail", "a".repeat(39)), "s"),
        "a".repeat(39)
    );
    assert_eq!(lane_name("", "72C1AA05-72ca"), "claude-72c1aa05");
    assert_eq!(lane_name("___", ""), "claude");
    for raw in ["Ünïcode wörk", "--x--", "a b c"] {
        crate::state::check_name(&lane_name(raw, "s")).unwrap();
    }
}

#[test]
fn git_worktree_add_is_spotted_in_any_shape() {
    for cmd in [
        "git worktree add ../app-fix -b fix",
        "git -C ../app worktree add /tmp/x",
        "cd repo && git worktree add ../y",
        "git -c core.autocrlf=false worktree --quiet add ../z",
        "\"C:\\Program Files\\Git\\cmd\\git.exe\" worktree add ..\\w",
        "npm test; git worktree add ../q",
    ] {
        assert!(makes_worktree(cmd), "{cmd}");
    }
    for cmd in [
        "git worktree list",
        "git worktree remove ../x",
        "git status",
        "echo git worktree add",
        "grep -r 'worktree add' docs",
        "lane new fix --print-path",
    ] {
        assert!(!makes_worktree(cmd), "{cmd}");
    }
}

#[test]
fn env_lines_are_plain_assignments() {
    let text = env_lines(&lane("fix-auth", "C:\\Users\\sam\\app.lanes\\fix-auth"));
    assert!(text.contains("export PORT=4100\n"));
    assert!(text.contains("export VITE_PORT=4101\n"));
    assert!(text.contains("export LANE_URL=http://fix-auth.localhost:4100\n"));
    assert!(text.contains("export LANE_PATH=C:/Users/sam/app.lanes/fix-auth\n"));
    for line in text.lines() {
        assert!(!line.contains(['"', '\'', '$', ' ']) || line.starts_with("export "));
    }
    // A folder with a space can't be written plainly, so it's left out rather than quoted.
    let spaced = env_lines(&lane("x", "/Users/sam/My Code/x"));
    assert!(!spaced.contains("LANE_PATH"));
    assert!(spaced.contains("export PORT=4100\n"));
}

#[test]
fn a_session_in_a_lane_is_told_its_ports() {
    let mut project = Project::default();
    project.dev.command = Some("npm run dev -- --port $PORT".into());
    let repo = Repo {
        root: PathBuf::from("/work/app"),
        project,
        lanes_dir: PathBuf::from("/work/app.lanes"),
    };
    let text = context_in_lane(&repo, &lane("fix-auth", "/work/app.lanes/fix-auth"));
    assert!(text.contains("lane `fix-auth`"));
    assert!(text.contains("PORT=4100"));
    assert!(text.contains("http://fix-auth.localhost:4100"));
    assert!(text.contains("lane exec fix-auth -- npm run dev -- --port 4100"));
    assert!(text.contains("lane new <name> --print-path"));
}

#[test]
fn hooks_merge_into_settings_once_and_keep_the_rest() {
    let mut settings = json!({
        "permissions": { "allow": ["Bash(npm test)"] },
        "hooks": {
            "PreToolUse": [{ "matcher": "Write", "hooks": [{ "type": "command", "command": "fmt.sh" }] }]
        }
    });
    assert!(merge_claude_hooks(&mut settings).unwrap());
    assert_eq!(settings["permissions"]["allow"][0], "Bash(npm test)");
    let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2, "the user's hook stays, ours is added");
    assert_eq!(pre[0]["hooks"][0]["command"], "fmt.sh");
    assert_eq!(pre[1]["matcher"], "Bash|PowerShell");
    assert_eq!(
        settings["hooks"]["WorktreeCreate"][0]["hooks"][0]["command"],
        "lane hook claude worktree-create"
    );
    let once = settings.clone();
    assert!(
        !merge_claude_hooks(&mut settings).unwrap(),
        "a second run adds nothing"
    );
    assert_eq!(settings, once);
    // Keys keep their order, so a user's file isn't reshuffled.
    let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["permissions", "hooks"]);

    assert!(merge_claude_hooks(&mut json!([])).is_err());
}

#[test]
fn claude_setup_writes_local_settings_and_keeps_them_out_of_git() {
    let t = TempRepo::new("claude");
    let repo = t.repo();
    let first = setup_claude(&repo).unwrap();
    assert!(first.changed);
    let text = fs::read_to_string(repo.root.join(CLAUDE_SETTINGS)).unwrap();
    assert!(text.contains("lane hook claude session-start"));
    let ignored = Command::new("git")
        .args(["check-ignore", "-q", CLAUDE_SETTINGS])
        .current_dir(&repo.root)
        .status()
        .unwrap()
        .success();
    assert!(ignored, "settings.local.json is ignored");
    assert!(!setup_claude(&repo).unwrap().changed);
    let exclude = fs::read_to_string(repo.root.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("/.claude/settings.local.json").count(), 1);

    fs::write(repo.root.join(CLAUDE_SETTINGS), "{ not json").unwrap();
    let e = setup_claude(&repo).unwrap_err();
    assert!(e.to_string().contains("isn't valid JSON"), "{e}");
}

#[test]
fn agents_md_gets_the_note_once_and_claude_md_points_at_it() {
    let t = TempRepo::new("agentsmd");
    let repo = t.repo();
    fs::write(
        repo.root.join("AGENTS.md"),
        "# Working here\n\nRun the tests.",
    )
    .unwrap();
    fs::write(repo.root.join("CLAUDE.md"), "Be brief.\n").unwrap();
    let changes = setup_agents_md(&repo).unwrap();
    assert!(changes.iter().all(|c| c.changed));
    let agents = fs::read_to_string(repo.root.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("# Working here\n\nRun the tests.\n\n<!-- Added by `lane init`."));
    assert!(agents.contains("lane new <name> --print-path"));
    let claude = fs::read_to_string(repo.root.join("CLAUDE.md")).unwrap();
    assert_eq!(claude, "Be brief.\n\n@AGENTS.md\n");

    let again = setup_agents_md(&repo).unwrap();
    assert!(again.iter().all(|c| !c.changed));
    assert_eq!(
        fs::read_to_string(repo.root.join("AGENTS.md")).unwrap(),
        agents
    );

    // No AGENTS.md yet: it's made with just the note.
    let t2 = TempRepo::new("agentsmd-new");
    setup_agents_md(&t2.repo()).unwrap();
    assert_eq!(
        fs::read_to_string(t2.0.join("AGENTS.md")).unwrap(),
        AGENTS_NOTE
    );
}

#[test]
fn the_git_hook_is_added_once_and_runs_the_one_before_it() {
    let t = TempRepo::new("githook");
    let repo = t.repo();
    let hooks = repo.root.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("post-checkout"), "#!/bin/sh\necho theirs\n").unwrap();

    let change = setup_git_hook(&repo).unwrap();
    assert!(change.changed);
    assert_eq!(
        fs::read_to_string(hooks.join("post-checkout")).unwrap(),
        POST_CHECKOUT
    );
    assert_eq!(
        fs::read_to_string(hooks.join("post-checkout.before-lanes")).unwrap(),
        "#!/bin/sh\necho theirs\n",
        "the existing hook is kept, to run after ours"
    );
    assert!(!setup_git_hook(&repo).unwrap().changed);

    // core.hooksPath (Husky, lefthook): nothing is written, and the note says why.
    let t2 = TempRepo::new("githook-path");
    let ok = Command::new("git")
        .args(["config", "core.hooksPath", ".husky"])
        .current_dir(&t2.0)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let change = setup_git_hook(&t2.repo()).unwrap();
    assert!(!change.changed);
    assert!(change.note.unwrap().contains("core.hooksPath"));
}

#[test]
fn the_plugin_runs_the_same_hooks_as_lane_init() {
    let file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/claude-code/hooks/hooks.json");
    let plugin: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(plugin["hooks"], claude_hooks(), "update {}", file.display());
}
