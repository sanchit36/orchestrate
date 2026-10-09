//! Worktree manager: creates and tears down worktrees and their branches under the
//! state dir. Nothing is ever checked out in the user's working copy.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::git::{git, succeeds};
use crate::report::REPORT_FILE;

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
    git(
        top,
        &["worktree", "add", "-q", "-b", branch, &path_str, base],
    )?;
    exclude_report(top)
}

/// Remove the worktree at `path` and delete `branch`.
pub fn remove(top: &Path, path: &Path, branch: &str) -> Result<()> {
    git(
        top,
        &["worktree", "remove", "--force", &path.to_string_lossy()],
    )?;
    git(top, &["branch", "-D", branch])?;
    Ok(())
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
