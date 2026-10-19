//! Read-only checks of this machine and folder: OS, whether files can be cloned here, Git, the repo,
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
    /// A guided way to clear a `!` or `✗`, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

/// How to clear a problem, step by step. Lanes never runs `command` itself: the user runs it, or
/// copies it, and then checks again. `open` is a settings page on this machine, opened only when
/// the user asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub title: String,
    pub steps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<Settings>,
}

/// A page in the system's own settings app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub label: &'static str,
    pub uri: &'static str,
}

impl Fix {
    /// How many steps come before the command: it goes after the first step that ends in a colon
    /// ("Run this in another terminal window:"), or after all of them.
    pub fn command_after(&self) -> usize {
        self.steps
            .iter()
            .position(|s| s.ends_with(':'))
            .map_or(self.steps.len(), |i| i + 1)
    }

    fn new(title: impl Into<String>) -> Fix {
        Fix {
            title: title.into(),
            steps: Vec::new(),
            command: None,
            open: None,
        }
    }
    fn step(mut self, step: impl Into<String>) -> Fix {
        self.steps.push(step.into());
        self
    }
    fn command(mut self, command: Option<String>) -> Fix {
        self.command = command;
        self
    }
    fn open(mut self, label: &'static str, uri: &'static str) -> Fix {
        self.open = Some(Settings { label, uri });
        self
    }
}

impl Check {
    pub(crate) fn new(
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
            fix: None,
        }
    }
    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Check {
        self.hint = Some(hint.into());
        self
    }
    fn fix(mut self, fix: Option<Fix>) -> Check {
        self.fix = fix;
        self
    }
}

/// Whether `cmd` is a program on `PATH`.
fn on_path(cmd: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|d| d.join(cmd).is_file() || (cfg!(windows) && d.join(format!("{cmd}.exe")).is_file()))
}

/// `s`, quoted for a shell when it has spaces.
fn quoted(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
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

// ---------- cloned or copied lanes ----------

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
        "apfs" => (CopySupport::Instant, "APFS can clone files"),
        "btrfs" => (CopySupport::Instant, "Btrfs can clone files"),
        "xfs" => (CopySupport::Instant, "XFS can clone files"),
        "refs" => (CopySupport::Instant, "ReFS (Dev Drive) can clone files"),
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
        "ntfs" => {
            "Lanes still works, by copying. On a Windows 11 Dev Drive, lanes are cloned instead."
        }
        "9p" | "drvfs" => "Keep the repo inside the WSL filesystem (under ~), not on /mnt/c.",
        "ext4" | "ext3" | "ext2" => {
            "Lanes still works, by copying. On a Btrfs or XFS disk, lanes are cloned instead."
        }
        _ => "Lanes still works, using plain copies.",
    }
}

/// The repo a fix moves: its folder, its name and where it was cloned from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub top: PathBuf,
    pub name: String,
    pub origin: Option<String>,
}

fn place(dir: &Path) -> Option<Place> {
    let (top, _) = repo_info(dir)?;
    let name = top.file_name()?.to_string_lossy().into_owned();
    let origin = git(Some(dir), &["remote", "get-url", "origin"]).filter(|o| !o.is_empty());
    Some(Place { top, name, origin })
}

/// How to get the repo at `place` into `dest`: a fresh clone when it has a remote (the copy here
/// stays as it is), otherwise a plain copy of the folder.
fn move_command(place: &Place, dest: &str, windows: bool) -> String {
    match &place.origin {
        Some(origin) => format!("git clone {origin} {}", quoted(dest)),
        None if windows => format!(
            "robocopy {} {} /E",
            quoted(&place.top.display().to_string()),
            quoted(dest)
        ),
        None => format!(
            "cp -r {} {}",
            quoted(&place.top.display().to_string()),
            quoted(dest)
        ),
    }
}

