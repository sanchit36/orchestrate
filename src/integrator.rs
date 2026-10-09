//! Integrator: rebases a ticket branch onto the integration branch, runs the gates
//! and merges it `--no-ff`.

use std::path::Path;

use anyhow::Result;

use crate::config::Gate;
use crate::gate;
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
    let changed = git(
        ticket_wt,
        &["diff", "-z", "--name-only", into.branch, "HEAD"],
    )?;
    let changed: Vec<String> = changed.split_terminator('\0').map(String::from).collect();
    if let Some(reason) = gate::run_all(gates, ticket_wt, &changed, logs, slug)? {
        return Ok(Landing::Failed(reason));
    }
    git(
        into.worktree,
        &["merge", "-q", "--no-ff", "-m", message, ticket_branch],
    )?;
    Ok(Landing::Merged(git(into.worktree, &["rev-parse", "HEAD"])?))
}
