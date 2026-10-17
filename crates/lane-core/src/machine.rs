//! Read-only checks of this machine and folder: OS, whether instant copies work here, Git, the repo,
//! the stack and the agents on `PATH`. Used by the onboarding, `lane setup --json` and later
//! `lane doctor` and `lane init`. Nothing here writes a file or touches the network.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Good to go.
    Ok,
    /// Neutral information.
    Info,
    /// Works, with a catch worth knowing.
    Warn,
    /// Needs fixing before Lanes can work.
    Fail,
}

impl Status {
    /// A symbol that carries the meaning without colour.
    pub fn symbol(self) -> &'static str {
        match self {
            Status::Ok => "✓",
            Status::Info => "·",
            Status::Warn => "!",
            Status::Fail => "✗",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub label: &'static str,
    pub status: Status,
    pub summary: String,
    /// What to do next, for anything that isn't a plain pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    fn new(
        id: &'static str,
        label: &'static str,
        status: Status,
        summary: impl Into<String>,
    ) -> Check {
        Check {
            id,
            label,
            status,
            summary: summary.into(),
            hint: None,
        }
    }
    fn hint(mut self, hint: impl Into<String>) -> Check {
        self.hint = Some(hint.into());
        self
    }
}

/// Every check, in the order the onboarding shows them.
pub const CHECK_IDS: [&str; 7] = ["os", "copies", "git", "repo", "stack", "agents", "disk"];

/// Run one check by id against `dir`. Each check is independent, so callers can run them in parallel.
pub fn run_check(id: &str, dir: &Path) -> Check {
    match id {
        "os" => os_check(),
        "copies" => copies_check(dir),
        "git" => git_check(),
        "repo" => repo_check(dir),
        "stack" => stack_check(dir),
        "agents" => agents_check(std::env::var_os("PATH").as_deref()),
        "disk" => disk_check(dir),
        other => Check::new(
            "unknown",
            "Unknown",
            Status::Info,
            format!("no check called {other}"),
        ),
    }
}

/// Run every check, one after another.
pub fn run_all(dir: &Path) -> Vec<Check> {
    CHECK_IDS.iter().map(|id| run_check(id, dir)).collect()
}

// ---------- OS ----------

pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" if is_wsl() => "Linux (WSL2)",
        "linux" => "Linux",
        other => other,
    }
}

fn is_wsl() -> bool {
    cfg!(target_os = "linux")
        && std::fs::read_to_string("/proc/version")
            .is_ok_and(|v| v.to_ascii_lowercase().contains("microsoft"))
}

fn os_check() -> Check {
    let os = os_name();
    let supported = matches!(std::env::consts::OS, "macos" | "windows" | "linux");
    let check = Check::new(
        "os",
        "System",
        if supported { Status::Ok } else { Status::Warn },
        format!("{os} · {}", std::env::consts::ARCH),
    );
    if supported {
        check
    } else {
        check.hint("Lanes supports macOS, Linux and Windows. Other systems may work, untested.")
    }
}

// ---------- instant copies ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopySupport {
    /// Copy-on-write clones: a lane costs almost no disk and appears in about a second.
    Instant,
    /// Works, with plain or linked copies: slower and uses more disk.
    Fallback,
}

/// What Lanes can do on a filesystem, with the reason in words.
pub fn classify_filesystem(name: &str) -> (CopySupport, &'static str) {
    match name.to_ascii_lowercase().as_str() {
        "apfs" => (CopySupport::Instant, "APFS clones files instantly"),
        "btrfs" => (CopySupport::Instant, "Btrfs clones files instantly"),
        "xfs" => (CopySupport::Instant, "XFS clones files instantly"),
        "refs" => (
            CopySupport::Instant,
            "ReFS (Dev Drive) clones files instantly",
        ),
        "ntfs" => (
            CopySupport::Fallback,
            "NTFS can't clone files, so lanes are copied",
        ),
        "ext4" | "ext3" | "ext2" => (
            CopySupport::Fallback,
            "ext4 can't clone files, so lanes are copied",
        ),
        "9p" | "drvfs" => (
            CopySupport::Fallback,
            "this is a Windows drive seen from WSL, which is slow",
        ),
        "zfs" => (
            CopySupport::Fallback,
            "ZFS block cloning isn't used yet, so lanes are copied",
        ),
        "hfs" => (
            CopySupport::Fallback,
            "HFS+ can't clone files, so lanes are copied",
        ),
        _ => (
            CopySupport::Fallback,
            "this filesystem can't clone files as far as we know",
        ),
    }
}

