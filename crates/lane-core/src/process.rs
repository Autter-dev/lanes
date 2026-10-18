//! Starting a process that outlives `lane` without holding anything of `lane`'s open: the install
//! count's `curl`, and `laned`.

use std::process::{Command, Stdio};

/// Set `cmd` up to run on its own: no input or output, its own session (Unix) or process group
/// without a console (Windows), so closing the terminal or pressing Ctrl-C doesn't stop it, and
/// nothing reading `lane`'s output waits for it.
pub fn detach(cmd: &mut Command) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid only changes the new child's session; it allocates nothing and is
        // async-signal-safe, as pre_exec requires.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    keep_std_handles_to_ourselves();
}

/// Windows hands every inheritable handle to a new process, so a child would hold on to the
/// pipes `lane`'s output goes to, and whoever reads them (an agent's hook runner, a script) would
/// wait for that child. Our own standard handles stop being inheritable. Children that should
/// share them still can: `Stdio::inherit` hands over a fresh inheritable copy.
pub fn keep_std_handles_to_ourselves() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{
            HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: GetStdHandle has no preconditions; SetHandleInformation only changes a
            // flag on a handle this process owns, and is skipped for missing or invalid handles.
            unsafe {
                let handle = GetStdHandle(which);
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}
