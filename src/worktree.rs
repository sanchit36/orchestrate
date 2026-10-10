//! Worktree manager: creates and tears down worktrees and their branches under the
//! state dir. Nothing is ever checked out in the user's working copy.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::config::RepoConfig;
use crate::git::{git, succeeds};
use crate::process;
use crate::report::REPORT_FILE;

/// Held around `git worktree add`/`remove`: run at once on one repo, one can read the
/// other's half-written `.git/worktrees/<name>` and fail.
static WORKTREES: Mutex<()> = Mutex::new(());

/// Check out `branch` at `path`, creating the branch from `HEAD` if it does not exist.
pub fn integration(top: &Path, path: &Path, branch: &str) -> Result<()> {
    let exists = succeeds(
        top,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )?;
    if !exists {
        git(top, &["branch", branch, "HEAD"])?;
    }
    git(
        top,
        &["worktree", "add", "-q", &path.to_string_lossy(), branch],
    )?;
    Ok(())
}

/// A new `branch` from `base`, checked out at `path`, with the report file git-excluded.
pub fn create(top: &Path, path: &Path, branch: &str, base: &str) -> Result<()> {
    let path_str = path.to_string_lossy();
    let _held = WORKTREES.lock().unwrap_or_else(|e| e.into_inner());
    git(
        top,
        &["worktree", "add", "-q", "-b", branch, &path_str, base],
    )?;
    exclude_report(top)
}

/// Remove the worktree at `path` and delete `branch`.
pub fn remove(top: &Path, path: &Path, branch: &str) -> Result<()> {
    remove_worktree(top, path)?;
    git(top, &["branch", "-D", branch])?;
    Ok(())
}

/// Remove the worktree at `path`, keeping its branch.
pub fn remove_worktree(top: &Path, path: &Path) -> Result<()> {
    let _held = WORKTREES.lock().unwrap_or_else(|e| e.into_inner());
    git(
        top,
        &["worktree", "remove", "--force", &path.to_string_lossy()],
    )?;
    Ok(())
}

/// Make the fresh worktree at `wt` ready for its session: initialise submodules, copy
/// each seed file from the primary checkout `top`, then run `prepare` with its output
/// to `log`. Returns why preparation failed, if it did.
pub fn prepare(top: &Path, wt: &Path, config: &RepoConfig, log: &Path) -> Result<Option<String>> {
    if wt.join(".gitmodules").exists()
        && let Err(e) = git(wt, &["submodule", "update", "--init", "--recursive"])
    {
        return Ok(Some(format!("submodule init failed: {e:#}")));
    }
    for file in &config.seed_files {
        let (from, to) = (top.join(file), wt.join(file));
        if !from.is_file() {
            return Ok(Some(format!(
                "seeding failed: {} not found",
                file.display()
            )));
        }
        fs::create_dir_all(to.parent().unwrap())?;
        fs::copy(&from, &to).with_context(|| format!("seeding {}", file.display()))?;
    }
    let Some(cmd) = &config.prepare else {
        return Ok(None);
    };
    let out = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    // Bounded like the session it prepares for.
    let timeout = config.session.timeout;
    Ok(
        match process::sh(cmd, wt, &out, timeout).context("running prepare")? {
            Some(status) if status.success() => None,
            Some(status) => Some(format!("prepare exited {}", status.code().unwrap_or(-1))),
            None => Some(format!("prepare timed out after {timeout:?}")),
        },
    )
}

/// Git has no per-worktree exclude file, so the report is excluded in the shared
/// `info/exclude`, once.
fn exclude_report(top: &Path) -> Result<()> {
    let path = git(top, &["rev-parse", "--git-path", "info/exclude"])?;
    let path = top.join(path);
    let pattern = format!("/{REPORT_FILE}");
    let current = fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|l| l == pattern) {
        return Ok(());
    }
    fs::create_dir_all(path.parent().unwrap())?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    let sep = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    writeln!(file, "{sep}{pattern}")?;
    Ok(())
}