fn copies_hint(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "ntfs" => "Lanes still works. For instant lanes, keep the repo on a Windows 11 Dev Drive.",
        "9p" | "drvfs" => "Keep the repo inside the WSL filesystem (under ~), not on /mnt/c.",
        "ext4" | "ext3" | "ext2" => "Lanes still works. A Btrfs or XFS disk makes lanes instant.",
        _ => "Lanes still works, using plain copies.",
    }
}

fn copies_check(dir: &Path) -> Check {
    match filesystem_name(dir) {
        Some(name) => {
            let (support, why) = classify_filesystem(&name);
            match support {
                CopySupport::Instant => {
                    Check::new("copies", "Instant lanes", Status::Ok, why.to_string())
                }
                CopySupport::Fallback => {
                    Check::new("copies", "Instant lanes", Status::Warn, why.to_string())
                        .hint(copies_hint(&name))
                }
            }
        }
        None => Check::new(
            "copies",
            "Instant lanes",
            Status::Info,
            "couldn't tell which filesystem this folder is on",
        )
        .hint("Lanes will test it the first time you make a lane."),
    }
}

/// The filesystem `dir` lives on, as a short lowercase name.
pub fn filesystem_name(dir: &Path) -> Option<String> {
    platform::filesystem_name(dir)
}

/// Free space on the volume holding `dir`, in bytes.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    platform::free_bytes(dir)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{CStr, CString};
    use std::path::Path;

    fn statfs(dir: &Path) -> Option<libc::statfs> {
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized out-parameter.
        (unsafe { libc::statfs(c.as_ptr(), &mut st) } == 0).then_some(st)
    }

    pub fn filesystem_name(dir: &Path) -> Option<String> {
        let st = statfs(dir)?;
        // SAFETY: the kernel fills f_fstypename with a NUL-terminated string.
        let name = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
        Some(name.to_string_lossy().to_ascii_lowercase())
    }

    pub fn free_bytes(dir: &Path) -> Option<u64> {
        let st = statfs(dir)?;
        Some(st.f_bavail as u64 * st.f_bsize as u64)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use std::ffi::CString;
    use std::path::Path;

    fn c_path(dir: &Path) -> Option<CString> {
        use std::os::unix::ffi::OsStrExt;
        CString::new(dir.as_os_str().as_bytes()).ok()
    }

    pub fn filesystem_name(dir: &Path) -> Option<String> {
        let c = c_path(dir)?;
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized out-parameter.
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        #[allow(clippy::unnecessary_cast)]
        let name = match st.f_type as u64 {
            0x9123_683E => "btrfs",
            0x5846_5342 => "xfs",
            0xEF53 => "ext4",
            0x0102_1994 => "tmpfs",
            0x794C_7630 => "overlayfs",
            0x2FC1_2FC1 => "zfs",
            0x0102_1997 => "9p",
            0x5346_544E | 0x7366_746E => "ntfs",
            0x6969 => "nfs",
            _ => "unknown",
        };
        Some(name.to_string())
    }

    pub fn free_bytes(dir: &Path) -> Option<u64> {
        let c = c_path(dir)?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        #[allow(clippy::unnecessary_cast)]
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetVolumeInformationW, GetVolumePathNameW,
    };

    fn wide(dir: &Path) -> Vec<u16> {
        dir.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn volume_root(dir: &Path) -> Option<Vec<u16>> {
        let path = wide(dir);
        let mut root = vec![0u16; 1024];
        // SAFETY: `path` is NUL-terminated and `root` has the length we pass.
        let ok = unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
        (ok != 0).then_some(root)
    }

    pub fn filesystem_name(dir: &Path) -> Option<String> {
        let root = volume_root(dir)?;
        let mut name = vec![0u16; 64];
        // SAFETY: `root` is NUL-terminated; unused out-parameters are null; `name` has the length we pass.
        let ok = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..end]).to_ascii_lowercase())
    }

    pub fn free_bytes(dir: &Path) -> Option<u64> {
        let root = volume_root(dir)?;
        let mut free: u64 = 0;
        // SAFETY: `root` is NUL-terminated; `free` is a valid out-parameter; the others may be null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                root.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::path::Path;
    pub fn filesystem_name(_: &Path) -> Option<String> {
        None
    }
    pub fn free_bytes(_: &Path) -> Option<u64> {
        None
    }
}

// ---------- Git ----------

/// "git version 2.51.0.windows.1" → (2, 51, "2.51.0").
pub fn parse_git_version(out: &str) -> Option<(u32, u32, String)> {
    let v = out
        .trim()
        .strip_prefix("git version ")?
        .split_whitespace()
        .next()?;
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()
        .filter(|p| p.chars().all(|c| c.is_ascii_digit()));
    let shown = match patch {
        Some(p) => format!("{major}.{minor}.{p}"),
        None => format!("{major}.{minor}"),
    };
    Some((major, minor, shown))
}

