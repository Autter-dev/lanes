//! Copying a folder tree into a lane, the fast way where the disk allows it.
//!
//! Every file is a copy-on-write clone where the filesystem supports one (APFS `clonefile`,
//! Btrfs and XFS `FICLONE`, ReFS block cloning on a Windows Dev Drive): the copy shares disk
//! blocks with the original until one side writes, so it's near-instant and nearly free on disk.
//! Elsewhere (NTFS, ext4 and the rest) the same walk does a plain parallel copy: slower, but
//! correct everywhere.
//!
//! [`probe`] tests a folder for real instead of trusting the filesystem's name, and
//! [`clone_tree`] reports how many files were cloned and how many copied, so Lanes never
//! claims a clone it didn't get.

use crate::error::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// How a lane's files get there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Copy-on-write clones: instant, sharing disk blocks.
    Clone,
    /// Plain copies: every byte written again.
    Copy,
}

/// One way of putting a file at a new path.
pub trait CloneBackend: Send + Sync {
    /// What this backend aims for.
    fn method(&self) -> Method;
    /// Put `src`'s contents at `dst` (a new file). Returns whether it was really cloned.
    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<Method>;
}

/// Copy-on-write clones, falling back to a copy for any one file the disk won't clone.
pub struct Reflink;

impl CloneBackend for Reflink {
    fn method(&self) -> Method {
        Method::Clone
    }
    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<Method> {
        match reflink_copy::reflink_or_copy(src, dst)? {
            None => Ok(Method::Clone),
            Some(_) => Ok(Method::Copy),
        }
    }
}

/// Plain copies, for disks that can't clone.
pub struct PlainCopy;

impl CloneBackend for PlainCopy {
    fn method(&self) -> Method {
        Method::Copy
    }
    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<Method> {
        fs::copy(src, dst).map(|_| Method::Copy)
    }
}

/// The backend for a method.
pub fn backend(method: Method) -> Box<dyn CloneBackend> {
    match method {
        Method::Clone => Box::new(Reflink),
        Method::Copy => Box::new(PlainCopy),
    }
}

/// Whether files in `dir` can be cloned, found by trying it. `dir` must exist.
pub fn probe(dir: &Path) -> Method {
    let base = dir.join(format!(".lane-probe-{}", std::process::id()));
    let (src, dst) = (base.with_extension("a"), base.with_extension("b"));
    let result = (|| -> io::Result<()> {
        let mut f = File::create(&src)?;
        // A few clusters of real data, so the filesystem has blocks to share.
        f.write_all(&[0x5a; 64 * 1024])?;
        f.sync_all()?;
        drop(f);
        reflink_copy::reflink(&src, &dst)
    })();
    let _ = fs::remove_file(&src);
    let _ = fs::remove_file(&dst);
    if result.is_ok() {
        Method::Clone
    } else {
        Method::Copy
    }
}

/// What a folder's disk can do, for messages and `lane doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detected {
    /// The filesystem's name, when known (`apfs`, `btrfs`, `refs`, `ntfs`, …).
    pub filesystem: Option<String>,
    /// What actually works there, tested.
    pub method: Method,
}

/// Detect what `dir`'s disk can do. `dir` must exist.
pub fn detect(dir: &Path) -> Detected {
    Detected {
        filesystem: crate::machine::filesystem_name(dir),
        method: probe(dir),
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Names skipped at the top of the tree (each lane gets its own `.git`, for example).
    pub skip: Vec<OsString>,
    /// Worker threads; 0 means one per core, up to 16.
    pub threads: usize,
    /// Copy modification times and permissions too (on by default; off only for measuring).
    pub keep_metadata: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            skip: vec![OsString::from(".git")],
            threads: 0,
            keep_metadata: true,
        }
    }
}