/// The guided fix for a filesystem that can't clone files. `instant` lists the volumes on this
/// machine that can (Dev Drives on Windows, Btrfs or XFS mounts on Linux).
pub fn copies_fix(
    fs: &str,
    place: Option<&Place>,
    instant: &[String],
    free: Option<u64>,
) -> Option<Fix> {
    let fs = fs.to_ascii_lowercase();
    let name = place.map_or("your-repo", |p| p.name.as_str());
    match fs.as_str() {
        "ntfs" => Some(match instant.first() {
            Some(drive) => {
                let dest = format!("{drive}src\\{name}");
                Fix::new("Move this repo to your Dev Drive")
                    .step(format!(
                        "You already have a Dev Drive at {drive}, where lanes are cloned instead of copied."
                    ))
                    .step(match place {
                        Some(p) if p.origin.is_some() => {
                            "Clone the repo there. The copy here stays as it is:"
                        }
                        Some(_) => "Copy the repo there. The copy here stays as it is:",
                        None => "Clone or copy your repo there, then work from that folder.",
                    })
                    .command(place.map(|p| move_command(p, &dest, true)))
                    .step(format!(
                        "Open a terminal in {dest} and run `lane` from there."
                    ))
            }
            None => {
                let room = match free {
                    Some(f) if f >= 50_000_000_000 => format!(
                        "Choose Create Dev Drive, then a new virtual disk of 50 GB or more. This disk has {} free.",
                        human_bytes(f)
                    ),
                    Some(f) => format!(
                        "Choose Create Dev Drive. It needs 50 GB and this disk has only {} free, so free up space or pick another disk.",
                        human_bytes(f)
                    ),
                    None => {
                        "Choose Create Dev Drive, then a new virtual disk of 50 GB or more."
                            .to_string()
                    }
                };
                Fix::new("Clone lanes on a Dev Drive")
                    .step("Optional: Lanes already works here. On a Dev Drive, new lanes are cloned instead of copied, so they share your files' data. It needs Windows 11 22H2 or later.")
                    .step("Open Settings › System › Storage › Disks & volumes.")
                    .step(room)
                    .step(format!(
                        "Clone or copy {name} onto the new drive, then run `lane` there. Check again here and Lanes finds the drive."
                    ))
                    .open("Disks & volumes", "ms-settings:disksandvolumes")
            }
        }),
        "9p" | "drvfs" => {
            let dest = format!("~/src/{name}");
            Some(
                Fix::new("Move the repo into WSL")
                    .step("Folders under /mnt/c live on Windows. WSL reads them slowly and can't clone them.")
                    .step(match place {
                        Some(_) => "Put the repo in your WSL home instead. The copy on Windows stays as it is:",
                        None => "Clone your repos into your WSL home (under ~) instead.",
                    })
                    .command(place.map(|p| move_command(p, &dest, false)))
                    .step(format!("Then `cd {dest}` and run `lane` from there.")),
            )
        }
        "ext4" | "ext3" | "ext2" => Some(match instant.first() {
            Some(mount) => {
                let dest = format!("{}/src/{name}", mount.trim_end_matches('/'));
                Fix::new(format!("Keep repos on {mount}"))
                    .step(format!("{mount} can clone files, so lanes there are cloned instead of copied."))
                    .step(match place {
                        Some(_) => "Put the repo there. The copy here stays as it is:",
                        None => "Clone your repos there.",
                    })
                    .command(place.map(|p| move_command(p, &dest, false)))
                    .step(format!("Then `cd {dest}` and run `lane` from there."))
            }
            None => Fix::new("Cloned lanes on Linux")
                .step("Optional: Lanes works here with plain copies.")
                .step("On a Btrfs or XFS disk, lanes are cloned instead of copied. If you add one, keep your repos there and check again."),
        }),
        _ => None,
    }
}

