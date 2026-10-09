//! Integrator: rebases a ticket branch onto the integration branch, runs the gates
//! and merges it `--no-ff`.

use std::fs::File;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::config::Gate;
use crate::git::{git, succeeds};

/// Where integration happens.
pub struct Integration<'a> {
    pub worktree: &'a Path,
    pub branch: &'a str,
}

pub enum Landing {
    /// The merge commit on the integration branch.
    Merged(String),
    /// Why the ticket did not land.
    Failed(String),
}

/// Land the ticket branch checked out at `ticket_wt`. Gate output goes to
/// `<logs>/<slug>.gate-<name>.log`.
pub fn land(
    into: &Integration,
    ticket_wt: &Path,
    ticket_branch: &str,
    gates: &[Gate],
    logs: &Path,
    slug: &str,
    message: &str,
) -> Result<Landing> {
    if !succeeds(ticket_wt, &["rebase", "-q", into.branch])? {
        git(ticket_wt, &["rebase", "--abort"])?;
        return Ok(Landing::Failed("rebase conflict".into()));
    }
    for gate in gates {
        let log = logs.join(format!("{slug}.gate-{}.log", gate.name));
        let out = File::create(&log).with_context(|| format!("creating {}", log.display()))?;
        let status = Command::new("sh")
            .args(["-c", &gate.run])
            .current_dir(ticket_wt)
            .stdout(out.try_clone()?)
            .stderr(out)
            .status()
            .with_context(|| format!("running gate {}", gate.name))?;
        if !status.success() {
            let code = status.code().unwrap_or(-1);
            return Ok(Landing::Failed(format!("gate {} exited {code}", gate.name)));
        }
    }
    git(
        into.worktree,
        &["merge", "-q", "--no-ff", "-m", message, ticket_branch],
    )?;
    Ok(Landing::Merged(git(into.worktree, &["rev-parse", "HEAD"])?))
}
