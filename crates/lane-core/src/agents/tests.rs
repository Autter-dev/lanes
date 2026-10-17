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
    fn git(&self, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=Lanes Test",
                "-c",
                "user.email=test@lanes.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&self.0)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
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

fn clone(from: &str, dir: &str) -> RepoCopy {
    RepoCopy::Clone {
        from: from.into(),
        dir: Some(PathBuf::from(dir)),
    }
}

fn folder(from: &str, dir: &str) -> RepoCopy {
    RepoCopy::Folder {
        from: from.into(),
        dir: Some(PathBuf::from(dir)),
    }
}

#[test]
fn clones_and_copies_are_read_from_the_command() {
    let cases: Vec<(&str, bool, Vec<RepoCopy>)> = vec![
        ("git clone . ../app-2", false, vec![clone(".", "")]),
        (
            "git clone --depth 1 -b main -- ../app ../app-2",
            false,
            vec![clone("../app", "")],
        ),
        ("git -C .. clone app app-2", false, vec![clone("app", "..")]),
        (
            "cd .. && git clone ./app ./app-2",
            false,
            vec![clone("./app", "..")],
        ),
        (
            "git clone git@github.com:acme/app.git ../x",
            false,
            vec![clone("git@github.com:acme/app.git", "")],
        ),
        (
            "gh repo clone acme/app ../x",
            false,
            vec![clone("https://github.com/acme/app", "")],
        ),
        (
            "git clone \"$(git remote get-url origin)\" ../x",
            false,
            vec![RepoCopy::CloneRemote],
        ),
        (
            "git clone $(git config remote.origin.url) ../x",
            false,
            vec![RepoCopy::CloneRemote],
        ),
        ("cp -r . ../app-copy", false, vec![folder(".", "")]),
        ("cp -a ./ ../app-copy", false, vec![folder("./", "")]),
        ("cp -R \"$PWD\" /tmp/app", false, vec![folder(".", "")]),
        (
            "cp --recursive -t ../out . src",
            false,
            vec![folder(".", ""), folder("src", "")],
        ),
        ("cd .. ; cp -rf app app-2", false, vec![folder("app", "..")]),
        (
            "Copy-Item -Recurse -Path . -Destination ..\\app-copy",
            false,
            vec![folder(".", "")],
        ),
        ("cp -r . ..\\app-copy", true, vec![folder(".", "")]),
        (
            "copy -Path:$PWD -Destination ..\\x -Recurse",
            true,
            vec![folder(".", "")],
        ),
        (
            "xcopy . ..\\app-copy /E /I /H",
            false,
            vec![folder(".", "")],
        ),
        (
            "robocopy C:\\work\\app C:\\work\\app-2 /MIR",
            false,
            vec![folder("C:\\work\\app", "")],
        ),
        (
            "cmd //c \"xcopy . ..\\app-copy //E //I\"",
            false,
            vec![folder(".", "")],
        ),
        (
            "pwsh -Command \"cp -r . ../app-copy\"",
            false,
            vec![folder(".", "")],
        ),
        ("git worktree add ../x", false, vec![RepoCopy::Worktree]),
    ];
    for (cmd, powershell, want) in cases {
        assert_eq!(repo_copies(cmd, powershell), want, "{cmd}");
    }
    // `cd -` loses track of where the command runs.
    assert_eq!(
        repo_copies("cd - && cp -r . ../x", false),
        [RepoCopy::Folder {
            from: ".".into(),
            dir: None
        }]
    );

    for (cmd, powershell) in [
        ("npm test", false),
        ("git status", false),
        ("git clone --bare . ../app.git", false),
        ("git clone --mirror https://github.com/acme/app", false),
        ("cp . ../x", false),
        ("cp -r ../template", false),
        ("cp src/a.ts src/b.ts", false),
        ("Copy-Item . ..\\x", true),
        ("copy . ..\\x", false),
        ("xcopy . ..\\x /I", false),
        ("robocopy . ..\\x", false),
        (
            "robocopy . \\\\server\\app /MIR /XD .git node_modules",
            false,
        ),
        ("Copy-Item -Recurse . ..\\x -Exclude .git", true),
        ("echo cp -r . ../x", false),
        ("grep -r 'git clone' docs", false),
        ("gh pr checkout 12", false),
    ] {
        assert_eq!(repo_copies(cmd, powershell), [], "{cmd}");
    }
}

#[test]
fn remotes_match_however_they_are_written() {
    let key = |u: &str| copies::remote_key(u);
    let want = Some("github.com/acme/app".to_string());
    for url in [
        "https://github.com/acme/app",
        "https://github.com/Acme/App.git",
        "https://user@github.com/acme/app/",
        "git@github.com:acme/app.git",
        "ssh://git@github.com:22/acme/app",
        "git+ssh://git@github.com/acme/app.git",
    ] {
        assert_eq!(key(url), want, "{url}");
    }
    assert_eq!(
        key("https://gitlab.com/acme/app").as_deref(),
        Some("gitlab.com/acme/app")
    );
    for local in [
        ".",
        "../app",
        "C:\\work\\app",
        "C:/work/app",
        "/work/app",
        "file:///work/app",
    ] {
        assert_eq!(key(local), None, "{local}");
    }
}

#[test]
fn only_copies_of_this_repo_are_turned_down() {
    let t = TempRepo::new("copies");
    t.git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    t.git(&["remote", "add", "origin", "https://github.com/acme/app.git"]);
    fs::create_dir_all(t.0.join("src")).unwrap();
    let root = &t.0;
    let name = root.file_name().unwrap().to_string_lossy().to_string();
    let reason = |cmd: &str, cwd: &Path| deny_reason(&repo_copies(cmd, false), cwd, root);

    for cmd in [
        "git clone . ../app-2".to_string(),
        "git clone ./.git ../app-2".to_string(),
        format!("git clone ../{name} ../app-2"),
        format!("git clone \"{}\" ../app-2", root.display()),
        format!("cd .. && git clone {name} app-2"),
        "git clone git@github.com:Acme/App ../app-2".to_string(),
        "git clone \"$(git remote get-url origin)\" ../app-2".to_string(),
        "gh repo clone acme/app ../app-2".to_string(),
    ] {
        assert_eq!(reason(&cmd, root), Some(CLONE_DENIED), "{cmd}");
    }
    for cmd in [
        "cp -r . ../app-copy".to_string(),
        "cp -r ./ ../app-copy".to_string(),
        format!("cd .. && cp -r {name} {name}-2"),
        "xcopy . ..\\app-copy /E /I".to_string(),
    ] {
        assert_eq!(reason(&cmd, root), Some(COPY_DENIED), "{cmd}");
    }
    // `..` from a subfolder is the repo's folder too.
    assert_eq!(
        reason("git clone .. ../../app-2", &root.join("src")),
        Some(CLONE_DENIED)
    );
    assert_eq!(reason("git worktree add ../x", root), Some(WORKTREE_DENIED));
    for cmd in [
        "git clone https://github.com/acme/other ../other",
        "git clone ../somewhere-else ../x",
        "cp -r src ../src-copy",
        "cp -r * ../app-copy",
        // Copying into the repo's folder isn't copying the repo.
        "cp -r ../template .",
        "cd - && cp -r . ../x",
        "npm test",
    ] {
        assert_eq!(reason(cmd, root), None, "{cmd}");
    }
    // From a subfolder, `.` isn't the repo's folder.
    assert_eq!(reason("cp -r . ../src-copy", &root.join("src")), None);

    // Every worktree of the repo counts, so a lane can't be copied either.
    let wt = root.with_file_name(format!("{name}-wt"));
    let _ = fs::remove_dir_all(&wt);
    t.git(&["worktree", "add", "-q", "--detach", &wt.to_string_lossy()]);
    assert_eq!(reason("cp -r . ../wt-copy", &wt), Some(COPY_DENIED));
    let _ = fs::remove_dir_all(&wt);

    // Git Bash spells C:\work as /c/work.
    if cfg!(windows) {
        let unix = root.to_string_lossy().replace('\\', "/");
        let unix = format!("/{}{}", unix[..1].to_lowercase(), &unix[2..]);
        assert_eq!(
            reason(&format!("cp -r \"{unix}\" ../app-copy"), root),
            Some(COPY_DENIED),
            "{unix}"
        );
    }
}

#[test]
fn each_denial_has_its_own_reason_and_says_what_to_run() {
    let reasons = [WORKTREE_DENIED, CLONE_DENIED, COPY_DENIED];
    for r in reasons {
        assert!(r.contains("lane new <name> --print-path"), "{r}");
    }
    assert!(CLONE_DENIED.contains("cloning"));
    assert!(COPY_DENIED.contains("copying") && COPY_DENIED.contains("git archive"));
    assert_ne!(WORKTREE_DENIED, CLONE_DENIED);
    assert_ne!(CLONE_DENIED, COPY_DENIED);
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

    // core.hooksPath set to the repo's own hooks folder is the same as not set.
    let t2 = TempRepo::new("githook-own");
    t2.git(&["config", "core.hooksPath", ".git/hooks"]);
    assert!(setup_git_hook(&t2.repo()).unwrap().changed);
    assert!(t2.0.join(".git/hooks/post-checkout").is_file());
}

#[test]
fn husky_gets_one_line_first_in_its_post_checkout() {
    let t = TempRepo::new("husky");
    t.git(&["config", "core.hooksPath", ".husky/_"]);
    let repo = t.repo();
    let hook = t.0.join(".husky/post-checkout");

    // No post-checkout yet: it's made with just the line.
    let change = setup_git_hook(&repo).unwrap();
    assert!(change.changed && change.check_in, "{change:?}");
    assert_eq!(change.path, hook);
    assert!(
        change
            .note
            .unwrap()
            .contains("added a line to .husky/post-checkout")
    );
    assert_eq!(fs::read_to_string(&hook).unwrap(), format!("{HOOK_LINE}\n"));
    assert!(!t.0.join(".git/hooks/post-checkout").exists());
    let again = setup_git_hook(&repo).unwrap();
    assert!(!again.changed && again.check_in);
    assert!(again.note.unwrap().contains("already set up"));
    assert_eq!(fs::read_to_string(&hook).unwrap(), format!("{HOOK_LINE}\n"));

    // An existing script keeps its `#!` line first and its line endings.
    fs::write(&hook, "#!/usr/bin/env sh\r\nnpm run sync\r\n").unwrap();
    assert!(setup_git_hook(&repo).unwrap().changed);
    assert_eq!(
        fs::read_to_string(&hook).unwrap(),
        format!("#!/usr/bin/env sh\r\n{HOOK_LINE}\r\nnpm run sync\r\n")
    );
    fs::write(&hook, "npm run sync").unwrap();
    setup_git_hook(&repo).unwrap();
    assert_eq!(
        fs::read_to_string(&hook).unwrap(),
        format!("{HOOK_LINE}\nnpm run sync")
    );

    // Husky in a subfolder of a monorepo.
    let t2 = TempRepo::new("husky-web");
    t2.git(&["config", "core.hooksPath", "web/.husky/_/"]);
    let change = setup_git_hook(&t2.repo()).unwrap();
    assert_eq!(change.path, t2.0.join("web/.husky/post-checkout"));
    assert!(change.path.is_file());
}

#[test]
fn the_husky_line_runs_lane_only_where_it_is_installed() {
    // A teammate without `lane`: the line does nothing and the hook still succeeds, even with
    // `sh -e`, the way Husky runs it. PATH is an empty folder, so no `lane` is found.
    let paths = std::env::var_os("PATH").unwrap_or_default();
    let Some(sh) = std::env::split_paths(&paths)
        .flat_map(|d| [d.join("sh"), d.join("sh.exe")])
        .find(|p| p.is_file())
    else {
        return; // No sh on this machine.
    };
    let t = TempRepo::new("husky-line");
    let empty = t.0.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let out = Command::new(sh)
        .args([
            "-e",
            "-c",
            &format!("{HOOK_LINE}\necho after"),
            "post-checkout",
        ])
        .args(["0000000000000000000000000000000000000000", "abc", "1"])
        .current_dir(&t.0)
        .env("PATH", &empty)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "after");
}

