//! Starting and stopping processes: the part that differs most between systems, behind
//! [`ProcessControl`]. [`Unix`] uses process groups and signals; [`Windows`] uses Job Objects.
//!
//! A lane's commands (`lane exec`) start in a group of their own, so `lane rm` can stop
//! everything they started, and the terminal stays theirs while they run. `laned` and the
//! install count's `curl` start detached, so they outlive `lane` without holding on to it.

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// How long a lane's processes get to stop when asked, before they're stopped outright.
pub const GRACE: Duration = Duration::from_secs(3);

/// A command started in a group of its own, which everything it starts joins too.
pub struct Group {
    child: Child,
    /// The Job Object that holds the tree, named after the command's process id so another
    /// process (`laned`, `lane rm`) can stop it. Its name lasts as long as this handle. None if
    /// the job couldn't be made.
    #[cfg(windows)]
    _job: Option<windows::Handle>,
    /// It has the terminal: Ctrl-C and Ctrl-Z go to it, as to a shell's foreground job.
    #[cfg(unix)]
    terminal: bool,
}

impl Group {
    /// The command's process id. It names the group: pass it to [`ProcessControl::stop`].
    pub fn id(&self) -> u32 {
        self.child.id()
    }
}

/// Starting, stopping and watching process trees, done each system's way.
pub trait ProcessControl: Send + Sync {
    /// Start `cmd` in a group of its own: a new process group (Unix) or a Job Object (Windows),
    /// so everything it starts can be stopped with it. With `foreground`, it gets the terminal
    /// the way a shell's foreground job does. On Windows this sets `cmd`'s creation flags.
    fn spawn(&self, cmd: &mut Command, foreground: bool) -> io::Result<Group>;

    /// Wait for the command to end. Ctrl-C is the command's to act on: this waits for it to
    /// finish rather than leaving it behind.
    fn wait(&self, group: &mut Group) -> io::Result<ExitStatus>;

    /// Ask the group led by `pid` to stop, so it can clean up: SIGTERM on Unix. False when the
    /// system has no way to ask (Windows console programs), so only [`kill`](Self::kill) stops
    /// it.
    fn terminate(&self, pid: u32) -> io::Result<bool>;

    /// Stop the group led by `pid` now: SIGKILL on Unix, TerminateJobObject on Windows.
    fn kill(&self, pid: u32) -> io::Result<()>;

    /// Whether anything in the group led by `pid` is still running.
    fn alive(&self, pid: u32) -> bool;

    /// Whether this process is in the group led by `pid`, like a script run in a lane that
    /// removes its own lane. Stopping that group would stop the caller halfway.
    fn holds_us(&self, pid: u32) -> bool;

    /// Set `cmd` up to run on its own: no input or output, its own session (Unix) or process
    /// group without a console (Windows), so closing the terminal or pressing Ctrl-C doesn't
    /// stop it, and nothing reading `lane`'s output waits for it.
    fn detach(&self, cmd: &mut Command);

    /// No console window for `cmd`, which runs while an agent's hook may be running `lane`
    /// from a GUI app (Windows). Nothing elsewhere.
    fn no_window(&self, cmd: &mut Command);

    /// Stop our own standard handles being handed to the processes we start (Windows), so
    /// whoever reads `lane`'s output doesn't wait for them. Nothing elsewhere.
    fn keep_std_handles_to_ourselves(&self);

    /// Stop the groups led by `pids`: ask each first, then after `grace`, stop whatever's left
    /// outright. Groups this process is in are left alone. Returns once they've all gone, or a
    /// couple of seconds after forcing; the pids still running then.
    fn stop(&self, pids: &[u32], grace: Duration) -> Vec<u32> {
        let pids: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|&p| p > 0 && !self.holds_us(p))
            .collect();
        let mut asked = false;
        for &pid in &pids {
            asked |= self.terminate(pid).unwrap_or(false);
        }
        if asked {
            wait_until_gone(self, &pids, grace);
        }
        for &pid in &pids {
            if self.alive(pid) {
                let _ = self.kill(pid);
            }
        }
        wait_until_gone(self, &pids, Duration::from_secs(2));
        pids.into_iter().filter(|&p| self.alive(p)).collect()
    }
}

