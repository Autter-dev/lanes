//! Spotting shell commands that would make another copy of the repo: `git worktree add`, a
//! `git clone` of the repo itself, or a recursive copy of its folder. Reading a command is
//! string work alone, so the hook lets almost every command through before any Git or disk
//! work; only a command that might copy the repo is checked against the repo.

use super::{CLONE_DENIED, COPY_DENIED, WORKTREE_DENIED};
use crate::engine::same_path;
use std::cell::OnceCell;
use std::path::{Path, PathBuf};

/// A command that might copy the repo, as read from the command alone. `dir` is where that part
/// of the command runs, relative to the session's folder, after any `cd` before it; `None` when
/// that can't be told (after `cd -`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoCopy {
    /// `git worktree add`.
    Worktree,
    /// `git clone <from>` or `gh repo clone <from>`: a copy when `from` is this repo, by one of
    /// its folders or its `origin` URL.
    Clone { from: String, dir: Option<PathBuf> },
    /// `git clone` of a URL Git itself looks up, like `git clone "$(git remote get-url origin)"`.
    CloneRemote,
    /// A recursive copy of the folder `from` (`cp -r`, `xcopy /e`, `robocopy /e`,
    /// `Copy-Item -Recurse`): a copy when `from` is one of the repo's own folders.
    Folder { from: String, dir: Option<PathBuf> },
}

/// Words that run the command after them.
const WRAPPERS: [&str; 6] = ["sudo", "command", "exec", "env", "time", "nohup"];

/// Every part of a shell command that might copy the repo. `powershell` says the command is for
/// PowerShell, where `cp` is `Copy-Item`.
pub fn repo_copies(command: &str, powershell: bool) -> Vec<RepoCopy> {
    let lower = command.to_ascii_lowercase();
    if !["worktree", "clone", "cp", "copy"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return Vec::new();
    }
    let mut found = Vec::new();
    scan(command, powershell, &mut Some(PathBuf::new()), &mut found);
    found
}

/// Whether a shell command runs `git worktree add`, in any segment of it and with any of Git's
/// own options in between (`git -C ../app worktree add ...`).
pub fn makes_worktree(command: &str) -> bool {
    repo_copies(command, false).contains(&RepoCopy::Worktree)
}

fn scan(command: &str, powershell: bool, dir: &mut Option<PathBuf>, found: &mut Vec<RepoCopy>) {
    for segment in command.split(['\n', ';', '&', '|']) {
        let words = shell_words(segment);
        // The command word: past `NAME=value` assignments and wrappers like `sudo`.
        let Some(at) = words
            .iter()
            .position(|w| !w.contains('=') && !WRAPPERS.contains(&w.as_str()))
        else {
            continue;
        };
        let args = &words[at + 1..];
        let program = program(&words[at]);
        match program.as_str() {
            "git" => found.extend(git(args, dir.clone())),
            "gh" => found.extend(gh(args, dir)),
            "cd" | "chdir" | "pushd" | "set-location" | "sl" | "push-location" => {
                *dir = cd(dir.take(), args);
            }
            "popd" | "pop-location" => *dir = None,
            "cp" if !powershell => found.extend(cp(args, dir)),
            "cp" | "cpi" | "copy-item" => found.extend(copy_item(args, dir)),
            "copy" if powershell => found.extend(copy_item(args, dir)),
            "xcopy" => found.extend(windows_copy(args, dir, &["s", "e"])),
            "robocopy" => found.extend(windows_copy(args, dir, &["s", "e", "mir"])),
            "bash" | "sh" | "zsh" | "cmd" | "powershell" | "pwsh" => {
                if let Some(inner) = inner_command(&program, args) {
                    scan(
                        &inner,
                        matches!(program.as_str(), "powershell" | "pwsh"),
                        dir,
                        found,
                    );
                }
            }
            _ => {}
        }
    }
}

/// A program's name as typed, without its folder or `.exe`, in lowercase: `git`, `copy-item`.
fn program(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = name.to_ascii_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_string()
}

/// The command a shell is told to run: `bash -c "..."`, `cmd /c ...`, `pwsh -Command ...`.
fn inner_command(shell: &str, args: &[String]) -> Option<String> {
    let flag = args.iter().position(|w| {
        let w = w.to_ascii_lowercase();
        match shell {
            "cmd" => matches!(w.trim_start_matches('/'), "c" | "k") && w.starts_with('/'),
            "powershell" | "pwsh" => w.len() > 1 && "-command".starts_with(&w),
            _ => w.starts_with('-') && !w.starts_with("--") && w.ends_with('c'),
        }
    })?;
    let rest = &args[flag + 1..];
    (!rest.is_empty()).then(|| rest.join(" "))
}

