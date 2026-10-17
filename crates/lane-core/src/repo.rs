//! Finding the repo a folder belongs to.

use crate::error::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The top-level folder of the Git repo `dir` is in, as Git reports it.
pub fn top_level(dir: &Path) -> Result<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|_| {
            Error::new(ErrorKind::Failure, "Git isn't installed or isn't on PATH")
                .hint("Install Git, then run the command again. `lane setup` shows how.")
        })?;
    if !out.status.success() {
        return Err(Error::new(
            ErrorKind::NotARepo,
            format!("{} isn't inside a Git repo", dir.display()),
        )
        .hint("Run it inside a repo, or add --all to see lanes in every repo."));
    }
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(PathBuf::from(top))
}