fn wait_until_gone<P: ProcessControl + ?Sized>(control: &P, pids: &[u32], limit: Duration) {
    let deadline = Instant::now() + limit;
    while pids.iter().any(|&p| control.alive(p)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// This system's [`ProcessControl`].
pub fn system() -> &'static dyn ProcessControl {
    #[cfg(unix)]
    {
        &Unix
    }
    #[cfg(windows)]
    {
        &Windows
    }
}

/// [`ProcessControl::detach`] for this system.
pub fn detach(cmd: &mut Command) {
    system().detach(cmd)
}

/// [`ProcessControl::keep_std_handles_to_ourselves`] for this system.
pub fn keep_std_handles_to_ourselves() {
    system().keep_std_handles_to_ourselves()
}

#[cfg(unix)]
pub use unix::Unix;
#[cfg(windows)]
pub use windows::Windows;

#[cfg(unix)]
mod unix {
    use super::{Group, ProcessControl};
    use std::io;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicI32, Ordering};

    /// Process groups and signals.
    pub struct Unix;

    /// The group [`Unix::wait`] passes signals on to.
    static WAITING_FOR: AtomicI32 = AtomicI32::new(0);

    /// Signals for the command more than for `lane`: passed on to its group while waiting.
    const PASSED_ON: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

    extern "C" fn pass_on(signal: libc::c_int) {
        let group = WAITING_FOR.load(Ordering::SeqCst);
        if group > 1 {
            // SAFETY: kill is async-signal-safe.
            unsafe { libc::kill(-group, signal) };
        }
    }

    /// Whether this process is in the foreground of the terminal on its standard input.
    fn in_foreground() -> bool {
        // SAFETY: these only read the terminal's state.
        unsafe { libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp() }
    }

    /// The terminal's foreground group, or -1 when standard input isn't a terminal.
    fn foreground_group() -> libc::pid_t {
        // SAFETY: tcgetpgrp only reads the terminal's state.
        unsafe { libc::tcgetpgrp(0) }
    }

    /// Make `group` the terminal's foreground group, as a shell does for the job it runs.
    /// SIGTTOU is ignored meanwhile, as a process outside the foreground would otherwise be
    /// stopped for it. Async-signal-safe.
    fn give_terminal(group: libc::pid_t) {
        // SAFETY: signal and tcsetpgrp are async-signal-safe and touch no memory of ours.
        unsafe {
            let old = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(0, group);
            libc::signal(libc::SIGTTOU, old);
        }
    }

    /// Send `signal` to the group led by `pid`, or to `pid` alone if it leads none. A group
    /// that's gone already is fine.
    fn signal_group(pid: u32, signal: libc::c_int) -> io::Result<()> {
        let pid = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
        // Never 0 (our own group) or 1 (init), and -1 would mean every process.
        if pid <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a process group",
            ));
        }
        for target in [-pid, pid] {
            // SAFETY: kill has no memory-safety preconditions.
            if unsafe { libc::kill(target, signal) } == 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e);
            }
        }
        Ok(())
    }

    impl ProcessControl for Unix {
        fn spawn(&self, cmd: &mut Command, foreground: bool) -> io::Result<Group> {
            let terminal = foreground && in_foreground();
            // SAFETY: setpgid, getpid, signal and tcsetpgrp are async-signal-safe and allocate
            // nothing, as pre_exec requires.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    // Before it runs, so its first read of the terminal can't stop it.
                    if terminal {
                        give_terminal(libc::getpid());
                    }
                    Ok(())
                });
            }
            let child = cmd.spawn()?;
            let pid = child.id() as libc::pid_t;
            // As shells do: set from both sides, so it holds whichever runs first.
            // SAFETY: setpgid on our own child has no memory-safety preconditions.
            unsafe { libc::setpgid(pid, pid) };
            if terminal {
                give_terminal(pid);
            }
            Ok(Group { child, terminal })
        }

        fn wait(&self, group: &mut Group) -> io::Result<ExitStatus> {
            let pid = group.child.id() as libc::pid_t;
            WAITING_FOR.store(pid, Ordering::SeqCst);
            let handler = pass_on as extern "C" fn(libc::c_int) as libc::sighandler_t;
            // SAFETY: pass_on only calls kill, which is async-signal-safe.
            let old: Vec<libc::sighandler_t> = PASSED_ON
                .iter()
                .map(|&s| unsafe { libc::signal(s, handler) })
                .collect();
            let status = loop {
                let mut status = 0;
                // SAFETY: waitpid writes only to `status`.
                if unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) } == -1 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break Err(e);
                }
                if !libc::WIFSTOPPED(status) {
                    break Ok(ExitStatus::from_raw(status));
                }
                // Ctrl-Z stopped the command (or it read the terminal from the background). Take
                // the terminal back and stop too, so the shell sees its job stop; once continued,
                // hand the terminal back if it's ours (`fg`, not `bg`) and wake the command.
                // Without the terminal, a stop is left for whoever sent it.
                if group.terminal {
                    if foreground_group() == pid {
                        // SAFETY: getpgrp has no preconditions.
                        give_terminal(unsafe { libc::getpgrp() });
                    }
                    // SAFETY: kill has no memory-safety preconditions.
                    unsafe { libc::kill(0, libc::SIGTSTP) };
                    if in_foreground() {
                        give_terminal(pid);
                    }
                    // SAFETY: as above.
                    unsafe { libc::kill(-pid, libc::SIGCONT) };
                }
            };
            if group.terminal && foreground_group() == pid {
                // SAFETY: getpgrp has no preconditions.
                give_terminal(unsafe { libc::getpgrp() });
            }
            for (&s, h) in PASSED_ON.iter().zip(old) {
                // SAFETY: putting back the handler that was there.
                unsafe { libc::signal(s, h) };
            }
            WAITING_FOR.store(0, Ordering::SeqCst);
            status
        }

        fn terminate(&self, pid: u32) -> io::Result<bool> {
            signal_group(pid, libc::SIGTERM)?;
            // A stopped group can't act on SIGTERM until it's continued.
            let _ = signal_group(pid, libc::SIGCONT);
            Ok(true)
        }

        fn kill(&self, pid: u32) -> io::Result<()> {
            signal_group(pid, libc::SIGKILL)
        }

        fn alive(&self, pid: u32) -> bool {
            let Ok(pid) = libc::pid_t::try_from(pid) else {
                return false;
            };
            if pid <= 1 {
                return false;
            }
            // The group, or the process if it leads none. Someone else's is still running.
            [-pid, pid].into_iter().any(|target| {
                // SAFETY: signal 0 only checks that the target exists and may be signalled.
                let there = unsafe { libc::kill(target, 0) } == 0;
                there || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
            })
        }

        fn holds_us(&self, pid: u32) -> bool {
            // SAFETY: getpgrp has no preconditions.
            libc::pid_t::try_from(pid).is_ok_and(|p| p == unsafe { libc::getpgrp() })
        }

        fn detach(&self, cmd: &mut Command) {
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // SAFETY: setsid only changes the new child's session; it allocates nothing and is
            // async-signal-safe, as pre_exec requires.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }

        fn no_window(&self, _: &mut Command) {}

        fn keep_std_handles_to_ourselves(&self) {}
    }
}

