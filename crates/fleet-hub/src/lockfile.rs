//! Single-instance lockfile guard (the design: "lockfile single-instance").
//!
//! The Hub never auto-exits (D2); the user quits it explicitly. To make a
//! *second* `fleet-hub` launch refuse rather than fight over the socket, the
//! first instance takes an exclusive lockfile on startup and holds it for its
//! lifetime. A second launch that finds a *live* lock refuses
//! ([`LockError::AlreadyRunning`]); a lock left behind by a *crashed* instance
//! is detected as stale and reclaimed, so a crash never wedges the Hub out
//! permanently.
//!
//! Mechanism (stdlib-only, no extra deps):
//! - Acquire is an atomic `create_new` (O_EXCL) of the lock path. If that
//!   succeeds we own the lock and write our pid into it.
//! - If `create_new` fails with `AlreadyExists`, we read the recorded pid and
//!   probe liveness. A dead pid ⇒ stale lock ⇒ remove and retry once. A live
//!   pid ⇒ another Hub is up ⇒ refuse.
//! - On drop we remove the file, but only if it still records *our* pid (so we
//!   never delete a lock a different instance has since taken).
//!
//! This is advisory and best-effort across a crash window, which is exactly the
//! contract D2 needs: "a second Hub launch refuses".

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Errors acquiring the single-instance lock.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another live Hub instance already holds the lock.
    #[error("another fleet-hub instance is already running (pid {pid}, lock {path})")]
    AlreadyRunning { pid: u32, path: PathBuf },

    /// An I/O error while manipulating the lockfile.
    #[error("lockfile I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// An owned single-instance lock. Holding this value means this process is the
/// sole Hub; dropping it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    path: PathBuf,
    pid: u32,
}

impl InstanceLock {
    /// Acquire the single-instance lock at `path`, creating parent dirs.
    ///
    /// Returns [`LockError::AlreadyRunning`] if a live instance holds it.
    pub fn acquire(path: impl AsRef<Path>) -> Result<Self, LockError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| LockError::Io {
                    path: path.clone(),
                    source,
                })?;
            }
        }
        // Two attempts: the second only happens after we reclaim a stale lock.
        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let pid = std::process::id();
                    f.write_all(pid.to_string().as_bytes())
                        .map_err(|source| LockError::Io {
                            path: path.clone(),
                            source,
                        })?;
                    f.flush().map_err(|source| LockError::Io {
                        path: path.clone(),
                        source,
                    })?;
                    return Ok(InstanceLock { path, pid });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = read_pid(&path);
                    match holder {
                        Some(pid) if pid_is_alive(pid) => {
                            return Err(LockError::AlreadyRunning { pid, path });
                        }
                        _ => {
                            // Stale (dead pid or unreadable) — reclaim once.
                            if attempt == 0 {
                                let _ = fs::remove_file(&path);
                                continue;
                            }
                            // Lost a race to another reclaimer that is now live,
                            // or persistent unreadable lock: treat as running.
                            let pid = read_pid(&path).unwrap_or(0);
                            return Err(LockError::AlreadyRunning { pid, path });
                        }
                    }
                }
                Err(source) => return Err(LockError::Io { path, source }),
            }
        }
        unreachable!("acquire loop always returns within 2 attempts")
    }

    /// The path this lock is held at.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pid recorded in this lock (this process).
    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Only remove the file if it still records *our* pid, so we never delete
        // a lock another instance has since taken (e.g. after a manual rm).
        if read_pid(&self.path) == Some(self.pid) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Read the pid recorded in a lockfile, if it is present and parseable.
fn read_pid(path: &Path) -> Option<u32> {
    let mut s = String::new();
    OpenOptions::new()
        .read(true)
        .open(path)
        .ok()?
        .read_to_string(&mut s)
        .ok()?;
    s.trim().parse::<u32>().ok()
}

/// Probe whether a pid corresponds to a live process.
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // `kill(pid, 0)` performs error checking without sending a signal: 0 ⇒
    // alive (and signalable), EPERM ⇒ alive but not ours, ESRCH ⇒ no such pid.
    // SAFETY: kill with signal 0 has no side effects beyond setting errno.
    #[allow(unsafe_code)]
    let rc = unsafe { libc_kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    // Distinguish EPERM (alive) from ESRCH (dead).
    std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
}

#[cfg(not(unix))]
fn pid_is_alive(pid: u32) -> bool {
    // On non-unix we can't cheaply probe without extra deps; treat any recorded
    // pid as alive so we err on the side of refusing a second instance rather
    // than stomping a live one. (Windows is best-effort per engineering spec §22.)
    pid != 0
}

// Minimal FFI for `kill(2)` so we avoid pulling in the `libc`/`nix` crates for
// one syscall. Declared locally; linked from libc which is always present.
#[cfg(unix)]
const EPERM: i32 = 1;

#[cfg(unix)]
#[allow(unsafe_code)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "fleet-hub-test-{}-{}-{}.lock",
            tag,
            std::process::id(),
            nanos()
        ));
        p
    }

    fn nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn acquire_succeeds_on_fresh_path() {
        let p = temp_lock_path("fresh");
        let lock = InstanceLock::acquire(&p).expect("fresh acquire");
        assert!(p.exists());
        assert_eq!(read_pid(&p), Some(std::process::id()));
        drop(lock);
        assert!(!p.exists(), "drop removes the lockfile");
    }

    #[test]
    fn second_acquire_refuses_while_held() {
        let p = temp_lock_path("held");
        let _first = InstanceLock::acquire(&p).expect("first acquire");
        let second = InstanceLock::acquire(&p);
        match second {
            Err(LockError::AlreadyRunning { pid, .. }) => {
                assert_eq!(pid, std::process::id());
            }
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
    }

    #[test]
    fn reacquire_after_drop() {
        let p = temp_lock_path("redrop");
        {
            let _l = InstanceLock::acquire(&p).expect("first");
        }
        // After drop the lock is free again.
        let _l2 = InstanceLock::acquire(&p).expect("re-acquire after release");
    }

    #[test]
    fn stale_lock_from_dead_pid_is_reclaimed() {
        let p = temp_lock_path("stale");
        // Write a lock recording a pid that is essentially never alive.
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
            .unwrap();
        // Max u32 pid — not a live process.
        f.write_all(b"4294967294").unwrap();
        drop(f);
        // Acquire should reclaim the stale lock.
        let lock = InstanceLock::acquire(&p).expect("reclaim stale");
        assert_eq!(lock.pid(), std::process::id());
    }

    #[test]
    fn drop_does_not_remove_foreign_lock() {
        let p = temp_lock_path("foreign");
        let lock = InstanceLock::acquire(&p).expect("acquire");
        // Simulate another instance overwriting the lock with its pid.
        fs::write(&p, b"4294967294").unwrap();
        drop(lock);
        // Our drop must NOT have removed the foreign lock.
        assert!(p.exists(), "drop must not delete a lock we no longer own");
        let _ = fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn self_pid_is_alive() {
        assert!(pid_is_alive(std::process::id()));
        assert!(!pid_is_alive(0));
    }
}