fn git(dir: Option<&Path>, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    let out = cmd.args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_check() -> Check {
    match git(None, &["--version"])
        .as_deref()
        .and_then(parse_git_version)
    {
        Some((major, minor, shown)) if (major, minor) >= (2, 30) => {
            Check::new("git", "Git", Status::Ok, format!("git {shown}"))
        }
        Some((_, _, shown)) => {
            Check::new("git", "Git", Status::Warn, format!("git {shown} is old"))
                .hint("Update Git to 2.30 or later for the best results.")
        }
        None => Check::new(
            "git",
            "Git",
            Status::Fail,
            "Git isn't installed or isn't on PATH",
        )
        .hint("Install Git from git-scm.com, then run `lane setup` again."),
    }
}

/// The repo `dir` is in: (top folder name, current branch).
pub fn repo_info(dir: &Path) -> Option<(PathBuf, Option<String>)> {
    let top = git(Some(dir), &["rev-parse", "--show-toplevel"])?;
    let branch = git(Some(dir), &["branch", "--show-current"]).filter(|b| !b.is_empty());
    Some((PathBuf::from(top), branch))
}

fn repo_check(dir: &Path) -> Check {
    match repo_info(dir) {
        Some((top, branch)) => {
            let name = top
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| top.display().to_string());
            let on = branch.map(|b| format!(" on {b}")).unwrap_or_default();
            Check::new(
                "repo",
                "This folder",
                Status::Ok,
                format!("the Git repo {name}{on}"),
            )
        }
        None => Check::new("repo", "This folder", Status::Info, "not inside a Git repo")
            .hint("That's fine for now. Run `lane init` inside a project when you're ready."),
    }
}

// ---------- stack ----------

/// What a folder's files say about its stack, in the order people would name it.
pub fn detect_stack(files: &[String], cargo_toml: Option<&str>) -> Vec<&'static str> {
    let has = |name: &str| files.iter().any(|f| f.eq_ignore_ascii_case(name));
    let mut stack = Vec::new();
    if has("pnpm-lock.yaml") {
        stack.push("pnpm");
    } else if has("bun.lock") || has("bun.lockb") {
        stack.push("Bun");
    } else if has("yarn.lock") {
        stack.push("Yarn");
    } else if has("package-lock.json") {
        stack.push("npm");
    } else if has("package.json") {
        stack.push("Node.js");
    }
    if has("turbo.json") {
        stack.push("Turborepo");
    }
    if has("nx.json") {
        stack.push("Nx");
    }
    if has("Cargo.toml") {
        stack.push(if cargo_toml.is_some_and(|t| t.contains("[workspace]")) {
            "Cargo workspace"
        } else {
            "Cargo"
        });
    }
    if has("uv.lock") {
        stack.push("uv");
    } else if has("poetry.lock") {
        stack.push("Poetry");
    } else if has("pyproject.toml") || has("requirements.txt") {
        stack.push("Python");
    }
    if has("go.mod") {
        stack.push("Go");
    }
    if has("Gemfile") {
        stack.push("Ruby (Bundler)");
    }
    if has("compose.yaml")
        || has("compose.yml")
        || has("docker-compose.yml")
        || has("docker-compose.yaml")
    {
        stack.push("Docker Compose");
    }
    if has(".env.example") {
        stack.push(".env.example");
    }
    stack
}

fn stack_check(dir: &Path) -> Check {
    let root = repo_info(dir)
        .map(|(top, _)| top)
        .unwrap_or_else(|| dir.to_path_buf());
    let files: Vec<String> = std::fs::read_dir(&root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).ok();
    let stack = detect_stack(&files, cargo.as_deref());
    if stack.is_empty() {
        Check::new(
            "stack",
            "Your stack",
            Status::Info,
            "nothing recognised in this folder",
        )
        .hint("`lane init` asks about your stack when it can't tell.")
    } else {
        Check::new("stack", "Your stack", Status::Ok, stack.join(", "))
    }
}

// ---------- agents ----------

