//! The engine against real Git repos in temporary folders.

use super::*;
use crate::state::Store;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Lanes Test",
            "-c",
            "user.email=test@lanes.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo with a commit on `main`, a lane.toml whose install step appends to `installs.log`
/// (in `sh` and `cmd` alike), and a lockfile.
fn repo(name: &str) -> (PathBuf, Store) {
    let root = std::env::var_os("LANE_TEST_CLONE_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("lane-engine-{}-{name}", std::process::id()));
    let _ = remove_dir(&root);
    let dir = root.join("acme");
    fs::create_dir_all(dir.join("src")).unwrap();
    git_in(&dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("src/app.ts"), "export const app = 1;\n").unwrap();
    fs::write(dir.join("package-lock.json"), "{\"v\": 1}\n").unwrap();
    fs::write(dir.join(".gitignore"), "installs.log\n").unwrap();
    fs::write(
        dir.join("lane.toml"),
        "version = 1\n[setup]\ninstall = [\"git --version >> installs.log\"]\n",
    )
    .unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "first"]);
    let store = Store::open_at(&root.join("state/state.db")).unwrap();
    (dir, store)
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    fs::write(dir.join(file), text).unwrap();
    git_in(dir, &["add", "-A"]);
    git_in(dir, &["commit", "-q", "-m", file]);
    git_in(dir, &["rev-parse", "HEAD"])
}

