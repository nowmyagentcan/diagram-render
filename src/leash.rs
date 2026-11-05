//! What ties a child process to this one: it ends when this process ends, however that happens,
//! and its memory use can be read. The system does this differently on each one.

#[cfg(not(windows))]
use std::process::Child;
use std::process::Command;

#[cfg(windows)]
pub use windows_leash::Leash;

/// Readies `cmd` so the child ends with this process: on Linux the kernel sees to it.
#[cfg(target_os = "linux")]
pub fn prepare(cmd: &mut Command) {
    use std::io;
    use std::os::unix::process::CommandExt;
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: prctl, getppid and _exit are safe to call between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            // This process may have ended before the line above took effect.
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
pub fn prepare(_cmd: &mut Command) {}

/// On Linux `prepare` did the work; the child's use is in `/proc`.
#[cfg(target_os = "linux")]
pub struct Leash(u32);

#[cfg(target_os = "linux")]
impl Leash {
    pub fn new(child: &Child) -> std::io::Result<Self> {
        Ok(Self(child.id()))
    }

    /// The bytes of memory the child holds.
    pub fn memory(&self) -> u64 {
        let status = std::fs::read_to_string(format!("/proc/{}/status", self.0));
        let status = status.unwrap_or_default();
        let line = status.lines().find_map(|l| l.strip_prefix("VmRSS:"));
        let kib = line.and_then(|l| l.trim().trim_end_matches(" kB").parse::<u64>().ok());
        kib.unwrap_or(0) << 10
    }
}

/// macOS has no such setting, so a small shell waits on a pipe only this process holds: when the
/// pipe closes without a line, this process is gone and the shell kills the child.
#[cfg(target_os = "macos")]
pub struct Leash {
    pid: u32,
    watcher: Child,
}

#[cfg(target_os = "macos")]
impl Leash {
    pub fn new(child: &Child) -> std::io::Result<Self> {
        use std::process::Stdio;
        let pid = child.id();
        let script = r#"read line || kill -KILL "$1""#;
        let watcher = Command::new("/bin/sh")
            .args(["-c", script, "sh", &pid.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self { pid, watcher })
    }

    /// The bytes of memory the child holds.
    pub fn memory(&self) -> u64 {
        let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
        // SAFETY: `info` is a buffer of the size given, and is read only when it was filled.
        unsafe {
            let flavor = libc::PROC_PIDTASKINFO;
            let filled =
                libc::proc_pidinfo(self.pid as _, flavor, 0, info.as_mut_ptr().cast(), size);
            match filled == size {
                true => info.assume_init().pti_resident_size,
                false => 0,
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Leash {
    /// A line tells the shell to leave the child alone: it has ended.
    fn drop(&mut self) {
        use std::io::Write;
        if let Some(mut pipe) = self.watcher.stdin.take() {
            // The shell is gone if this fails, and the wait below ends at once.
            let _ = pipe.write_all(b"\n");
        }
        let _ = self.watcher.wait();
    }
}

/// A job object: the system ends what is in it when the last handle closes, and this process's
/// handle closes when this process ends.
#[cfg(windows)]
mod windows_leash {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject,
    };
    use windows::core::PCWSTR;

    const SIZE: u32 = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;

    pub struct Leash(HANDLE);

    impl Leash {
        pub fn new(child: &Child) -> io::Result<Self> {
            // SAFETY: each call gets a live handle and a buffer of the size it is told.
            unsafe {
                let leash = Self(CreateJobObjectW(None, PCWSTR::null())?);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let info = (&raw const limits).cast();
                SetInformationJobObject(leash.0, JobObjectExtendedLimitInformation, info, SIZE)?;
                AssignProcessToJobObject(leash.0, HANDLE(child.as_raw_handle()))?;
                Ok(leash)
            }
        }

        /// The most bytes of memory the child has held.
        pub fn memory(&self) -> u64 {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            let buffer = (&raw mut info).cast();
            // SAFETY: the handle is live and `buffer` has the size given.
            let read = unsafe {
                let class = JobObjectExtendedLimitInformation;
                QueryInformationJobObject(Some(self.0), class, buffer, SIZE, None)
            };
            read.map_or(0, |()| info.PeakProcessMemoryUsed as u64)
        }
    }

    impl Drop for Leash {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and isn't used again. The child has been reaped.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}