fn copies_check(dir: &Path) -> Check {
    match filesystem_name(dir) {
        Some(name) => {
            let (support, why) = classify_filesystem(&name);
            match support {
                CopySupport::Instant => {
                    Check::new("copies", "Cloned lanes", Status::Ok, why.to_string())
                }
                CopySupport::Fallback => {
                    let fix = copies_fix(
                        &name,
                        place(dir).as_ref(),
                        &instant_volumes(),
                        free_bytes(dir),
                    );
                    Check::new("copies", "Cloned lanes", Status::Warn, why.to_string())
                        .hint(copies_hint(&name))
                        .fix(fix)
                }
            }
        }
        None => Check::new(
            "copies",
            "Cloned lanes",
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

/// Volumes on this machine where lanes are cloned: Dev Drives (`D:\`) on Windows, Btrfs and XFS
/// mount points on Linux.
pub fn instant_volumes() -> Vec<String> {
    platform::instant_volumes()
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
        // f_bavail is already u64 on macOS; the cast keeps this the same as the Linux code.
        #[allow(clippy::unnecessary_cast)]
        Some(st.f_bavail as u64 * st.f_bsize as u64)
    }

    pub fn instant_volumes() -> Vec<String> {
        // Every Mac volume is APFS already.
        Vec::new()
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

    pub fn instant_volumes() -> Vec<String> {
        std::fs::read_to_string("/proc/mounts")
            .map(|m| super::instant_mounts(&m))
            .unwrap_or_default()
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        GetVolumePathNameW,
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
        volume_fs(&volume_root(dir)?)
    }

    /// The filesystem of the volume at `root`, a NUL-terminated path like `C:\`.
    fn volume_fs(root: &[u16]) -> Option<String> {
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

    pub fn instant_volumes() -> Vec<String> {
        const DRIVE_FIXED: u32 = 3;
        // SAFETY: no arguments; returns a bitmask of drive letters.
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .filter_map(|i| {
                let root = format!("{}:\\", (b'A' + i) as char);
                let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
                // SAFETY: `w` is NUL-terminated.
                if unsafe { GetDriveTypeW(w.as_ptr()) } != DRIVE_FIXED {
                    return None;
                }
                (volume_fs(&w)? == "refs").then_some(root)
            })
            .collect()
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
    pub fn instant_volumes() -> Vec<String> {
        Vec::new()
    }
}

/// Btrfs and XFS mount points in a `/proc/mounts` listing, leaving out the system's own.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn instant_mounts(mounts: &str) -> Vec<String> {
    const SYSTEM: [&str; 7] = ["/boot", "/snap", "/proc", "/sys", "/run", "/var", "/tmp"];
    mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_, at, fs) = (f.next()?, f.next()?, f.next()?);
            let at = at.replace("\\040", " ");
            let system = at == "/"
                || SYSTEM
                    .iter()
                    .any(|s| at == *s || at.starts_with(&format!("{s}/")));
            (matches!(fs, "btrfs" | "xfs") && !system).then_some(at)
        })
        .collect()
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

/// The command that installs (or, with `update`, updates) Git on `os`, using the package
/// managers `has` finds.
pub fn git_install_command(os: &str, update: bool, has: impl Fn(&str) -> bool) -> Option<String> {
    let cmd = match os {
        "windows" if has("winget") => {
            if update {
                "winget upgrade --id Git.Git -e --source winget"
            } else {
                "winget install --id Git.Git -e --source winget"
            }
        }
        "macos" if has("brew") => {
            if update {
                "brew upgrade git"
            } else {
                "brew install git"
            }
        }
        "macos" if !update => "xcode-select --install",
        "linux" => {
            [
                ("apt", "sudo apt install git"),
                ("dnf", "sudo dnf install git"),
                ("pacman", "sudo pacman -S git"),
                ("zypper", "sudo zypper install git"),
                ("apk", "sudo apk add git"),
            ]
            .into_iter()
            .find(|(pm, _)| has(pm))?
            .1
        }
        _ => return None,
    };
    Some(cmd.to_string())
}

/// The guided fix for a missing or old Git.
pub fn git_fix(os: &str, update: bool, has: impl Fn(&str) -> bool) -> Fix {
    let command = git_install_command(os, update, has);
    let fix = Fix::new(if update { "Update Git" } else { "Install Git" });
    let fix = match &command {
        Some(_) => fix
            .step("Run this in another terminal window:")
            .command(command)
            .step("Or download the installer from git-scm.com."),
        None => fix.step("Install Git with your system's package manager, or from git-scm.com."),
    };
    if os == "windows" {
        fix.step("Then open a new terminal, so it can find Git.")
    } else {
        fix
    }
}

/// Git installed in its usual place on Windows, but not on this terminal's `PATH` yet, as happens
/// right after installing it.
fn git_off_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let machine = std::env::var_os("ProgramFiles").map(|b| PathBuf::from(b).join("Git"));
    let user =
        std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join("Programs").join("Git"));
    [machine, user]
        .into_iter()
        .flatten()
        .map(|git| git.join("cmd").join("git.exe"))
        .find(|p| p.is_file())
}

fn git_check() -> Check {
    let os = std::env::consts::OS;
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
                .fix(Some(git_fix(os, true, on_path)))
        }
        None => match git_off_path() {
            Some(path) => Check::new(
                "git",
                "Git",
                Status::Warn,
                "Git is installed, but this terminal can't see it yet",
            )
            .hint("Open a new terminal, then run `lane` again.")
            .fix(Some(
                Fix::new("Open a new terminal")
                    .step(format!(
                        "Git is at {}, but this terminal started before Git was added to PATH.",
                        path.display()
                    ))
                    .step("Close this terminal, open a new one, and run `lane` again."),
            )),
            None => Check::new(
                "git",
                "Git",
                Status::Fail,
                "Git isn't installed or isn't on PATH",
            )
            .hint("Install Git, then check again.")
            .fix(Some(git_fix(os, false, on_path))),
        },
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