/// Command names of agents Lanes routes into lanes, with how people call them.
pub const AGENTS: [(&str, &str); 8] = [
    ("claude", "Claude Code"),
    ("cursor-agent", "Cursor"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("copilot", "GitHub Copilot CLI"),
    ("opencode", "OpenCode"),
    ("amp", "Amp"),
    ("aider", "Aider"),
];

/// Agents whose command is in one of `dirs`, checked with `exists`.
pub fn find_agents(
    dirs: &[PathBuf],
    exts: &[String],
    exists: impl Fn(&Path) -> bool,
) -> Vec<&'static str> {
    AGENTS
        .iter()
        .filter(|(cmd, _)| {
            dirs.iter().any(|d| {
                exists(&d.join(cmd))
                    || exts
                        .iter()
                        .any(|ext| exists(&d.join(format!("{cmd}{ext}"))))
            })
        })
        .map(|(_, name)| *name)
        .collect()
}

fn agents_check(path: Option<&std::ffi::OsStr>) -> Check {
    let dirs: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };
    let found = find_agents(&dirs, &exts, |p| p.is_file());
    if found.is_empty() {
        Check::new(
            "agents",
            "Agents",
            Status::Info,
            "no coding agents found on PATH",
        )
        .hint("Lanes works with Claude Code, Cursor, Codex, Gemini CLI, Copilot and more.")
    } else {
        Check::new("agents", "Agents", Status::Ok, found.join(", "))
            .hint("Lanes routes these into lanes automatically after `lane init`.")
    }
}

// ---------- disk ----------

pub fn human_bytes(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let gb = bytes as f64 / GB;
    if gb >= 100.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}

fn disk_check(dir: &Path) -> Check {
    match free_bytes(dir) {
        Some(free) if free >= 10_000_000_000 => Check::new("disk", "Free disk", Status::Ok, format!("{} free", human_bytes(free))),
        Some(free) => Check::new("disk", "Free disk", Status::Warn, format!("only {} free", human_bytes(free)))
            .hint("Lanes share disk blocks, but builds and databases still need room. Free up some space if you can."),
        None => Check::new("disk", "Free disk", Status::Info, "couldn't read free space"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystems_are_classified() {
        assert_eq!(classify_filesystem("APFS").0, CopySupport::Instant);
        assert_eq!(classify_filesystem("btrfs").0, CopySupport::Instant);
        assert_eq!(classify_filesystem("ReFS").0, CopySupport::Instant);
        assert_eq!(classify_filesystem("NTFS").0, CopySupport::Fallback);
        assert_eq!(classify_filesystem("ext4").0, CopySupport::Fallback);
        assert_eq!(classify_filesystem("something").0, CopySupport::Fallback);
    }

    #[test]
    fn ntfs_hint_mentions_dev_drive() {
        assert!(copies_hint("ntfs").contains("Dev Drive"));
        assert!(copies_hint("9p").contains("WSL"));
    }

    #[test]
    fn git_versions_parse() {
        assert_eq!(
            parse_git_version("git version 2.51.0.windows.1"),
            Some((2, 51, "2.51.0".into()))
        );
        assert_eq!(
            parse_git_version("git version 2.39.5 (Apple Git-154)"),
            Some((2, 39, "2.39.5".into()))
        );
        assert_eq!(parse_git_version("nope"), None);
    }

    #[test]
    fn stack_is_detected() {
        let files = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            detect_stack(
                &files(&[
                    "pnpm-lock.yaml",
                    "package.json",
                    "turbo.json",
                    "compose.yaml"
                ]),
                None
            ),
            ["pnpm", "Turborepo", "Docker Compose"]
        );
        assert_eq!(
            detect_stack(&files(&["Cargo.toml"]), Some("[workspace]\nmembers = []")),
            ["Cargo workspace"]
        );
        assert_eq!(
            detect_stack(&files(&["pyproject.toml", "uv.lock"]), None),
            ["uv"]
        );
        assert!(detect_stack(&files(&["README.md"]), None).is_empty());
    }

    #[test]
    fn agents_are_found_on_a_fake_path() {
        let dirs = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let present = [
            PathBuf::from("/b").join("claude"),
            PathBuf::from("/a").join("codex.exe"),
        ];
        let found = find_agents(&dirs, &[".exe".into()], |p| present.iter().any(|q| q == p));
        assert_eq!(found, ["Claude Code", "Codex"]);
    }

    #[test]
    fn bytes_are_human() {
        assert_eq!(human_bytes(212_000_000_000), "212 GB");
        assert_eq!(human_bytes(8_400_000_000), "8.4 GB");
        assert_eq!(human_bytes(500_000_000), "500 MB");
    }

    #[test]
    fn real_checks_run_here() {
        // Smoke test on the build machine: every check returns, none panics.
        let checks = run_all(&std::env::current_dir().unwrap());
        assert_eq!(checks.len(), CHECK_IDS.len());
        assert!(checks.iter().all(|c| !c.summary.is_empty()));
    }
}