/// Split a command into words the way a shell would, near enough: whitespace separates, and
/// single or double quotes keep a word together (`"C:\Program Files\Git\cmd\git.exe"`).
fn shell_words(s: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, '(' | ')') => {}
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// A path as written, with the shell's names for the current folder and the home folder spelled
/// `.` and `~`: `"$PWD"`, `$(pwd)` and `%CD%` are `.`; `$HOME` and `$env:USERPROFILE` are `~`.
fn path_word(w: &str) -> String {
    const NAMES: [(&str, &str); 10] = [
        ("$PWD", "."),
        ("${PWD}", "."),
        ("`pwd`", "."),
        ("%CD%", "."),
        ("$(Get-Location)", "."),
        ("$HOME", "~"),
        ("${HOME}", "~"),
        ("$env:HOME", "~"),
        ("$env:USERPROFILE", "~"),
        ("%USERPROFILE%", "~"),
    ];
    for (name, to) in NAMES {
        if let Some(head) = w.get(..name.len())
            && head.eq_ignore_ascii_case(name)
        {
            let rest = &w[name.len()..];
            if rest.is_empty() || rest.starts_with(['/', '\\']) {
                return format!("{to}{rest}");
            }
        }
    }
    w.to_string()
}

/// Whether a path names its folder without help from the current one: `/x`, `C:\x`, `~/x`.
fn rooted(p: &str) -> bool {
    p.starts_with(['/', '\\', '~']) || p.as_bytes().get(1) == Some(&b':')
}

/// Where a command runs after `cd <args>`.
fn cd(dir: Option<PathBuf>, args: &[String]) -> Option<PathBuf> {
    // Past options: `-P` (sh), `/d` (cmd), `-Path` (PowerShell).
    let to = args
        .iter()
        .find(|w| !(w.starts_with('-') && w.len() > 1) && !w.eq_ignore_ascii_case("/d"));
    match to.map(String::as_str) {
        None => Some(PathBuf::from("~")),
        Some("-") => None,
        Some(to) => go(dir, to),
    }
}

fn go(dir: Option<PathBuf>, to: &str) -> Option<PathBuf> {
    let to = path_word(to);
    if rooted(&to) {
        Some(PathBuf::from(to))
    } else {
        dir.map(|d| d.join(to))
    }
}

/// `git [-C <dir>] worktree add` or `git clone`.
fn git(args: &[String], mut dir: Option<PathBuf>) -> Option<RepoCopy> {
    let mut rest = args.iter();
    while let Some(w) = rest.next() {
        match w.as_str() {
            "-C" => dir = go(dir, rest.next()?),
            "-c" => {
                rest.next();
            }
            w if w.starts_with('-') => {}
            "worktree" => {
                let add = rest.find(|w| !w.starts_with('-')).map(String::as_str) == Some("add");
                return add.then_some(RepoCopy::Worktree);
            }
            "clone" => return clone(rest.as_slice(), dir),
            _ => return None,
        }
    }
    None
}

/// `git clone`'s options that take a value as the next word.
const CLONE_VALUES: [&str; 22] = [
    "-o",
    "-b",
    "-u",
    "-c",
    "-j",
    "--origin",
    "--branch",
    "--upload-pack",
    "--template",
    "--reference",
    "--reference-if-able",
    "--separate-git-dir",
    "--depth",
    "--shallow-since",
    "--shallow-exclude",
    "--config",
    "--server-option",
    "--jobs",
    "--filter",
    "--bundle-uri",
    "--revision",
    "--ref-format",
];

/// What `git clone <args>` clones. A bare or mirror clone has no files to work in, so it's left
/// alone.
fn clone(args: &[String], dir: Option<PathBuf>) -> Option<RepoCopy> {
    if args.iter().any(|w| w == "--bare" || w == "--mirror") {
        return None;
    }
    // `git clone "$(git remote get-url origin)"`, or PowerShell's `git clone (git config ...)`.
    if args
        .iter()
        .any(|w| w.contains("get-url") || (w.starts_with("remote.") && w.ends_with(".url")))
    {
        return Some(RepoCopy::CloneRemote);
    }
    let mut rest = args.iter();
    while let Some(w) = rest.next() {
        match w.as_str() {
            "--" => break,
            w if CLONE_VALUES.contains(&w) => {
                rest.next();
            }
            w if w.starts_with('-') => {}
            from => {
                return Some(RepoCopy::Clone {
                    from: path_word(from),
                    dir,
                });
            }
        }
    }
    rest.next().map(|from| RepoCopy::Clone {
        from: path_word(from),
        dir,
    })
}

