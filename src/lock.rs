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
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn path(repo_dir: &Path) -> PathBuf {
    repo_dir.join("lock")
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
    let taken = loop {
        match fs::hard_link(&tmp, &lock) {
            Ok(()) => break Ok(Lock { path: lock }),
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
        eprintln!(
            "orchestrate: taking over the lock of run {}: its pid {} is gone",
            old.run, old.pid
        );
        match fs::remove_file(&lock) {
            Err(e) if e.kind() != ErrorKind::NotFound => break Err(e.into()),
            _ => {}
        }
    };
    let _ = fs::remove_file(&tmp);
    taken
}

/// The lock's holder, if there is a lock file.
pub fn read(repo_dir: &Path) -> Result<Option<Holder>> {
    let lock = path(repo_dir);
    match fs::read_to_string(&lock) {
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
