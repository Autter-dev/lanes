//! The lane engine: the clean `_base` lane, and making, adopting and removing lanes.
//!
//! - **`_base`** is a worktree on the base branch with dependencies installed once (`[setup]
//!   install` in `lane.toml`). It moves when the branch moves; dependencies install again only
//!   when a lockfile or the install commands change.
//! - **`lane new`** reserves the name and ports first (status `creating`), adds a worktree record
//!   without writing any files, clones `_base`'s files in, copies `_base`'s Git index so
//!   `git status` works at once, fixes paths that point at `_base`, and marks the lane `ready`.
//!   If anything fails, everything it made is removed again, so a retry starts clean.
//! - **`lane adopt`** turns a worktree the repo already has into a lane.
//! - **`lane rm`** removes a lane's folder and worktree record, and frees its ports.
//!
//! One `lane new` runs at a time per repo (a lock in the lanes folder), so `_base` never moves
//! under a clone. Lanes never create branches.

use crate::clone::{self, Method};
use crate::error::{Error, ErrorKind, Result};
use crate::git;
use crate::project::Project;
use crate::state::{self, Lane, NewLane, Request, Status, Store};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// Lockfiles whose change means dependencies must be installed again.
const LOCKFILES: [&str; 16] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "uv.lock",
    "poetry.lock",
    "Pipfile.lock",
    "requirements.txt",
    "Gemfile.lock",
    "go.sum",
    "Cargo.lock",
    "composer.lock",
    "mix.lock",
    "pubspec.lock",
];

/// The repo a command works on: identified by its main worktree, so a command run inside a
/// lane still means the same repo.
#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
    pub project: Project,
    pub lanes_dir: PathBuf,
}

impl Repo {
    pub fn open(dir: &Path) -> Result<Repo> {
        let top = crate::repo::top_level(dir)?;
        let root = git::worktrees(&top)?
            .into_iter()
            .next()
            .map(|p| norm(&p))
            .unwrap_or_else(|| norm(&top));
        // No lane.toml: use what `lane init` would find, so every lane gets its dependencies.
        let project = if root.join(crate::project::FILE_NAME).is_file() {
            Project::load(&root)?
        } else {
            Project::detected(&crate::detect::detect(&root))
        };
        let lanes_dir = norm(&clean(&project.lanes_dir(&root)));
        Ok(Repo {
            root,
            project,
            lanes_dir,
        })
    }

    pub fn base_path(&self) -> PathBuf {
        self.lanes_dir.join("_base")
    }

    pub fn lane_path(&self, name: &str) -> PathBuf {
        self.lanes_dir.join(name)
    }

    /// Whether the repo has a lane.toml (`lane init` writes one).
    pub fn has_lane_toml(&self) -> bool {
        self.root.join(crate::project::FILE_NAME).is_file()
    }
}

/// One spelling per path, so the same folder never looks like two: Windows paths with `\` and
/// an uppercase drive letter (Git prints `c:/…` or `C:/…`), others as they are.
pub fn norm(p: &Path) -> PathBuf {
    if cfg!(windows) {
        let mut s = p.to_string_lossy().replace('/', "\\");
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            s[..1].make_ascii_uppercase();
        }
        PathBuf::from(s)
    } else {
        p.to_path_buf()
    }
}

/// Collapse `.` and `..` without touching the disk.
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// What `_base` did on the way to a new lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseInfo {
    pub path: PathBuf,
    /// The branch it follows.
    pub branch: String,
    pub commit: String,
    /// Made just now.
    pub created: bool,
    /// Moved to a newer commit of its branch just now.
    pub moved: bool,
    /// Dependencies were installed just now.
    pub installed: bool,
}

/// How a lane's files got there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneSummary {
    pub method: Method,
    /// The disk's filesystem, when known.
    pub filesystem: Option<String>,
    pub files: usize,
    pub cloned: usize,
    pub copied: usize,
    pub bytes: u64,
    pub millis: u64,
}

/// A new lane, and how it was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Made {
    pub lane: Lane,
    pub base: BaseInfo,
    pub clone: CloneSummary,
    /// Files whose paths pointed at `_base` and now point at the lane.
    pub fixed_paths: usize,
    /// The lane's `.env`, when one was written (from `env.template`, or `_base`'s own).
    #[serde(default)]
    pub env_file: Option<PathBuf>,
}

fn io_error(what: &str, path: &Path, e: io::Error) -> Error {
    Error::new(
        ErrorKind::Io,
        format!("couldn't {what} {}: {e}", path.display()),
    )
    .hint("Check that the folder exists and that you can write to it, then try again.")
}

