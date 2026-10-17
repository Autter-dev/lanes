//! The Git operations the lane engine needs, through the `git` command line. Every failure says
//! which command failed, with Git's own first line of complaint.

use crate::error::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    // Never ask a person anything, and never page. LANE_GIT tells Lanes' own post-checkout hook
    // that this worktree is Lanes' doing, so it isn't adopted twice.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LANE_GIT", "1");
    cmd
}

/// Run `git -C dir <args>` and return its trimmed stdout.
pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = git(dir).args(args).output().map_err(|_| {
        Error::new(ErrorKind::Failure, "Git isn't installed or isn't on PATH")
            .hint("Install Git, then run the command again. `lane setup` shows how.")
    })?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let why = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no message")
        .trim()
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ");
    Err(Error::new(
        ErrorKind::Failure,
        format!("`git {}` failed: {why}", args.join(" ")),
    )
    .hint("Check the repo with `git status`, then run the command again."))
}

/// Like [`run`], but `None` instead of an error.
pub fn try_run(dir: &Path, args: &[&str]) -> Option<String> {
    run(dir, args).ok()
}

fn absolute(dir: &Path, p: String) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() { p } else { dir.join(p) }
}

/// The repo's shared `.git` folder, the same for every worktree of one repo.
pub fn common_dir(dir: &Path) -> Result<PathBuf> {
    let p = run(dir, &["rev-parse", "--git-common-dir"])?;
    let p = absolute(dir, p);
    Ok(std::fs::canonicalize(&p).unwrap_or(p))
}

/// A path inside this worktree's Git folder, like its `index`.
pub fn git_path(dir: &Path, name: &str) -> Result<PathBuf> {
    Ok(absolute(dir, run(dir, &["rev-parse", "--git-path", name])?))
}

/// The commit `rev` names, if it names one.
pub fn commit_of(dir: &Path, rev: &str) -> Option<String> {
    try_run(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
}

/// The branch `_base` follows and its commit: the one set in `lane.toml`, else the remote's
/// default branch (preferring the local branch of the same name), else `main` or `master`,
/// else whatever is checked out.
pub fn base_branch(root: &Path, configured: Option<&str>) -> Result<(String, String)> {
    if let Some(b) = configured {
        return commit_of(root, b)
            .map(|c| (b.to_string(), c))
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Config,
                    format!(
                        "`lanes.base` in lane.toml names `{b}`, which isn't a branch or commit here"
                    ),
                )
                .hint(
                    "Set it to a branch that exists, like `main`, or remove it to use the default.",
                )
            });
    }
    let mut candidates = Vec::new();
    if let Some(remote) = try_run(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        if let Some(local) = remote.strip_prefix("origin/") {
            candidates.push(local.to_string());
        }
        candidates.push(remote);
    }
    candidates.extend(["main".to_string(), "master".to_string()]);
    for name in candidates {
        if let Some(c) = commit_of(root, &name) {
            return Ok((name, c));
        }
    }
    let c = commit_of(root, "HEAD").ok_or_else(|| {
        Error::new(ErrorKind::Failure, "this repo has no commits yet")
            .hint("Make a first commit, then run the command again.")
    })?;
    Ok(("HEAD".to_string(), c))
}

/// Add a worktree at `path` with a detached HEAD at `commit`. With `checkout = false`, no files
/// are written; the lane's files come from `_base` instead.
pub fn worktree_add(repo: &Path, path: &Path, commit: &str, checkout: bool) -> Result<()> {
    let path = path.to_string_lossy();
    let mut args = vec!["worktree", "add", "--detach", "--quiet"];
    if !checkout {
        args.push("--no-checkout");
    }
    args.extend([path.as_ref(), commit]);
    run(repo, &args).map(|_| ())
}

/// Remove the worktree at `path`, changes and all, and forget its record.
pub fn worktree_remove(repo: &Path, path: &Path) -> Result<()> {
    let p = path.to_string_lossy();
    run(repo, &["worktree", "remove", "--force", "--force", &p]).map(|_| ())
}

/// Forget records of worktrees whose folders are gone.
pub fn worktree_prune(repo: &Path) {
    let _ = run(repo, &["worktree", "prune"]);
}

/// Every worktree folder of the repo, the main one first.
pub fn worktrees(repo: &Path) -> Result<Vec<PathBuf>> {
    Ok(run(repo, &["worktree", "list", "--porcelain"])?
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect())
}

/// Move a worktree to `commit`, throwing away changes to tracked files. Untracked files (such
/// as installed dependencies) stay.
pub fn checkout_detached(dir: &Path, commit: &str) -> Result<()> {
    run(dir, &["checkout", "--detach", "--force", "--quiet", commit]).map(|_| ())
}

/// The blob ids of `paths` at `commit` (those that exist), as `path blob` lines.
pub fn blobs(dir: &Path, commit: &str, paths: &[&str]) -> Result<Vec<String>> {
    let mut args = vec!["ls-tree", commit, "--"];
    args.extend_from_slice(paths);
    Ok(run(dir, &args)?
        .lines()
        .filter_map(|l| {
            // "<mode> blob <id>\t<path>"
            let (meta, path) = l.split_once('\t')?;
            let id = meta.split_whitespace().nth(2)?;
            Some(format!("{path} {id}"))
        })
        .collect())
}

/// Bring the index's file stats up to date, so `git status` is fast and right in a copied lane.
pub fn refresh_index(dir: &Path) {
    let _ = git(dir).args(["update-index", "-q", "--refresh"]).output();
}