/// `gh repo clone <owner/repo or URL>`.
fn gh(args: &[String], dir: &Option<PathBuf>) -> Option<RepoCopy> {
    if args.len() < 3 || args[0] != "repo" || args[1] != "clone" {
        return None;
    }
    let from = args[2..].iter().find(|w| !w.starts_with('-'))?;
    let short = from.matches('/').count() == 1 && !from.contains(':') && !rooted(from);
    let from = if short && !from.starts_with('.') {
        format!("https://github.com/{from}")
    } else {
        path_word(from)
    };
    Some(RepoCopy::Clone {
        from,
        dir: dir.clone(),
    })
}

/// A recursive copy of each of `sources`, unless it leaves `.git` out (`robocopy /XD .git`,
/// `Copy-Item -Exclude .git`): that's an export of the files, not another repo.
fn folders(
    recursive: bool,
    sources: &[&str],
    args: &[String],
    dir: &Option<PathBuf>,
) -> Vec<RepoCopy> {
    let names_git = args.iter().any(|w| {
        let w = w.trim_end_matches(['/', '\\']);
        w.rsplit(['/', '\\']).next() == Some(".git")
    });
    if !recursive || names_git {
        return Vec::new();
    }
    sources
        .iter()
        .flat_map(|s| s.split(','))
        .filter(|s| !s.is_empty())
        .map(|from| RepoCopy::Folder {
            from: path_word(from),
            dir: dir.clone(),
        })
        .collect()
}

/// `cp -r` (or `-R`, `-a`): every path but the last is a source, unless `-t` names the target.
fn cp(args: &[String], dir: &Option<PathBuf>) -> Vec<RepoCopy> {
    let (mut recursive, mut target, mut options) = (false, false, true);
    let mut paths = Vec::new();
    let mut rest = args.iter();
    while let Some(w) = rest.next() {
        match w.as_str() {
            "--" if options => options = false,
            w if options && w.starts_with("--") => {
                let (name, value) = w.split_once('=').unwrap_or((w, ""));
                recursive |= matches!(name, "--recursive" | "--archive");
                target |= name == "--target-directory";
                if value.is_empty() && matches!(name, "--target-directory" | "--suffix") {
                    rest.next();
                }
            }
            w if options && w.starts_with('-') && w.len() > 1 => {
                recursive |= w.contains(['r', 'R', 'a']);
                target |= w.ends_with('t');
                if w.ends_with(['t', 'S']) {
                    rest.next();
                }
            }
            _ => paths.push(w.as_str()),
        }
    }
    if !target {
        paths.pop();
    }
    folders(recursive, &paths, args, dir)
}

/// PowerShell's `Copy-Item` (`cp`, `copy`, `cpi`): the source is `-Path` or the first word
/// without a name. Parameter names can be cut short, so `-r` is `-Recurse`.
fn copy_item(args: &[String], dir: &Option<PathBuf>) -> Vec<RepoCopy> {
    const VALUES: [&str; 7] = [
        "destination",
        "filter",
        "include",
        "exclude",
        "credential",
        "fromsession",
        "tosession",
    ];
    let (mut recurse, mut paths, mut positional) = (false, Vec::new(), Vec::new());
    let mut rest = args.iter();
    while let Some(w) = rest.next() {
        let Some(param) = w.strip_prefix('-').filter(|p| !p.is_empty()) else {
            positional.push(w.as_str());
            continue;
        };
        let (name, value) = param.split_once(':').unwrap_or((param, ""));
        let name = name.to_ascii_lowercase();
        if "recurse".starts_with(&name) {
            recurse = !value.eq_ignore_ascii_case("$false");
        } else if name.len() > 1 && ("path".starts_with(&name) || "literalpath".starts_with(&name))
        {
            match value {
                "" => paths.extend(rest.next().map(String::as_str)),
                v => paths.push(v),
            }
        } else if VALUES.iter().any(|v| v.starts_with(&name)) && value.is_empty() {
            rest.next();
        }
    }
    if paths.is_empty() {
        paths.extend(positional.first());
    }
    folders(recurse, &paths, args, dir)
}

/// `xcopy` and `robocopy`: the source is the first word that isn't a `/switch`, and the copy
/// goes into subfolders only with one of `recursive` (`/E`, `/S`, `/MIR`).
fn windows_copy(args: &[String], dir: &Option<PathBuf>, recursive: &[&str]) -> Vec<RepoCopy> {
    let mut deep = false;
    let mut from = None;
    for w in args {
        match switch(w) {
            Some(s) => deep |= recursive.iter().any(|r| s.eq_ignore_ascii_case(r)),
            None if from.is_none() => from = Some(w.as_str()),
            None => {}
        }
    }
    folders(deep, &Vec::from_iter(from), args, dir)
}