/// One `lane new` at a time per repo.
struct RepoLock(#[allow(dead_code)] File);

impl RepoLock {
    fn acquire(lanes_dir: &Path) -> Result<RepoLock> {
        let path = lanes_dir.join(".lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| io_error("open", &path, e))?;
        file.lock().map_err(|e| io_error("lock", &path, e))?;
        Ok(RepoLock(file))
    }
}

/// Make the lane `name`: an instant (or, on disks that can't clone, copied) copy of `_base`
/// with its own worktree record and ports. Safe to retry with a request id.
pub fn new_lane(
    store: &mut Store,
    repo: &Repo,
    name: &str,
    agent: Option<&str>,
    request_id: Option<&str>,
) -> Result<Made> {
    state::check_name(name)?;
    let command = format!("new {name}");
    let request = request_id.map(|id| Request {
        id,
        command: &command,
    });
    if let Some(r) = request
        && let Some(done) = store.request_result::<Made>(r)?
    {
        return Ok(done);
    }
    fs::create_dir_all(&repo.lanes_dir).map_err(|e| io_error("create", &repo.lanes_dir, e))?;
    let _lock = RepoLock::acquire(&repo.lanes_dir)?;

    let path = repo.lane_path(name);
    let ports = repo.project.ports.clone();
    let reserved = store.write(None, |tx| {
        let block = tx.allocate_ports(ports.start, ports.per_lane)?;
        tx.insert_lane(NewLane {
            repo: &repo.root,
            name,
            path: &path,
            base_commit: None,
            ports: block,
            agent,
            status: Status::Creating,
        })
    })?;

    match build_lane(store, repo, &reserved) {
        Ok((base, clone, fixed_paths, env_file)) => store.write(request, |tx| {
            tx.finish_lane(&reserved.id, &base.commit)?;
            let mut lane = reserved.clone();
            lane.status = Status::Ready;
            lane.base_commit = Some(base.commit.clone());
            Ok(Made {
                lane,
                base,
                clone,
                fixed_paths,
                env_file,
            })
        }),
        Err(e) => {
            let _ = remove_folder(repo, &path);
            let _ = store.write(None, |tx| tx.drop_lane(&reserved.id));
            Err(e)
        }
    }
}

type Built = (BaseInfo, CloneSummary, usize, Option<PathBuf>);

/// Agent settings that live only on this machine (Claude Code's hooks among them), copied from
/// the repo into the lane so an agent started in the lane has them too.
fn copy_local_agent_files(root: &Path, lane: &Path) -> Result<()> {
    for rel in crate::agents::LOCAL_AGENT_FILES {
        let (from, to) = (root.join(rel), lane.join(rel));
        if from.is_file() && !to.exists() {
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir).map_err(|e| io_error("create", dir, e))?;
            }
            fs::copy(&from, &to).map_err(|e| io_error("copy", &from, e))?;
        }
    }
    Ok(())
}

fn build_lane(store: &mut Store, repo: &Repo, lane: &Lane) -> Result<Built> {
    let path = lane.path.as_path();
    let base = ensure_base(store, repo)?;
    // The name is reserved for us, so anything already at the path is left from a crash.
    if path.exists() {
        remove_folder(repo, path)?;
    }
    git::worktree_add(&repo.root, path, &base.commit, false)?;
    let method = clone::probe(&repo.lanes_dir);
    let report = clone::clone_tree(
        &base.path,
        path,
        clone::backend(method).as_ref(),
        &clone::Options::default(),
    )?;
    // `_base`'s index lists exactly these files, so `git status` works without a full checkout.
    let (from, to) = (
        git::git_path(&base.path, "index")?,
        git::git_path(path, "index")?,
    );
    fs::copy(&from, &to).map_err(|e| io_error("copy the Git index to", &to, e))?;
    git::refresh_index(path);
    let fixed = fix_paths(&base.path, path)?;
    copy_local_agent_files(&repo.root, path)?;
    let env_file = crate::lane_env::write_dotenv(lane, repo.project.env.template.as_deref())?;
    let summary = CloneSummary {
        method: report.method,
        filesystem: crate::machine::filesystem_name(&repo.lanes_dir),
        files: report.files,
        cloned: report.cloned,
        copied: report.copied,
        bytes: report.bytes,
        millis: report.elapsed.as_millis() as u64,
    };
    Ok((base, summary, fixed, env_file))
}

