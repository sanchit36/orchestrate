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

/// Whether `git <args>` exits 0 in `dir`.
pub fn succeeds(dir: &Path, args: &[&str]) -> Result<bool> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .context("running git")?;
    Ok(out.status.success())
}
