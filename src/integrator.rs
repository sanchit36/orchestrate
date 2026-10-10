//! Integrator: rebases a ticket branch onto the integration branch, runs the gates
//! (through a conflict-resolution session when the rebase conflicts) and merges it
//! `--no-ff`.

use std::path::Path;

use anyhow::Result;

use crate::config::{Gate, SessionConfig};
use crate::gate;
use crate::git::{self, git, succeeds};
use crate::prompt;
use crate::session;
use crate::state::GateResult;

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
/// `<logs>/<slug>.gate-<name>.log`; each gate run's exit code goes to `results`.
/// A rebase conflict is handed to a session (output in `<logs>/<slug>.conflict.log`).
/// Gates check exactly the commit that merges, so a worktree holding changes beyond
/// `baseline` (its `git status` before the session) fails the ticket.
#[expect(clippy::too_many_arguments)]
pub fn land(
    into: &Integration,
    ticket_wt: &Path,
    ticket_branch: &str,
    baseline: &[String],
    gates: &[Gate],
    session: &SessionConfig,
    logs: &Path,
    slug: &str,
    message: &str,
    results: &mut Vec<GateResult>,
) -> Result<Landing> {
    if let Some(reason) = uncommitted(ticket_wt, baseline)? {
        return Ok(Landing::Failed(reason));
    }
    if !succeeds(ticket_wt, &["rebase", "-q", into.branch])? {
        let log = logs.join(format!("{slug}.conflict.log"));
        let failed = match resolve(into, ticket_wt, ticket_branch, session, &log)? {
            None => uncommitted(ticket_wt, baseline)?,
            failed => failed,
        };
        if let Some(reason) = failed {
            return Ok(Landing::Failed(reason));
        }
    }
    let changed = git(
        ticket_wt,
        &["diff", "-z", "--name-only", into.branch, "HEAD"],
    )?;
    let changed: Vec<String> = changed.split_terminator('\0').map(String::from).collect();
    if let Some(reason) = gate::run_all(gates, ticket_wt, &changed, logs, slug, results)? {
        return Ok(Landing::Failed(reason));
    }
    git(
        into.worktree,
        &["merge", "-q", "--no-ff", "-m", message, ticket_branch],
    )?;
    Ok(Landing::Merged(git(into.worktree, &["rev-parse", "HEAD"])?))
}

/// Why `wt` is not exactly its `HEAD` plus `baseline`, if it is not.
fn uncommitted(wt: &Path, baseline: &[String]) -> Result<Option<String>> {
    let left: Vec<String> = (git::changes(wt)?.into_iter())
        .filter(|c| !baseline.contains(c))
        // An entry is `XY <path>`.
        .map(|c| c.get(3..).unwrap_or_default().to_string())
        .collect();
    let reason = format!("uncommitted changes in the worktree: {}", left.join(", "));
    Ok((!left.is_empty()).then_some(reason))
}

/// Run a conflict-resolution session on the stopped rebase; `Some(reason)` when it
/// failed, with any unfinished rebase aborted so the ticket branch is left as it was.
fn resolve(
    into: &Integration,
    ticket_wt: &Path,
    ticket_branch: &str,
    session: &SessionConfig,
    log: &Path,
) -> Result<Option<String>> {
    let prompt = prompt::conflict(ticket_branch, into.branch);
    let mode = &session.permission_mode;
    let code = session::run(ticket_wt, &prompt, mode, session.timeout, log)?;
    // Succeeds only when the session left the rebase unfinished.
    let unfinished = succeeds(ticket_wt, &["rebase", "--abort"])?;
    let onto = ["merge-base", "--is-ancestor", into.branch, ticket_branch];
    let reason = match code {
        None => format!("conflict resolution timed out after {:?}", session.timeout),
        Some(code) if code != 0 => format!("conflict resolution exited {code}"),
        _ if unfinished => "conflict resolution left the rebase unfinished".into(),
        _ if !succeeds(ticket_wt, &onto)? => "conflict resolution did not rebase".into(),
        _ => return Ok(None),
    };
    Ok(Some(reason))
}