/// Make `_base` if it's missing, move it to the base branch's latest commit, and install
/// dependencies when they changed. Call with the repo lock held.
pub fn ensure_base(store: &mut Store, repo: &Repo) -> Result<BaseInfo> {
    let (branch, commit) = git::base_branch(&repo.root, repo.project.lanes.base.as_deref())?;
    let path = repo.base_path();
    let record = store.base(&repo.root)?;
    let (mut created, mut moved) = (false, false);
    if !path.join(".git").is_file() {
        if path.exists() {
            remove_dir(&path).map_err(|e| io_error("remove", &path, e))?;
        }
        git::worktree_prune(&repo.root);
        git::worktree_add(&repo.root, &path, &commit, true)?;
        created = true;
    } else if git::commit_of(&path, "HEAD").as_deref() != Some(commit.as_str()) {
        git::checkout_detached(&path, &commit)?;
        moved = true;
    }
    let commands = &repo.project.setup.install;
    let key = install_key(&path, &commit, commands)?;
    let stale = record.as_ref().and_then(|b| b.install_key.as_deref()) != Some(key.as_str());
    let mut installed = false;
    if created || stale {
        // Forget the old key first, so an install that fails runs again next time.
        store.write(None, |tx| tx.set_base(&repo.root, &path, &commit, None))?;
        install(&path, commands)?;
        installed = !commands.is_empty();
    }
    store.write(None, |tx| {
        tx.set_base(&repo.root, &path, &commit, Some(&key))
    })?;
    Ok(BaseInfo {
        path,
        branch,
        commit,
        created,
        moved,
        installed,
    })
}

/// What dependencies were installed from: the install commands and the lockfiles' blobs.
fn install_key(base: &Path, commit: &str, commands: &[String]) -> Result<String> {
    let blobs = git::blobs(base, commit, &LOCKFILES)?;
    Ok(format!("{}\n--\n{}", commands.join("\n"), blobs.join("\n")))
}

/// Run the install commands in `_base`, in order. Their output goes to stderr, so `--json`
/// output stays clean.
fn install(base: &Path, commands: &[String]) -> Result<()> {
    for cmd in commands {
        let started = std::time::Instant::now();
        // Inside an agent's hook the agent reads stderr, so the install talks to the log instead.
        let (out, err) = match crate::log::quiet().then(crate::log::file).flatten() {
            Some(f) => match f.try_clone() {
                Ok(g) => (Stdio::from(f), Stdio::from(g)),
                Err(_) => (Stdio::null(), Stdio::null()),
            },
            None => (Stdio::from(io::stderr()), Stdio::from(io::stderr())),
        };
        let status = shell(cmd)
            .current_dir(base)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .status()
            .map_err(|e| {
                crate::log::error(
                    "install",
                    format!("couldn't run `{cmd}` in {}: {e}", base.display()),
                )
                .write();
                Error::new(ErrorKind::Failure, format!("couldn't run `{cmd}`: {e}"))
                    .hint("Check the command in `[setup] install` in lane.toml.")
            })?;
        let ms = started.elapsed().as_millis() as u64;
        if !status.success() {
            let code = status
                .code()
                .map_or("a signal".to_string(), |c| format!("exit code {c}"));
            crate::log::error(
                "install",
                format!("`{cmd}` failed in {} ({code})", base.display()),
            )
            .ms(ms)
            .write();
            return Err(Error::new(
                ErrorKind::Failure,
                format!("`{cmd}` failed in {} ({code})", base.display()),
            )
            .hint("Fix the command in `[setup] install` in lane.toml (`lane init` writes one), or run it there by hand to see why. `lane logs` has what happened."));
        }
        crate::log::info("install", format!("`{cmd}` in {}", base.display()))
            .ms(ms)
            .write();
    }
    Ok(())
}

fn shell(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    }
}

