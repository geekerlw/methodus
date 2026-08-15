//! Single-instance advisory lock (`~/.methodus/methodus.lock`).
//!
//! Mutating commands (`run`, `task create`, `recover`) take the lock.
//! Read-only queries bypass it so a second terminal can `task list` while a run
//! is in progress.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::error::CoreError;

/// Held for the lifetime of a mutating CLI invocation. Dropping releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// Try to acquire an exclusive flock. Fails immediately if another instance holds it.
    pub fn try_acquire(home: &Path) -> Result<Self, CoreError> {
        let path = home.join("methodus.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;

        try_lock_exclusive(&file).map_err(|e| {
            if e.kind() == io::ErrorKind::WouldBlock {
                CoreError::Locked(path.clone())
            } else {
                CoreError::Io(e)
            }
        })?;

        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;

        Ok(Self { _file: file })
    }
}

fn try_lock_exclusive(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

/// Whether `pid` still exists. `kill(pid, 0)`: 0 or EPERM → alive; ESRCH → dead.
pub fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn second_lock_fails() {
        let dir = tempdir().unwrap();
        let first = InstanceLock::try_acquire(dir.path()).unwrap();
        let err = InstanceLock::try_acquire(dir.path()).unwrap_err();
        assert!(matches!(err, CoreError::Locked(_)));
        drop(first);
        InstanceLock::try_acquire(dir.path()).unwrap();
    }

    #[test]
    fn current_process_is_alive() {
        assert!(process_is_alive(std::process::id()));
    }

    #[test]
    fn bogus_pid_is_dead() {
        // PID 1 is init/launchd and is alive; pick a huge unlikely pid.
        assert!(!process_is_alive(u32::MAX - 7));
    }
}