#[cfg(windows)]
mod windows {
    use super::{Group, ProcessControl};
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        SetHandleInformation, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
        OpenJobObjectW, QueryInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, DETACHED_PROCESS,
        GetCurrentProcess, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ResumeThread, THREAD_SUSPEND_RESUME,
        TerminateProcess, WaitForSingleObject,
    };

    // Job access rights (winnt.h).
    const JOB_OBJECT_QUERY: u32 = 0x0004;
    const JOB_OBJECT_TERMINATE: u32 = 0x0008;

    /// The exit code of a process stopped from outside, as `taskkill /F` uses.
    const STOPPED: u32 = 1;

    /// Job Objects.
    pub struct Windows;

    /// A handle, closed when dropped.
    pub struct Handle(HANDLE);

    // SAFETY: a kernel handle may be used and closed from any thread.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Handle {
        fn open(handle: HANDLE) -> Option<Handle> {
            (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(Handle(handle))
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: we own the handle and close it once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// The name another process opens a command's job by: `Local\` is this login session's.
    fn job_name(pid: u32) -> Vec<u16> {
        format!("Local\\lanes-exec-{pid}")
            .encode_utf16()
            .chain([0])
            .collect()
    }

    fn open_job(pid: u32, access: u32) -> Option<Handle> {
        let name = job_name(pid);
        // SAFETY: the name is NUL-terminated and outlives the call.
        Handle::open(unsafe { OpenJobObjectW(access, 0, name.as_ptr()) })
    }

    /// A job named after `child`, holding it. The child is still suspended, so nothing it
    /// starts can slip out first.
    fn new_job(child: &Child) -> io::Result<Handle> {
        let name = job_name(child.id());
        // SAFETY: no security attributes means the default ones; the name is NUL-terminated.
        let job = Handle::open(unsafe { CreateJobObjectW(std::ptr::null(), name.as_ptr()) })
            .ok_or_else(io::Error::last_os_error)?;
        // SAFETY: both handles are open.
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Resume the threads of a process started suspended.
    fn resume(pid: u32) -> io::Result<()> {
        // SAFETY: a snapshot of every thread; closed when `snapshot` drops.
        let snapshot = Handle::open(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })
            .ok_or_else(io::Error::last_os_error)?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = false;
        // SAFETY: `entry` is a THREADENTRY32 with its size set, as both calls require.
        let mut more = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: opening a thread by id; closed when `thread` drops.
                let thread = Handle::open(unsafe {
                    OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID)
                });
                // SAFETY: an open thread handle.
                if let Some(thread) = thread
                    && unsafe { ResumeThread(thread.0) } != u32::MAX
                {
                    resumed = true;
                }
            }
            // SAFETY: as above.
            more = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
        }
        match resumed {
            true => Ok(()),
            false => Err(io::Error::other(
                "couldn't resume the command after starting it",
            )),
        }
    }

    /// While `lane` waits, Ctrl-C and Ctrl-Break (which reach every process on the console) are
    /// the command's to act on. Closing the console still ends `lane`.
    unsafe extern "system" fn leave_to_the_command(event: u32) -> windows_sys::core::BOOL {
        (event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT) as windows_sys::core::BOOL
    }

    fn process_alive(pid: u32) -> bool {
        // SAFETY: opening a process by id; closed when `process` drops.
        let process = Handle::open(unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        });
        match process {
            // SAFETY: an open process handle; a zero timeout only checks whether it has ended.
            Some(p) => (unsafe { WaitForSingleObject(p.0, 0) }) == WAIT_TIMEOUT,
            // Someone else's process is still a process.
            None => io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED as i32),
        }
    }

    impl ProcessControl for Windows {
        fn spawn(&self, cmd: &mut Command, _foreground: bool) -> io::Result<Group> {
            // It shares the console either way, so Ctrl-C reaches it as it is.
            cmd.creation_flags(CREATE_SUSPENDED);
            let mut child = cmd.spawn()?;
            // Without a job (Windows before 8 inside another job, say), it still runs, and
            // stopping it stops the command alone.
            let job = match new_job(&child) {
                Ok(job) => Some(job),
                Err(e) => {
                    crate::log::warn(
                        "exec",
                        format!(
                            "couldn't put `{}` in a job of its own ({e}); stopping it may leave what it started running",
                            cmd.get_program().to_string_lossy()
                        ),
                    )
                    .write();
                    None
                }
            };
            // Never leave it suspended.
            if let Err(e) = resume(child.id()) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            Ok(Group { child, _job: job })
        }

        fn wait(&self, group: &mut Group) -> io::Result<ExitStatus> {
            // SAFETY: the handler touches no memory and stays valid for the life of the process.
            unsafe { SetConsoleCtrlHandler(Some(leave_to_the_command), 1) };
            let status = group.child.wait();
            // SAFETY: as above.
            unsafe { SetConsoleCtrlHandler(Some(leave_to_the_command), 0) };
            status
        }

        fn terminate(&self, _pid: u32) -> io::Result<bool> {
            // Console programs can't be asked from outside: no SIGTERM, and Ctrl-C would reach
            // everything on the console, the terminal's shell included.
            Ok(false)
        }

        fn kill(&self, pid: u32) -> io::Result<()> {
            if let Some(job) = open_job(pid, JOB_OBJECT_TERMINATE) {
                // SAFETY: an open job handle with terminate access.
                return match unsafe { TerminateJobObject(job.0, STOPPED) } {
                    0 => Err(io::Error::last_os_error()),
                    _ => Ok(()),
                };
            }
            // No job: the process alone, if it's still there.
            // SAFETY: opening a process by id; closed when `process` drops.
            let process = Handle::open(unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) });
            // SAFETY: an open process handle with terminate access.
            let done = process.is_some_and(|p| unsafe { TerminateProcess(p.0, STOPPED) } != 0);
            let error = io::Error::last_os_error();
            match done || !process_alive(pid) {
                true => Ok(()),
                false => Err(error),
            }
        }

        fn alive(&self, pid: u32) -> bool {
            if let Some(job) = open_job(pid, JOB_OBJECT_QUERY) {
                let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
                // SAFETY: `info` is the struct this class fills, with its size given.
                let ok = unsafe {
                    QueryInformationJobObject(
                        job.0,
                        JobObjectBasicAccountingInformation,
                        (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                        std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                        std::ptr::null_mut(),
                    )
                };
                if ok != 0 {
                    return info.ActiveProcesses > 0;
                }
            }
            process_alive(pid)
        }

        fn holds_us(&self, pid: u32) -> bool {
            let Some(job) = open_job(pid, JOB_OBJECT_QUERY) else {
                return pid == std::process::id();
            };
            let mut inside = 0;
            // SAFETY: GetCurrentProcess is a pseudo-handle; `inside` is written by the call.
            let ok = unsafe { IsProcessInJob(GetCurrentProcess(), job.0, &mut inside) };
            ok != 0 && inside != 0
        }

        fn detach(&self, cmd: &mut Command) {
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            self.keep_std_handles_to_ourselves();
        }

        fn no_window(&self, cmd: &mut Command) {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        /// Windows hands every inheritable handle to a new process, so a child would hold on to
        /// the pipes `lane`'s output goes to, and whoever reads them (an agent's hook runner, a
        /// script) would wait for that child. Children that should share them still can:
        /// `Stdio::inherit` hands over a fresh inheritable copy.
        fn keep_std_handles_to_ourselves(&self) {
            for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                // SAFETY: GetStdHandle has no preconditions; SetHandleInformation only changes
                // a flag on a handle this process owns, and is skipped for missing or invalid
                // handles.
                unsafe {
                    let handle = GetStdHandle(which);
                    if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                        SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// A command that runs for a minute and starts a process of its own, in `dir`.
    fn a_minute(dir: &std::path::Path) -> Command {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 60 127.0.0.1 > nul"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 60; exit 0"]);
            c
        };
        cmd.current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    #[test]
    fn a_group_is_stopped_whole_and_waiting_ends() {
        let dir = std::env::temp_dir().join(format!("lane-process-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let control = system();
        let mut group = control.spawn(&mut a_minute(&dir), false).unwrap();
        let pid = group.id();
        assert!(control.alive(pid));
        assert!(!control.holds_us(pid));

        let waiting = std::thread::spawn(move || {
            let status = system().wait(&mut group).unwrap();
            (status, group)
        });
        let started = Instant::now();
        assert!(
            control.stop(&[pid], GRACE).is_empty(),
            "nothing left running"
        );
        assert!(!control.alive(pid));
        let (status, _group) = waiting.join().unwrap();
        assert!(!status.success(), "{status:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // Its folder can go now: nothing holds it open.
        std::fs::remove_dir_all(&dir).unwrap();
        // Stopping again, or something that's gone, is fine.
        assert!(control.stop(&[pid], GRACE).is_empty());
        assert!(control.kill(pid).is_ok());
    }

    #[test]
    fn the_exit_code_comes_back() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit 7"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "exit 7"]);
            c
        };
        let control = system();
        let mut group = control.spawn(&mut cmd, false).unwrap();
        assert_eq!(control.wait(&mut group).unwrap().code(), Some(7));
    }

    #[test]
    fn our_own_group_is_never_stopped() {
        let control = system();
        assert!(control.stop(&[0], Duration::ZERO).is_empty());
        #[cfg(unix)]
        {
            // SAFETY: getpgrp has no preconditions.
            let ours = unsafe { libc::getpgrp() } as u32;
            assert!(control.holds_us(ours), "so `stop` leaves it alone");
        }
        #[cfg(windows)]
        assert!(control.holds_us(std::process::id()));
    }
}
