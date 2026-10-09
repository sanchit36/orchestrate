//! Thin wrapper over the `git` CLI.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Run `git <args>` in `dir`; the trimmed stdout on success, stderr in the error otherwise.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_string())
}

/// The default branch of the `origin` remote, or `None` when there is no `origin`.
/// Uses the local `origin/HEAD` when set, otherwise asks the remote (read only).
pub fn origin_default_branch(dir: &Path) -> Result<Option<String>> {
    let local = [
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ];
    if let Ok(head) = git(dir, &local) {
        return Ok(head.strip_prefix("origin/").map(String::from));
    }
    if !succeeds(dir, &["remote", "get-url", "origin"])? {
        return Ok(None);
    }
    let out = git(dir, &["ls-remote", "--symref", "origin", "HEAD"])
        .context("finding origin's default branch; set `protected_branches` to skip this")?;
    Ok(out
        .lines()
        .find_map(|l| l.strip_prefix("ref: refs/heads/")?.strip_suffix("\tHEAD"))
        .map(String::from))
}

/// The branch checked out in `dir`, or `None` on a detached `HEAD`.
pub fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
}

/// Each entry of `git status` in `dir`: tracked changes and untracked, not ignored, files.
pub fn changes(dir: &Path) -> Result<Vec<String>> {
    let out = git(
        dir,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )?;
    Ok(out.split_terminator('\0').map(String::from).collect())
}

/// Whether `git <args>` exits 0 in `dir`.
pub fn succeeds(dir: &Path, args: &[&str]) -> Result<bool> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .context("running git")?;
    Ok(out.status.success())
}