/// Turn the worktree at `path` (made by `git worktree add`, an agent's own tool, or anything
/// else) into a lane. Adopting a folder that's already a lane returns that lane.
pub fn adopt(
    store: &mut Store,
    repo: &Repo,
    path: &Path,
    name: Option<&str>,
    agent: Option<&str>,
    request_id: Option<&str>,
) -> Result<Lane> {
    let top = norm(&crate::repo::top_level(path)?);
    if same_path(&top, &repo.root) {
        return Err(Error::usage(format!(
            "{} is the repo itself, not one of its worktrees",
            top.display()
        ))
        .hint(
            "Make a lane with `lane new <name>`, or adopt a folder made with `git worktree add`.",
        ));
    }
    if !same_path(&git::common_dir(&top)?, &git::common_dir(&repo.root)?) {
        return Err(
            Error::usage(format!("{} belongs to a different repo", top.display()))
                .hint("Run `lane adopt` from inside the repo that worktree belongs to."),
        );
    }
    if let Some(lane) = store.lane_at(&top)? {
        return Ok(lane);
    }
    let name = match name {
        Some(n) => n.to_string(),
        None => lane_name(
            &top.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    };
    state::check_name(&name)?;
    let commit = git::commit_of(&top, "HEAD");
    let command = format!("adopt {}", top.display());
    let request = request_id.map(|id| Request {
        id,
        command: &command,
    });
    let ports = repo.project.ports.clone();
    store.write(request, |tx| {
        let block = tx.allocate_ports(ports.start, ports.per_lane)?;
        tx.insert_lane(NewLane {
            repo: &repo.root,
            name: &name,
            path: &top,
            base_commit: commit.as_deref(),
            ports: block,
            agent,
            status: Status::Ready,
        })
    })
}

/// How an adopted worktree was made ready to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Deps {
    /// The repo installs nothing, or the worktree already had its dependency folders.
    None,
    /// Cloned from `_base`, whose lockfiles match.
    Cloned,
    /// The lockfiles differ from `_base`'s, so the install commands ran in the worktree.
    Installed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ready {
    pub deps: Deps,
    /// Dependency folders cloned from `_base`, relative to the worktree.
    pub folders: Vec<PathBuf>,
    pub fixed_paths: usize,
    pub env_file: Option<PathBuf>,
}

/// Folders that hold installed dependencies, cloned from `_base` into an adopted worktree.
const DEP_DIRS: [&str; 2] = ["node_modules", ".venv"];

/// Make a lane that another tool made (an adopted worktree) ready to run, like one from
/// `lane new`: its `.env`, the agent settings that live on this machine, and its dependencies.
/// When its lockfiles match `_base`'s, the dependency folders are cloned from `_base`, which is
/// instant where the disk clones files; otherwise the install commands run in the lane. Folders
/// already there are left alone, so running it again is safe.
pub fn make_ready(store: &mut Store, repo: &Repo, lane: &Lane) -> Result<Ready> {
    let path = lane.path.as_path();
    copy_local_agent_files(&repo.root, path)?;
    let env_file = crate::lane_env::write_dotenv(lane, repo.project.env.template.as_deref())?;
    let commands = &repo.project.setup.install;
    let mut ready = Ready {
        deps: Deps::None,
        folders: Vec::new(),
        fixed_paths: 0,
        env_file,
    };
    if commands.is_empty() {
        return Ok(ready);
    }
    fs::create_dir_all(&repo.lanes_dir).map_err(|e| io_error("create", &repo.lanes_dir, e))?;
    let _lock = RepoLock::acquire(&repo.lanes_dir)?;
    let base = ensure_base(store, repo)?;
    let head = git::commit_of(path, "HEAD").unwrap_or_default();
    let same = !head.is_empty()
        && git::blobs(path, &head, &LOCKFILES)?
            == git::blobs(&base.path, &base.commit, &LOCKFILES)?;
    if !same {
        install(path, commands)?;
        ready.deps = Deps::Installed;
        return Ok(ready);
    }
    let method = clone::probe(&repo.lanes_dir);
    for rel in dep_dirs(&base.path) {
        let to = path.join(&rel);
        if to.exists() {
            continue;
        }
        clone::clone_tree(
            &base.path.join(&rel),
            &to,
            clone::backend(method).as_ref(),
            &clone::Options::default(),
        )?;
        ready.folders.push(rel);
    }
    if !ready.folders.is_empty() {
        ready.deps = Deps::Cloned;
        ready.fixed_paths = fix_paths(&base.path, path)?;
    }
    Ok(ready)
}

/// The dependency folders in a checkout, relative to it: at the top and up to three levels
/// down, for monorepos (`packages/web/node_modules`). Never inside one, or inside `.git`.
fn dep_dirs(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, rel: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(root.join(rel)) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child = rel.join(name.as_ref());
            if DEP_DIRS.contains(&name.as_ref()) {
                out.push(child);
            } else if depth < 3 && name != ".git" && name != "target" {
                walk(root, &child, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, Path::new(""), 0, &mut out);
    out.sort();
    out
}

/// A lane name from a folder name: lowercase, with anything else turned into hyphens.
pub fn lane_name(folder: &str) -> String {
    let mut out = String::new();
    for c in folder.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(40).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "lane".to_string()
    } else {
        out
    }
}

/// Remove the lane `name`: its folder and worktree record (unless `keep_files`), then its record
/// and ports. Safe to retry with a request id.
pub fn remove(
    store: &mut Store,
    repo: &Repo,
    name: &str,
    keep_files: bool,
    request_id: Option<&str>,
) -> Result<Lane> {
    let command = format!("rm {name}");
    let request = request_id.map(|id| Request {
        id,
        command: &command,
    });
    if let Some(r) = request
        && let Some(done) = store.request_result::<Lane>(r)?
    {
        return Ok(done);
    }
    let lane = store.lane(&repo.root, name)?.ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            format!("no lane called `{name}` in this repo"),
        )
        .hint("Run `lane list` to see this repo's lanes.")
    })?;
    if same_path(&lane.path, &repo.root) || repo.root.starts_with(&lane.path) {
        return Err(Error::new(
            ErrorKind::Failure,
            format!(
                "refusing to delete {}, which holds the repo",
                lane.path.display()
            ),
        )
        .hint(format!(
            "Forget the lane without touching files: `lane rm {name} --keep-files`."
        )));
    }
    if !keep_files {
        remove_folder(repo, &lane.path)?;
    }
    store.write(request, |tx| tx.remove_lane(&repo.root, name))
}

