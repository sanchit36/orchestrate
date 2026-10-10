//! Repo lock: one live run per repo. The lock file in the per-repo state dir holds
//! the live run's pid and run id, and is removed when the run lets go of it.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Holder {
    pub pid: i32,
    pub run: String,
}

/// Held by the live run; dropping it removes the lock file.
pub struct Lock {
    pub path: PathBuf,
    /// The notice of a lock taken over from a dead run, for the run to report.
    pub taken_over: Option<String>,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn path(repo_dir: &Path) -> PathBuf {
    repo_dir.join("lock")
}

/// Env var holding the pid of a `--detach` launcher, for the run it launched.
pub const LAUNCHER: &str = "ORCHESTRATE_LAUNCHER";

/// Where a detached run copies its lock for its launcher: a fast run can take and let
/// go of the lock between two polls, but this copy stays until the launcher takes it.
fn started(repo_dir: &Path, launcher: u32) -> PathBuf {
    repo_dir.join(format!("started.{launcher}"))
}

/// Take the lock for `run` in this process. Refuses while a live process holds it;
/// takes over, with a notice, a lock whose pid is gone.
pub fn acquire(repo_dir: &Path, run: &str) -> Result<Lock> {
    let lock = path(repo_dir);
    let pid = std::process::id() as i32;
    // Write aside, then hard-link into place: creation is atomic and never half-written.
    let tmp = repo_dir.join(format!("lock.{pid}"));
    let holder = Holder {
        pid,
        run: run.into(),
    };
    fs::write(&tmp, toml::to_string(&holder)?)?;
    let mut taken_over = None;
    let taken = loop {
        match fs::hard_link(&tmp, &lock) {
            Ok(()) => {
                let lock = Lock {
                    path: lock,
                    taken_over,
                };
                // Only the launcher's own child answers it, not runs nested under that run.
                let parent = unsafe { libc::getppid() } as u32;
                if std::env::var(LAUNCHER).is_ok_and(|v| v == parent.to_string()) {
                    let started = started(repo_dir, parent);
                    let _ = fs::remove_file(&started);
                    if let Err(e) = fs::hard_link(&tmp, &started) {
                        break Err(e).with_context(|| format!("creating {}", started.display()));
                    }
                }
                break Ok(lock);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => break Err(e).with_context(|| format!("creating {}", lock.display())),
        }
        let Some(old) = read(repo_dir)? else { continue };
        if alive(old.pid) {
            let _ = fs::remove_file(&tmp);
            bail!(
                "run {} is live in this repo (pid {}); stop it with `orchestrate stop`",
                old.run,
                old.pid
            );
        }
        taken_over = Some(format!(
            "taking over the lock of run {}: its pid {} is gone",
            old.run, old.pid
        ));
        match fs::remove_file(&lock) {
            Err(e) if e.kind() != ErrorKind::NotFound => break Err(e.into()),
            _ => {}
        }
    };
    let _ = fs::remove_file(&tmp);
    taken
}

/// The holder of the run this process launched with `--detach`, once it took the lock.
pub fn take_started(repo_dir: &Path) -> Result<Option<Holder>> {
    let started = started(repo_dir, std::process::id());
    let holder = read_file(&started)?;
    if holder.is_some() {
        let _ = fs::remove_file(&started);
    }
    Ok(holder)
}

/// The lock's holder, if there is a lock file.
pub fn read(repo_dir: &Path) -> Result<Option<Holder>> {
    read_file(&path(repo_dir))
}

fn read_file(lock: &Path) -> Result<Option<Holder>> {
    match fs::read_to_string(lock) {
        Ok(s) => Ok(Some(
            toml::from_str(&s).with_context(|| format!("reading {}", lock.display()))?,
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", lock.display())),
    }
}

/// The holder of the lock, if its process is still alive.
pub fn live(repo_dir: &Path) -> Result<Option<Holder>> {
    Ok(read(repo_dir)?.filter(|h| alive(h.pid)))
}

fn alive(pid: i32) -> bool {
    // Signal 0 checks existence only; EPERM means it exists under another user.
    pid > 0
        && (unsafe { libc::kill(pid, 0) == 0 }
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}