/// The guided fix for a disk that's nearly full.
pub fn disk_fix(os: &str) -> Fix {
    let fix = Fix::new("Free up some space");
    let fix = match os {
        "windows" => fix
            .step("Storage Sense clears temporary files and the Recycle Bin for you.")
            .open("Storage Sense", "ms-settings:storagesense"),
        "macos" => fix
            .step("Storage settings show what's using space and what's safe to remove.")
            .open(
                "Storage settings",
                "x-apple.systempreferences:com.apple.settings.Storage",
            ),
        _ => fix
            .step("See the biggest folders in your home:")
            .command(Some("du -sh ~/* 2>/dev/null | sort -h | tail -n 10".into())),
    };
    fix.step("Old build folders (target, node_modules, .venv) are often the biggest. Delete the ones you don't need.")
}

fn disk_check(dir: &Path) -> Check {
    match free_bytes(dir) {
        Some(free) if free >= 10_000_000_000 => Check::new("disk", "Free disk", Status::Ok, format!("{} free", human_bytes(free))),
        Some(free) => Check::new("disk", "Free disk", Status::Warn, format!("only {} free", human_bytes(free)))
            .hint("Lanes share disk blocks, but builds and databases still need room. Free up some space if you can.")
            .fix(Some(disk_fix(std::env::consts::OS))),
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

    fn repo(origin: Option<&str>) -> Place {
        Place {
            top: PathBuf::from(r"C:\Users\me\code\acme"),
            name: "acme".into(),
            origin: origin.map(Into::into),
        }
    }

    #[test]
    fn ntfs_fix_offers_an_existing_dev_drive() {
        let p = repo(Some("https://github.com/me/acme"));
        let fix = copies_fix("NTFS", Some(&p), &[r"D:\".into()], Some(200_000_000_000)).unwrap();
        assert_eq!(fix.title, "Move this repo to your Dev Drive");
        assert_eq!(
            fix.command.as_deref(),
            Some(r"git clone https://github.com/me/acme D:\src\acme")
        );
        assert!(fix.open.is_none());
    }

    #[test]
    fn ntfs_fix_without_a_dev_drive_opens_settings() {
        let fix = copies_fix("ntfs", None, &[], Some(225_000_000_000)).unwrap();
        assert_eq!(fix.title, "Clone lanes on a Dev Drive");
        assert_eq!(fix.open.unwrap().uri, "ms-settings:disksandvolumes");
        assert!(fix.steps.iter().any(|s| s.contains("225 GB free")));
        let tight = copies_fix("ntfs", None, &[], Some(30_000_000_000)).unwrap();
        assert!(tight.steps.iter().any(|s| s.contains("only 30.0 GB")));
    }

    #[test]
    fn repos_without_a_remote_are_copied() {
        let p = repo(None);
        let fix = copies_fix("ntfs", Some(&p), &[r"E:\".into()], None).unwrap();
        assert_eq!(
            fix.command.as_deref(),
            Some(r"robocopy C:\Users\me\code\acme E:\src\acme /E")
        );
        let wsl = copies_fix("9p", Some(&p), &[], None).unwrap();
        assert!(wsl.command.unwrap().starts_with("cp -r "));
    }

    #[test]
    fn fast_filesystems_need_no_fix() {
        assert!(copies_fix("apfs", None, &[], None).is_none());
        assert!(
            copies_fix("ext4", None, &["/data".into()], None)
                .unwrap()
                .title
                == "Keep repos on /data"
        );
    }

    #[test]
    fn mounts_that_clone_are_found() {
        let mounts = "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /data btrfs rw 0 0\n/dev/sdc1 /boot/efi xfs rw 0 0\n/dev/sdd1 /mnt/my\\040disk xfs rw 0 0\n";
        assert_eq!(instant_mounts(mounts), ["/data", "/mnt/my disk"]);
    }

    #[test]
    fn git_install_commands_fit_the_machine() {
        let only = |pm: &'static str| move |c: &str| c == pm;
        assert_eq!(
            git_install_command("windows", false, only("winget")).as_deref(),
            Some("winget install --id Git.Git -e --source winget")
        );
        assert_eq!(
            git_install_command("macos", false, only("none")).as_deref(),
            Some("xcode-select --install")
        );
        assert_eq!(
            git_install_command("macos", true, only("brew")).as_deref(),
            Some("brew upgrade git")
        );
        assert_eq!(
            git_install_command("linux", false, only("dnf")).as_deref(),
            Some("sudo dnf install git")
        );
        assert_eq!(git_install_command("linux", false, only("none")), None);
        let fix = git_fix("linux", false, only("none"));
        assert!(fix.command.is_none() && fix.steps[0].contains("git-scm.com"));
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