/// Remove a lane's folder and its worktree record, even with read-only files in it.
fn remove_folder(repo: &Repo, path: &Path) -> Result<()> {
    if path.exists() {
        let _ = git::worktree_remove(&repo.root, path);
        if path.exists() {
            remove_dir(path).map_err(|e| io_error("remove", path, e))?;
        }
    }
    git::worktree_prune(&repo.root);
    Ok(())
}

fn remove_dir(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows won't delete read-only files: clear the flag everywhere, then try again.
            clear_readonly(path);
            fs::remove_dir_all(path)
        }
    }
}

fn clear_readonly(path: &Path) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            clear_readonly(&p);
        } else if let Ok(meta) = fs::symlink_metadata(&p) {
            let mut perm = meta.permissions();
            if perm.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                let _ = fs::set_permissions(&p, perm);
            }
        }
    }
}

/// Whether two paths are the same folder: by the filesystem when both exist, else by spelling.
pub fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => norm(a) == norm(b),
    }
}

/// Rewrite text files in a lane that name `_base`'s path so they name the lane's: Python
/// virtualenvs write their own absolute path into `pyvenv.cfg`, activate scripts and script
/// shebangs. Binary files (such as Windows `.exe` launchers) are left as they are.
pub fn fix_paths(base: &Path, lane: &Path) -> Result<usize> {
    let mut fixed = 0;
    for venv in [".venv", "venv", "env"] {
        let (from, to) = (base.join(venv), lane.join(venv));
        if !to.join("pyvenv.cfg").is_file() {
            continue;
        }
        let mut files = vec![to.join("pyvenv.cfg")];
        for dir in ["bin", "Scripts"] {
            if let Ok(entries) = fs::read_dir(to.join(dir)) {
                files.extend(
                    entries
                        .flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                        .map(|e| e.path()),
                );
            }
        }
        let (old, new) = (spellings(&from), spellings(&to));
        for f in files {
            if rewrite(&f, &old, &new).map_err(|e| io_error("fix paths in", &f, e))? {
                fixed += 1;
            }
        }
    }
    Ok(fixed)
}

/// A path as tools may have written it: as given, with `/`, and with `\`.
fn spellings(p: &Path) -> [String; 3] {
    let s = p.to_string_lossy().into_owned();
    [s.clone(), s.replace('\\', "/"), s.replace('/', "\\")]
}

fn rewrite(file: &Path, old: &[String; 3], new: &[String; 3]) -> io::Result<bool> {
    let meta = fs::metadata(file)?;
    if meta.len() > 1_000_000 {
        return Ok(false);
    }
    let bytes = fs::read(file)?;
    if bytes.contains(&0) {
        return Ok(false);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(false);
    };
    let mut out = text.clone();
    for (o, n) in old.iter().zip(new) {
        out = out.replace(o.as_str(), n);
    }
    if out == text {
        return Ok(false);
    }
    let tmp = file.with_extension("lane-tmp");
    fs::write(&tmp, out)?;
    fs::set_permissions(&tmp, meta.permissions())?;
    fs::rename(&tmp, file)?;
    Ok(true)
}

#[cfg(test)]
mod tests;