/// What a tree clone did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// `clone` only if every file was cloned.
    pub method: Method,
    pub files: usize,
    /// Files with content that were cloned.
    pub cloned: usize,
    /// Files with content that were copied.
    pub copied: usize,
    /// Empty files: nothing to clone or copy, so they count as neither.
    pub empty: usize,
    pub dirs: usize,
    pub symlinks: usize,
    pub bytes: u64,
    #[serde(serialize_with = "millis")]
    pub elapsed: Duration,
}

fn millis<S: serde::Serializer>(d: &Duration, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_millis() as u64)
}

struct FileJob {
    rel: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    permissions: fs::Permissions,
}

fn io_error(what: &str, path: &Path, e: io::Error) -> Error {
    let hint = if e.kind() == io::ErrorKind::StorageFull {
        "Free some disk space, then try again."
    } else {
        "Check that the disk has room and that you can write there, then try again."
    };
    Error::new(
        ErrorKind::Io,
        format!("couldn't {what} {}: {e}", path.display()),
    )
    .hint(hint)
}

/// Copy the tree at `src` into `dst` (which must not exist yet, or be empty), with `backend`,
/// in parallel. Folders, file contents, permissions and modification times come across;
/// symlinks are recreated as symlinks. If `dst` sits inside `src`, it's skipped.
pub fn clone_tree(
    src: &Path,
    dst: &Path,
    backend: &dyn CloneBackend,
    opts: &Options,
) -> Result<Report> {
    let started = Instant::now();
    // Skipped names may already be there (a lane's own `.git` file comes first).
    if let Ok(mut entries) = fs::read_dir(dst)
        && entries.any(|e| e.is_ok_and(|e| !opts.skip.contains(&e.file_name())))
    {
        return Err(Error::new(
            ErrorKind::Conflict,
            format!("{} already has files in it", dst.display()),
        )
        .hint("Pick another lane name, or remove that folder first."));
    }
    let src_real = fs::canonicalize(src).map_err(|e| io_error("read", src, e))?;
    let dst_real = dst
        .parent()
        .and_then(|p| fs::canonicalize(p).ok())
        .map(|p| p.join(dst.file_name().unwrap_or_default()));

    // Walk first, so folders exist before files are written and the work splits evenly.
    let mut dirs = vec![PathBuf::new()];
    let mut files = Vec::new();
    let mut links = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let here = src.join(&rel);
        let entries = fs::read_dir(&here).map_err(|e| io_error("read", &here, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_error("read", &here, e))?;
            let name = entry.file_name();
            if rel.as_os_str().is_empty() && opts.skip.contains(&name) {
                continue;
            }
            let child = rel.join(&name);
            let path = src.join(&child);
            let kind = entry.file_type().map_err(|e| io_error("read", &path, e))?;
            if kind.is_symlink() {
                links.push(child);
            } else if kind.is_dir() {
                if dst_real
                    .as_ref()
                    .is_some_and(|d| src_real.join(&child) == *d)
                {
                    continue;
                }
                dirs.push(child.clone());
                stack.push(child);
            } else {
                let meta = entry.metadata().map_err(|e| io_error("read", &path, e))?;
                files.push(FileJob {
                    rel: child,
                    len: meta.len(),
                    modified: meta.modified().ok(),
                    permissions: meta.permissions(),
                });
            }
        }
    }

    for d in &dirs {
        let path = dst.join(d);
        fs::create_dir_all(&path).map_err(|e| io_error("create", &path, e))?;
    }

    let next = AtomicUsize::new(0);
    let cloned = AtomicUsize::new(0);
    let empty = files.iter().filter(|f| f.len == 0).count();
    let bytes = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let threads = match opts.threads {
        0 => std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(16),
        n => n,
    }
    .min(files.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = files.get(i) else { break };
                    match copy_one(src, dst, job, backend, opts.keep_metadata) {
                        Ok(method) => {
                            if method == Some(Method::Clone) {
                                cloned.fetch_add(1, Ordering::Relaxed);
                            }
                            bytes.fetch_add(job.len, Ordering::Relaxed);
                        }
                        Err(e) => {
                            stop.store(true, Ordering::Relaxed);
                            failure.lock().unwrap().get_or_insert(e);
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }

    for rel in &links {
        copy_link(&src.join(rel), &dst.join(rel), src, dst)?;
    }

    let cloned = cloned.into_inner();
    let copied = files.len() - empty - cloned;
    Ok(Report {
        // A clone only if every file with content was really cloned.
        method: match (cloned, copied) {
            (0, 0) => backend.method(),
            (_, 0) => Method::Clone,
            _ => Method::Copy,
        },
        files: files.len(),
        cloned,
        copied,
        empty,
        dirs: dirs.len() - 1,
        symlinks: links.len(),
        bytes: bytes.into_inner(),
        elapsed: started.elapsed(),
    })
}

/// Put one file in place. Returns how its content got there, or `None` for an empty file.
fn copy_one(
    src: &Path,
    dst: &Path,
    job: &FileJob,
    backend: &dyn CloneBackend,
    keep_metadata: bool,
) -> Result<Option<Method>> {
    let (from, to) = (src.join(&job.rel), dst.join(&job.rel));
    let method = if job.len == 0 {
        File::create(&to).map_err(|e| io_error("create", &job.rel, e))?;
        None
    } else {
        Some(
            backend
                .copy_file(&from, &to)
                .map_err(|e| io_error("copy", &job.rel, e))?,
        )
    };
    if keep_metadata {
        set_metadata(&to, job)
            .map_err(|e| io_error("set the time and permissions on", &job.rel, e))?;
    }
    Ok(method)
}

/// Copy a file's modification time and permissions through one handle that only has rights to
/// its attributes, so it works on read-only files too. Keeping times means build tools see the
/// same files as in the source and don't rebuild everything.
fn set_metadata(to: &Path, job: &FileJob) -> io::Result<()> {
    let file = open_for_attributes(to)?;
    if let Some(modified) = job.modified {
        file.set_modified(modified)?;
    }
    file.set_permissions(job.permissions.clone())
}

#[cfg(windows)]
fn open_for_attributes(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .open(path)
}

#[cfg(not(windows))]
fn open_for_attributes(path: &Path) -> io::Result<File> {
    // The owner may change times and mode through a read-only descriptor.
    File::open(path)
}

/// A link that points into the source tree by absolute path points into the copy instead, so a
/// lane never reaches back into `_base`. Relative links need nothing.
fn rebase(target: PathBuf, src_root: &Path, dst_root: &Path) -> PathBuf {
    match target.strip_prefix(src_root) {
        Ok(inside) if target.is_absolute() => dst_root.join(inside),
        _ => target,
    }
}

#[cfg(unix)]
fn copy_link(src: &Path, dst: &Path, src_root: &Path, dst_root: &Path) -> Result<()> {
    let target = rebase(
        fs::read_link(src).map_err(|e| io_error("read", src, e))?,
        src_root,
        dst_root,
    );
    std::os::unix::fs::symlink(&target, dst).map_err(|e| io_error("create the link", dst, e))
}

#[cfg(windows)]
fn copy_link(src: &Path, dst: &Path, src_root: &Path, dst_root: &Path) -> Result<()> {
    use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
    // Windows hands back a junction's target as `\\?\D:\...`: plain spelling, so a target inside
    // the source is still seen as inside it.
    let target = rebase(
        junction::plain(&fs::read_link(src).map_err(|e| io_error("read", src, e))?),
        &junction::plain(src_root),
        dst_root,
    );
    // From the link itself, so a link whose target has gone still counts as a folder link.
    let is_dir = fs::symlink_metadata(src).is_ok_and(|m| m.file_type().is_symlink_dir());
    if !is_dir {
        return match symlink_file(&target, dst) {
            Ok(()) => Ok(()),
            // Without Developer Mode, Windows won't make symlinks: copy what a file link points to.
            Err(_) => fs::copy(src, dst)
                .map(|_| ())
                .map_err(|e| io_error("copy", src, e)),
        };
    }
    // A folder junction stays a junction: pnpm links every package with one on Windows. A folder
    // symlink stays a symlink, or becomes a junction when Windows won't make symlinks (no
    // Developer Mode). Junctions need no privilege; they point by absolute path.
    let made = if junction::is_junction(src) {
        junction::make(&junction::absolute(dst, &target), dst)
    } else {
        match symlink_dir(&target, dst) {
            Err(e) if e.raw_os_error() == Some(junction::ERROR_PRIVILEGE_NOT_HELD) => {
                junction::make(&junction::absolute(dst, &target), dst)
            }
            made => made,
        }
    };
    made.map_err(|e| io_error("create the link", dst, e))
}

/// Windows folder junctions: reading what a link is, and making one, without Developer Mode.
#[cfg(windows)]
mod junction {
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FileAttributeTagInfo, GetFileInformationByHandleEx,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    pub const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const GENERIC_WRITE: u32 = 0x4000_0000;

    /// `path` itself, not what it points to.
    fn open_link(path: &Path, access: u32) -> io::Result<File> {
        OpenOptions::new()
            .access_mode(access)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }

    /// Whether `path` is a folder junction (a mount point), not a symlink.
    pub fn is_junction(path: &Path) -> bool {
        let Ok(file) = open_link(path, FILE_READ_ATTRIBUTES) else {
            return false;
        };
        let mut info = FILE_ATTRIBUTE_TAG_INFO {
            FileAttributes: 0,
            ReparseTag: 0,
        };
        // SAFETY: the handle is open for the call; `info` is a FILE_ATTRIBUTE_TAG_INFO of the
        // size passed, which is what FileAttributeTagInfo writes.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileAttributeTagInfo,
                (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        ok != 0 && info.ReparseTag == IO_REPARSE_TAG_MOUNT_POINT
    }

    /// `\\?\D:\x` as `D:\x` and `\\?\UNC\host\share` as `\\host\share`, with backslashes and an
    /// upper-case drive letter, so paths compare by what they name.
    pub fn plain(p: &Path) -> PathBuf {
        let s = p.to_string_lossy().replace('/', "\\");
        let mut s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            rest.to_string()
        } else {
            s
        };
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            s[..1].make_ascii_uppercase();
        }
        PathBuf::from(s)
    }

    /// Where `target` points from the link at `link`, as an absolute path without `.` or `..`:
    /// a junction can only hold that.
    pub fn absolute(link: &Path, target: &Path) -> PathBuf {
        let joined = match target.is_absolute() {
            true => target.to_path_buf(),
            false => link.parent().unwrap_or(link).join(target),
        };
        let mut out = PathBuf::new();
        for c in plain(&joined).components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    /// Make `link` a junction to the folder `target` (absolute).
    pub fn make(target: &Path, link: &Path) -> io::Result<()> {
        fs::create_dir(link)?;
        let made = set_mount_point(target, link);
        if made.is_err() {
            let _ = fs::remove_dir(link);
        }
        made
    }

    fn set_mount_point(target: &Path, link: &Path) -> io::Result<()> {
        let wide = |s: &std::ffi::OsStr| s.encode_wide().collect::<Vec<u16>>();
        // The NT form, `\??\D:\x`, is what the filesystem follows; the plain form is for display.
        let plain = plain(target);
        let substitute = wide(format!(r"\??\{}", plain.display()).as_ref());
        let print = wide(plain.as_os_str());
        let (sub_len, print_len) = (substitute.len() * 2, print.len() * 2);
        // REPARSE_DATA_BUFFER's mount point form: the tag, the data length, a reserved word, then
        // offsets and lengths of the two names, then both names, each followed by a NUL.
        let data_len = 8 + sub_len + 2 + print_len + 2;
        if data_len > 16 * 1024 - 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the junction's target path is too long",
            ));
        }
        let mut buf: Vec<u8> = Vec::with_capacity(8 + data_len);
        buf.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buf.extend((data_len as u16).to_le_bytes());
        buf.extend(0u16.to_le_bytes());
        buf.extend(0u16.to_le_bytes());
        buf.extend((sub_len as u16).to_le_bytes());
        buf.extend(((sub_len + 2) as u16).to_le_bytes());
        buf.extend((print_len as u16).to_le_bytes());
        for unit in substitute.iter().chain([&0]).chain(&print).chain([&0]) {
            buf.extend(unit.to_le_bytes());
        }
        let file = open_link(link, GENERIC_WRITE)?;
        let mut returned = 0u32;
        // SAFETY: the handle is open for writing; `buf` is a complete REPARSE_DATA_BUFFER of the
        // length passed, and no output buffer is used.
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_REPARSE_POINT,
                buf.as_ptr().cast(),
                buf.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        match ok {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = match std::env::var_os("LANE_TEST_CLONE_DIR") {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => std::env::temp_dir(),
        };
        let dir = root.join(format!("lane-clone-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A small repo-like tree: nested folders, a big file, an empty file, a read-only file, a
    /// `.git` to skip and an ignored dependency folder to keep.
    fn sample(root: &Path) -> PathBuf {
        let src = root.join("src");
        for d in [
            "app/routes",
            "node_modules/left-pad",
            ".git/objects",
            "empty-dir",
        ] {
            fs::create_dir_all(src.join(d)).unwrap();
        }
        fs::write(
            src.join("app/routes/auth.ts"),
            "export const login = () => 1;\n",
        )
        .unwrap();
        fs::write(
            src.join("node_modules/left-pad/index.js"),
            "module.exports = x => x;\n",
        )
        .unwrap();
        fs::write(src.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(src.join(".gitignore"), "node_modules\n").unwrap();
        fs::write(src.join("empty.txt"), "").unwrap();
        fs::write(src.join("big.bin"), vec![7u8; 3 * 1024 * 1024 + 17]).unwrap();
        let ro = src.join("locked.txt");
        fs::write(&ro, "read only\n").unwrap();
        let mut p = fs::metadata(&ro).unwrap().permissions();
        p.set_readonly(true);
        fs::set_permissions(&ro, p).unwrap();
        src
    }

    fn unlock(dir: &Path) {
        // Read-only files block cleanup on Windows.
        for f in ["src/locked.txt", "dst/locked.txt"] {
            if let Ok(m) = fs::metadata(dir.join(f)) {
                let mut p = m.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                p.set_readonly(false);
                let _ = fs::set_permissions(dir.join(f), p);
            }
        }
        let _ = fs::remove_dir_all(dir);
    }

    fn check_copy(src: &Path, dst: &Path) {
        for f in [
            "app/routes/auth.ts",
            "node_modules/left-pad/index.js",
            ".gitignore",
            "empty.txt",
            "big.bin",
            "locked.txt",
        ] {
            assert_eq!(
                fs::read(src.join(f)).unwrap(),
                fs::read(dst.join(f)).unwrap(),
                "{f}"
            );
        }
        assert!(dst.join("empty-dir").is_dir());
        assert!(!dst.join(".git").exists(), "the top-level .git is skipped");
        assert!(
            fs::metadata(dst.join("locked.txt"))
                .unwrap()
                .permissions()
                .readonly()
        );
        let (a, b) = (
            fs::metadata(src.join("app/routes/auth.ts"))
                .unwrap()
                .modified()
                .unwrap(),
            fs::metadata(dst.join("app/routes/auth.ts"))
                .unwrap()
                .modified()
                .unwrap(),
        );
        assert_eq!(a, b, "modification times are kept");
    }

    #[test]
    fn plain_copy_copies_everything() {
        let dir = temp("plain");
        let src = sample(&dir);
        let dst = dir.join("dst");
        let report = clone_tree(&src, &dst, &PlainCopy, &Options::default()).unwrap();
        check_copy(&src, &dst);
        assert_eq!(report.method, Method::Copy);
        assert_eq!(report.files, 6);
        assert_eq!((report.cloned, report.copied, report.empty), (0, 5, 1));
        assert_eq!(
            report.dirs, 5,
            "app, app/routes, node_modules, left-pad, empty-dir"
        );
        assert!(report.bytes > 3 * 1024 * 1024);
        unlock(&dir);
    }

    #[test]
    fn reflink_clones_where_it_can_and_copies_where_it_cannot() {
        let dir = temp("reflink");
        let src = sample(&dir);
        let dst = dir.join("dst");
        let can_clone = probe(&dir) == Method::Clone;
        let report = clone_tree(
            &src,
            &dst,
            &Reflink,
            &Options {
                threads: 3,
                ..Options::default()
            },
        )
        .unwrap();
        check_copy(&src, &dst);
        assert_eq!(report.files, 6);
        assert_eq!(report.empty, 1);
        if can_clone {
            assert_eq!(report.method, Method::Clone, "{report:?}");
            assert_eq!((report.cloned, report.copied), (5, 0));
        } else {
            assert_eq!(report.method, Method::Copy, "{report:?}");
            assert_eq!(
                (report.cloned, report.copied),
                (0, 5),
                "nothing is called a clone that wasn't"
            );
        }
        unlock(&dir);
    }

    /// On disks that clone (macOS temp folders, and `LANE_TEST_CLONE_DIR` in CI and on a Dev
    /// Drive), the probe must say so; elsewhere this test checks nothing.
    #[test]
    fn the_probe_finds_clone_support() {
        let dir = temp("probe");
        let expect_clone =
            cfg!(target_os = "macos") || std::env::var_os("LANE_TEST_CLONE_DIR").is_some();
        let found = probe(&dir);
        if expect_clone {
            assert_eq!(found, Method::Clone, "{:?}", detect(&dir));
        }
        assert!(
            fs::read_dir(&dir).unwrap().next().is_none(),
            "the probe leaves nothing behind"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_with_files_is_never_overwritten() {
        let dir = temp("occupied");
        let src = sample(&dir);
        let dst = dir.join("dst");
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("keep.txt"), "mine").unwrap();
        let e = clone_tree(&src, &dst, &PlainCopy, &Options::default()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Conflict);
        assert_eq!(fs::read_to_string(dst.join("keep.txt")).unwrap(), "mine");
        unlock(&dir);
    }

    #[test]
    fn a_destination_inside_the_source_is_skipped() {
        let dir = temp("nested");
        let src = sample(&dir);
        let dst = src.join("lanes/one");
        fs::create_dir_all(src.join("lanes")).unwrap();
        let report = clone_tree(&src, &dst, &PlainCopy, &Options::default()).unwrap();
        assert!(dst.join("app/routes/auth.ts").exists());
        assert!(
            !dst.join("lanes/one").exists(),
            "never copies itself into itself"
        );
        assert_eq!(report.files, 6);
        unlock(&dir);
    }

    #[test]
    fn skipped_names_may_already_be_in_the_destination() {
        let dir = temp("own-git");
        let src = sample(&dir);
        let dst = dir.join("dst");
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join(".git"), "gitdir: elsewhere\n").unwrap();
        clone_tree(&src, &dst, &PlainCopy, &Options::default()).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join(".git")).unwrap(),
            "gitdir: elsewhere\n",
            "kept, not overwritten"
        );
        assert!(dst.join("app/routes/auth.ts").exists());
        unlock(&dir);
    }

    #[test]
    fn absolute_links_into_the_source_are_rebased() {
        let root = std::env::temp_dir().join("acme.lanes");
        let (src, dst) = (root.join("_base"), root.join("fix"));
        assert_eq!(
            rebase(src.join("node_modules/x"), &src, &dst),
            dst.join("node_modules/x")
        );
        assert_eq!(
            rebase(PathBuf::from("../x"), &src, &dst),
            PathBuf::from("../x")
        );
        let elsewhere = std::env::temp_dir().join("tools/node");
        assert_eq!(rebase(elsewhere.clone(), &src, &dst), elsewhere);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_stay_symlinks_and_modes_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("links");
        let src = sample(&dir);
        std::os::unix::fs::symlink("app/routes/auth.ts", src.join("link.ts")).unwrap();
        let script = src.join("run.sh");
        fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let dst = dir.join("dst");
        let report = clone_tree(&src, &dst, &Reflink, &Options::default()).unwrap();
        assert_eq!(report.symlinks, 1);
        assert_eq!(
            fs::read_link(dst.join("link.ts")).unwrap(),
            Path::new("app/routes/auth.ts")
        );
        assert_eq!(
            fs::metadata(dst.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        unlock(&dir);
    }

    /// pnpm on Windows links every package with a folder junction, by absolute path. A lane gets
    /// junctions of its own, without Developer Mode, pointing into the lane, never back at `_base`.
    #[cfg(windows)]
    #[test]
    fn junctions_stay_junctions_and_point_into_the_copy() {
        let dir = temp("junctions");
        let src = dir.join("src");
        let pkg = src.join("node_modules/.pnpm/left-pad@1.3.0/node_modules/left-pad");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.js"), "module.exports = x => x;\n").unwrap();
        let outside = dir.join("store");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("shared.txt"), "shared\n").unwrap();
        // Made the way pnpm makes them, with `mklink /J`, which needs no privilege.
        let mklink = |link: &Path, target: &Path| {
            let ok = std::process::Command::new("cmd")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(junction::plain(link))
                .arg(junction::plain(target))
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "mklink /J {}", link.display());
        };
        mklink(&src.join("node_modules/left-pad"), &pkg);
        mklink(&src.join("node_modules/store"), &outside);

        let dst = dir.join("dst");
        let report = clone_tree(&src, &dst, &PlainCopy, &Options::default()).unwrap();
        assert_eq!(report.symlinks, 2);
        let inside = dst.join("node_modules/left-pad");
        assert!(
            junction::is_junction(&inside),
            "a junction stays a junction"
        );
        assert_eq!(
            junction::plain(&fs::read_link(&inside).unwrap()),
            junction::plain(&dst.join("node_modules/.pnpm/left-pad@1.3.0/node_modules/left-pad")),
            "a link into the tree points into the copy"
        );
        // Writing through the lane's link changes the lane, not the source.
        fs::write(inside.join("index.js"), "changed\n").unwrap();
        assert_eq!(
            fs::read_to_string(pkg.join("index.js")).unwrap(),
            "module.exports = x => x;\n"
        );
        let store = dst.join("node_modules/store");
        assert!(junction::is_junction(&store));
        assert_eq!(
            junction::plain(&fs::read_link(&store).unwrap()),
            junction::plain(&outside),
            "a link out of the tree still points where it did"
        );
        assert_eq!(
            fs::read_to_string(store.join("shared.txt")).unwrap(),
            "shared\n"
        );
        // Removing the copy removes its links, never what they point to.
        fs::remove_dir_all(&dst).unwrap();
        assert!(outside.join("shared.txt").exists() && pkg.join("index.js").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_by_what_they_name() {
        assert_eq!(
            junction::plain(Path::new(r"\\?\d:\a\b")),
            PathBuf::from(r"D:\a\b")
        );
        assert_eq!(
            junction::plain(Path::new(r"\\?\UNC\host\share\x")),
            PathBuf::from(r"\\host\share\x")
        );
        assert_eq!(
            junction::plain(Path::new("D:/a/b")),
            PathBuf::from(r"D:\a\b")
        );
        assert_eq!(
            junction::absolute(Path::new(r"D:\l\node_modules\x"), Path::new(r"..\.pnpm\x")),
            PathBuf::from(r"D:\l\.pnpm\x")
        );
    }
}