/// A Windows switch's name, without its `:value`: `/E`, `/R:3`, or `//E` from Git Bash. A path
/// like `/c/Users/sam` isn't one.
fn switch(w: &str) -> Option<&str> {
    let name = w.strip_prefix('/')?;
    let name = name.strip_prefix('/').unwrap_or(name);
    let name = name.split(':').next().unwrap_or_default();
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic())).then_some(name)
}

/// Why a command was turned down, or `None` to let it run: it makes a worktree, clones this repo,
/// or copies one of its folders whole. `cwd` is where the session runs the command, `root` the
/// repo's main folder. Git is asked for the repo's folders and remote only when needed.
pub fn deny_reason(found: &[RepoCopy], cwd: &Path, root: &Path) -> Option<&'static str> {
    if found.contains(&RepoCopy::Worktree) {
        return Some(WORKTREE_DENIED);
    }
    let folders = OnceCell::new();
    let is_folder = |path: &Path| {
        folders
            .get_or_init(|| crate::git::worktrees(root).unwrap_or_else(|_| vec![root.into()]))
            .iter()
            .any(|f| same_path(f, path))
    };
    let origin = OnceCell::new();
    let is_origin = |key: &str| {
        origin
            .get_or_init(|| {
                crate::git::try_run(root, &["remote", "get-url", "origin"])
                    .and_then(|url| remote_key(&url))
            })
            .as_deref()
            == Some(key)
    };
    for f in found {
        match f {
            RepoCopy::CloneRemote => return Some(CLONE_DENIED),
            RepoCopy::Clone { from, dir } => {
                let this = match remote_key(from) {
                    Some(key) => is_origin(&key),
                    None => resolve(cwd, dir.as_deref(), &file_path(from)).is_some_and(|p| {
                        // `git clone ../app/.git` clones the repo too.
                        let p = match p.file_name() {
                            Some(n) if n == ".git" => {
                                p.parent().map(Path::to_path_buf).unwrap_or(p)
                            }
                            _ => p,
                        };
                        is_folder(&p)
                    }),
                };
                if this {
                    return Some(CLONE_DENIED);
                }
            }
            RepoCopy::Folder { from, dir } => {
                if resolve(cwd, dir.as_deref(), from).is_some_and(|p| is_folder(&p)) {
                    return Some(COPY_DENIED);
                }
            }
            RepoCopy::Worktree => {}
        }
    }
    None
}

/// The path in a `file://` URL, or the word as it is.
fn file_path(from: &str) -> String {
    let Some(rest) = from.strip_prefix("file://") else {
        return from.to_string();
    };
    // `file:///C:/work/app` on Windows.
    match rest.as_bytes() {
        [b'/', _, b':', ..] => rest[1..].to_string(),
        _ => rest.to_string(),
    }
}

/// A path from a command, made absolute: relative to `dir`, which is relative to `cwd`. `None`
/// when it's relative and `dir` can't be told.
fn resolve(cwd: &Path, dir: Option<&Path>, from: &str) -> Option<PathBuf> {
    let from = expand(from);
    let base = match dir {
        Some(d) => cwd.join(expand(&d.to_string_lossy())),
        None if from.has_root() => cwd.to_path_buf(),
        None => return None,
    };
    Some(base.join(from))
}

/// `~` as the home folder, and on Windows, Git Bash's `/c/Users` as `C:/Users`.
fn expand(p: &str) -> PathBuf {
    let p = match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            let home = if cfg!(windows) {
                std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"))
            } else {
                std::env::var("HOME")
            };
            match home {
                Ok(home) => format!("{home}{rest}"),
                Err(_) => p.to_string(),
            }
        }
        _ => p.to_string(),
    };
    if cfg!(windows)
        && let [b'/', drive, rest @ ..] = p.as_bytes()
        && drive.is_ascii_alphabetic()
        && (rest.is_empty() || rest[0] == b'/')
    {
        return PathBuf::from(format!("{}:/{}", *drive as char, p.get(3..).unwrap_or("")));
    }
    PathBuf::from(p)
}

/// A remote URL as `host/path` in lowercase, so the ways of writing one address match:
/// `https://github.com/acme/app.git`, `git@github.com:acme/app` and
/// `ssh://git@github.com:22/acme/app/`. `None` for a local path or a `file://` URL.
pub(super) fn remote_key(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let scheme_ok = !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
            if !scheme_ok || scheme.eq_ignore_ascii_case("file") {
                return None;
            }
            rest.split_once('/').unwrap_or((rest, ""))
        }
        // Git's short form, `[user@]host:path`. One letter before the colon is a Windows drive.
        None => {
            let (host, path) = url.split_once(':')?;
            if host.len() < 2 || host.contains(['/', '\\']) {
                return None;
            }
            (host, path)
        }
    };
    let host = host
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    (!host.is_empty() && !path.is_empty()).then(|| format!("{host}/{}", path.to_ascii_lowercase()))
}