fn installs(base: &Path) -> usize {
    fs::read_to_string(base.join("installs.log"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[test]
fn a_new_lane_is_a_ready_worktree_with_dependencies_and_no_branch() {
    let (dir, mut store) = repo("new");
    let repo = Repo::open(&dir).unwrap();
    let made = new_lane(&mut store, &repo, "fix-auth", Some("claude"), None).unwrap();

    let lane = &made.lane;
    assert_eq!(lane.status, Status::Ready);
    assert_eq!(lane.path, repo.lane_path("fix-auth"));
    assert!(made.base.created && made.base.installed, "{:?}", made.base);
    assert_eq!(installs(&repo.base_path()), 1);
    // Tracked files and installed dependencies both came across.
    // (Compared with _base's copy, since Git may convert line endings on checkout.)
    assert_eq!(
        fs::read(lane.path.join("src/app.ts")).unwrap(),
        fs::read(repo.base_path().join("src/app.ts")).unwrap()
    );
    assert!(
        lane.path.join("installs.log").exists(),
        "dependencies from _base"
    );
    assert!(
        made.clone.files >= 4 && made.clone.cloned + made.clone.copied <= made.clone.files,
        "{:?}",
        made.clone
    );
    // On a disk that clones (macOS, and the Btrfs, XFS and ReFS disks in CI), nothing is copied.
    if cfg!(target_os = "macos") || std::env::var_os("LANE_TEST_CLONE_DIR").is_some() {
        assert_eq!(
            made.clone.method,
            crate::clone::Method::Clone,
            "{:?}",
            made.clone
        );
        assert_eq!(made.clone.copied, 0, "{:?}", made.clone);
    }
    // A real worktree: clean status, detached at the base commit, and no new branch.
    assert_eq!(git_in(&lane.path, &["status", "--porcelain"]), "");
    assert_eq!(git_in(&lane.path, &["rev-parse", "HEAD"]), made.base.commit);
    assert_eq!(
        git_in(&lane.path, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD"
    );
    assert_eq!(
        git_in(&dir, &["branch", "--list", "--format=%(refname:short)"]),
        "main"
    );
    assert_eq!(lane.ports.start, 4100);
    // Listed, and findable by folder.
    assert_eq!(store.lanes(Some(&repo.root)).unwrap().len(), 1);
    assert_eq!(store.lane_at(&lane.path).unwrap().unwrap().id, lane.id);
    // A change in the lane shows up in git status there only.
    fs::write(lane.path.join("src/app.ts"), "export const app = 2;\n").unwrap();
    assert!(git_in(&lane.path, &["status", "--porcelain"]).contains("src/app.ts"));
    assert_eq!(git_in(&dir, &["status", "--porcelain"]), "");
}

#[test]
fn the_second_lane_reuses_base_and_names_must_be_free() {
    let (dir, mut store) = repo("second");
    let repo = Repo::open(&dir).unwrap();
    new_lane(&mut store, &repo, "a", None, None).unwrap();
    let b = new_lane(&mut store, &repo, "b", None, None).unwrap();
    assert!(!b.base.created && !b.base.installed, "{:?}", b.base);
    assert_eq!(installs(&repo.base_path()), 1, "installed once");
    assert_eq!(b.lane.ports.start, 4110);
    let e = new_lane(&mut store, &repo, "a", None, None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    let e = new_lane(&mut store, &repo, "Bad_Name", None, None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
}

#[test]
fn base_follows_main_and_reinstalls_only_when_a_lockfile_changes() {
    let (dir, mut store) = repo("follow");
    let repo = Repo::open(&dir).unwrap();
    new_lane(&mut store, &repo, "a", None, None).unwrap();

    let c2 = commit(&dir, "src/new.ts", "export const n = 1;\n");
    let b = new_lane(&mut store, &repo, "b", None, None).unwrap();
    assert!(b.base.moved && !b.base.installed, "{:?}", b.base);
    assert_eq!(b.base.commit, c2);
    assert!(b.lane.path.join("src/new.ts").exists());
    assert_eq!(installs(&repo.base_path()), 1);

    commit(&dir, "package-lock.json", "{\"v\": 2}\n");
    let c = new_lane(&mut store, &repo, "c", None, None).unwrap();
    assert!(c.base.moved && c.base.installed, "{:?}", c.base);
    assert_eq!(installs(&repo.base_path()), 2);
}

#[test]
fn rm_removes_the_folder_the_worktree_record_and_the_ports() {
    let (dir, mut store) = repo("rm");
    let repo = Repo::open(&dir).unwrap();
    let a = new_lane(&mut store, &repo, "a", None, None).unwrap().lane;
    let gone = remove(&mut store, &repo, "a", false, None).unwrap();
    assert_eq!(gone.id, a.id);
    assert!(!a.path.exists());
    let listed = git_in(&dir, &["worktree", "list", "--porcelain"]);
    assert!(
        !listed.contains("/a\n") && !listed.ends_with("/a"),
        "{listed}"
    );
    assert!(store.lane(&repo.root, "a").unwrap().is_none());
    // Its ports are free again.
    let b = new_lane(&mut store, &repo, "b", None, None).unwrap();
    assert_eq!(b.lane.ports.start, 4100);
    let e = remove(&mut store, &repo, "a", false, None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn an_existing_worktree_can_be_adopted_once() {
    let (dir, mut store) = repo("adopt");
    let repo = Repo::open(&dir).unwrap();
    let wt = dir.parent().unwrap().join("Agent Task_7");
    git_in(
        &dir,
        &["worktree", "add", "-q", "--detach", &wt.to_string_lossy()],
    );
    let lane = adopt(&mut store, &repo, &wt, None, Some("codex"), None).unwrap();
    assert_eq!(lane.name, "agent-task-7");
    assert_eq!(lane.status, Status::Ready);
    assert!(same_path(&lane.path, &wt));
    // Adopting again (from inside it, too) gives the same lane.
    let again = adopt(&mut store, &repo, &wt.join("src"), None, None, None).unwrap();
    assert_eq!(again.id, lane.id);
    // The repo itself can't be adopted.
    let e = adopt(&mut store, &repo, &dir, None, None, None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
    // A command run inside the lane still means the same repo.
    assert_eq!(Repo::open(&wt).unwrap().root, repo.root);
}

#[test]
fn a_failed_install_leaves_nothing_and_runs_again_next_time() {
    let (dir, mut store) = repo("failing");
    fs::write(
        dir.join("lane.toml"),
        "version = 1\n[setup]\ninstall = [\"git definitely-not-a-command\"]\n",
    )
    .unwrap();
    commit(
        &dir,
        "lane.toml",
        &fs::read_to_string(dir.join("lane.toml")).unwrap(),
    );
    let repo = Repo::open(&dir).unwrap();
    let e = new_lane(&mut store, &repo, "x", None, None).unwrap_err();
    assert!(e.to_string().contains("[setup] install"), "{e}");
    assert!(store.lanes(None).unwrap().is_empty(), "no lane recorded");
    assert!(!repo.lane_path("x").exists(), "no folder left behind");
    assert_eq!(
        store.base(&repo.root).unwrap().unwrap().install_key,
        None,
        "the install runs again next time"
    );
}

#[test]
fn a_retried_new_with_a_request_id_makes_one_lane() {
    let (dir, mut store) = repo("retry");
    let repo = Repo::open(&dir).unwrap();
    let first = new_lane(&mut store, &repo, "x", None, Some("req-1")).unwrap();
    let again = new_lane(&mut store, &repo, "x", None, Some("req-1")).unwrap();
    assert_eq!(first, again);
    assert_eq!(store.lanes(None).unwrap().len(), 1);
}

#[test]
fn virtualenv_paths_are_pointed_at_the_lane() {
    let root = std::env::temp_dir().join(format!("lane-venv-{}", std::process::id()));
    let _ = remove_dir(&root);
    let (base, lane) = (root.join("_base"), root.join("fix"));
    for dir in [&base, &lane] {
        fs::create_dir_all(dir.join(".venv/bin")).unwrap();
        let venv = base.join(".venv");
        fs::write(dir.join(".venv/pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        fs::write(
            dir.join(".venv/bin/activate"),
            format!("VIRTUAL_ENV=\"{}\"\n", venv.display()),
        )
        .unwrap();
        fs::write(
            dir.join(".venv/bin/pytest"),
            format!(
                "#!{}/bin/python\n",
                venv.to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        fs::write(dir.join(".venv/bin/python"), b"\x7fELF\0binary").unwrap();
    }
    let fixed = fix_paths(&base, &lane).unwrap();
    assert_eq!(
        fixed, 2,
        "activate and pytest; pyvenv.cfg and the binary are left alone"
    );
    let activate = fs::read_to_string(lane.join(".venv/bin/activate")).unwrap();
    assert!(
        activate.contains(&lane.join(".venv").display().to_string()),
        "{activate}"
    );
    let pytest = fs::read_to_string(lane.join(".venv/bin/pytest")).unwrap();
    assert!(
        pytest.contains(&lane.join(".venv").to_string_lossy().replace('\\', "/")),
        "{pytest}"
    );
    assert!(
        fs::read_to_string(base.join(".venv/bin/activate"))
            .unwrap()
            .contains("_base"),
        "base untouched"
    );
    let _ = remove_dir(&root);
}

#[test]
fn folder_names_become_lane_names() {
    assert_eq!(lane_name("fix-auth"), "fix-auth");
    assert_eq!(lane_name("Agent Task_7"), "agent-task-7");
    assert_eq!(lane_name("--weird--"), "weird");
    assert_eq!(lane_name("日本"), "lane");
    assert_eq!(lane_name(&"x".repeat(60)).len(), 40);
}

#[test]
fn paths_have_one_spelling() {
    if cfg!(windows) {
        assert_eq!(
            norm(Path::new("c:/Users/me/acme")),
            PathBuf::from("C:\\Users\\me\\acme")
        );
    }
    assert_eq!(
        clean(Path::new("/work/acme/../acme.lanes/./x")),
        PathBuf::from("/work/acme.lanes/x")
    );
}

#[test]
fn a_repo_without_lane_toml_uses_what_init_would_find() {
    let root = std::env::temp_dir().join(format!("lane-detected-{}", std::process::id()));
    let _ = remove_dir(&root);
    fs::create_dir_all(&root).unwrap();
    git_in(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("package-lock.json"),
        "{}
",
    )
    .unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"scripts": {"dev": "vite"}, "devDependencies": {"vite": "6"}}"#,
    )
    .unwrap();
    let repo = Repo::open(&root).unwrap();
    assert_eq!(repo.project.setup.install, ["npm ci"]);
    assert_eq!(
        repo.project.dev.command.as_deref(),
        Some("npm run dev -- --port $PORT")
    );
    // With a lane.toml, that file wins, even when it says less.
    fs::write(
        root.join("lane.toml"),
        "version = 1
",
    )
    .unwrap();
    let repo = Repo::open(&root).unwrap();
    assert!(repo.project.setup.install.is_empty());
    let _ = remove_dir(&root);
}

#[test]
fn a_worktree_another_tool_made_gets_dependencies_from_base() {
    let (dir, mut store) = repo("ready");
    // The install step makes a dependency folder, the way `npm ci` makes node_modules.
    fs::write(
        dir.join("lane.toml"),
        "version = 1\n[setup]\ninstall = [\"mkdir node_modules\", \"git --version >> installs.log\"]\n",
    )
    .unwrap();
    fs::write(dir.join(".gitignore"), "installs.log\nnode_modules/\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "deps"]);
    let repo = Repo::open(&dir).unwrap();

    // Any tool's `git worktree add`: no dependencies of its own.
    let other = dir.with_file_name("by-a-tool");
    git_in(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &other.to_string_lossy(),
        ],
    );
    assert!(!other.join("node_modules").exists());
    let lane = adopt(&mut store, &repo, &other, None, Some("codex"), None).unwrap();

    let ready = make_ready(&mut store, &repo, &lane).unwrap();
    assert_eq!(ready.deps, Deps::Cloned, "{ready:?}");
    assert_eq!(ready.folders, [PathBuf::from("node_modules")]);
    assert!(other.join("node_modules").is_dir());
    assert_eq!(installs(&repo.base_path()), 1, "installed once, in _base");
    assert_eq!(installs(&other), 0, "nothing ran in the worktree itself");

    // Again: nothing to do, nothing runs.
    let again = make_ready(&mut store, &repo, &lane).unwrap();
    assert_eq!(again.deps, Deps::None);
    assert_eq!(installs(&repo.base_path()), 1);

    // Different lockfile: the install runs in the worktree instead.
    let third = dir.with_file_name("other-deps");
    git_in(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &third.to_string_lossy(),
        ],
    );
    fs::write(third.join("package-lock.json"), "{\"v\": 2}\n").unwrap();
    git_in(&third, &["add", "-A"]);
    git_in(&third, &["commit", "-q", "-m", "bump"]);
    let lane = adopt(&mut store, &repo, &third, None, None, None).unwrap();
    let ready = make_ready(&mut store, &repo, &lane).unwrap();
    assert_eq!(ready.deps, Deps::Installed);
    assert_eq!(installs(&third), 1);
}