#[test]
fn other_hook_folders_get_the_line_to_add_and_nothing_is_written() {
    for custom in [".githooks", ".husky"] {
        let t = TempRepo::new(&format!("hookspath{custom}"));
        t.git(&["config", "core.hooksPath", custom]);
        let change = setup_git_hook(&t.repo()).unwrap();
        assert!(!change.changed && !change.check_in, "{change:?}");
        assert_eq!(change.path, t.0.join(custom).join("post-checkout"));
        assert!(!change.path.exists());
        assert!(!t.0.join(".git/hooks/post-checkout").exists());
        let note = change.note.unwrap();
        assert!(note.contains("core.hooksPath"), "{note}");
        assert!(note.contains(&format!("{custom}/post-checkout")), "{note}");
        assert_eq!(change.snippet.unwrap(), format!("{HOOK_LINE}\n"));
    }
}

#[test]
fn lefthook_gets_a_command_for_its_config() {
    let t = TempRepo::new("lefthook");
    let repo = t.repo();
    let hook = t.0.join(".git/hooks/post-checkout");

    // lefthook without post-checkout leaves that hook file alone, so it's ours as usual.
    fs::write(
        t.0.join("lefthook.yml"),
        "pre-commit:\n  commands:\n    lint:\n      run: npm run lint\n",
    )
    .unwrap();
    assert!(setup_git_hook(&repo).unwrap().changed);
    assert!(hook.is_file());
    fs::remove_file(&hook).unwrap();

    // With post-checkout, lefthook owns the file: nothing is written, and the note says what to
    // add to lefthook.yml.
    fs::write(
        t.0.join("lefthook.yml"),
        "post-checkout:\n  commands:\n    deps:\n      run: npm install\n",
    )
    .unwrap();
    let change = setup_git_hook(&repo).unwrap();
    assert!(!change.changed && !change.check_in, "{change:?}");
    assert_eq!(change.path, t.0.join("lefthook.yml"));
    assert!(!hook.exists());
    assert!(change.note.unwrap().contains("lefthook.yml"));
    let snippet = change.snippet.unwrap();
    assert_eq!(
        snippet,
        "post-checkout:\n  commands:\n    lanes:\n      run: command -v lane >/dev/null 2>&1 && lane hook git post-checkout {0} >/dev/null 2>&1 || true\n"
    );

    // Once it's there, it's set up.
    fs::write(
        t.0.join("lefthook.yml"),
        format!(
            "post-checkout:\n  commands:\n    deps:\n      run: npm install\n    lanes:\n      run: {LEFTHOOK_RUN}\n"
        ),
    )
    .unwrap();
    let change = setup_git_hook(&repo).unwrap();
    assert!(!change.changed && change.snippet.is_none());
    assert!(change.note.unwrap().contains("already set up"));

    // TOML, and post-checkout in lefthook-local.yml only.
    let t2 = TempRepo::new("lefthook-toml");
    fs::write(
        t2.0.join("lefthook.toml"),
        "[post-checkout.commands.deps]\nrun = \"npm i\"\n",
    )
    .unwrap();
    let snippet = setup_git_hook(&t2.repo()).unwrap().snippet.unwrap();
    assert!(
        snippet.starts_with("[post-checkout.commands.lanes]\nrun = \""),
        "{snippet}"
    );
    let t3 = TempRepo::new("lefthook-local");
    fs::write(t3.0.join("lefthook.yml"), "pre-commit:\n").unwrap();
    fs::write(t3.0.join("lefthook-local.yml"), "post-checkout:\n").unwrap();
    let change = setup_git_hook(&t3.repo()).unwrap();
    assert_eq!(change.path, t3.0.join("lefthook-local.yml"));
    assert!(!t3.0.join(".git/hooks/post-checkout").exists());
}

#[test]
fn the_plugin_runs_the_same_hooks_as_lane_init() {
    let file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/claude-code/hooks/hooks.json");
    let plugin: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(plugin["hooks"], claude_hooks(), "update {}", file.display());
}
