//! Integrator: rebases a ticket branch onto the integration branch, runs the gates
//! (through a conflict-resolution session when the rebase conflicts) and merges it
//! `--no-ff`.

use std::path::Path;

use anyhow::Result;

use crate::config::{Gate, SessionConfig};
use crate::gate;
use crate::git::{git, succeeds};
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
#[expect(clippy::too_many_arguments)]
pub fn land(
    into: &Integration,
    ticket_wt: &Path,
    ticket_branch: &str,
    gates: &[Gate],
    session: &SessionConfig,
    logs: &Path,
    slug: &str,
    message: &str,
    results: &mut Vec<GateResult>,
) -> Result<Landing> {
    if !succeeds(ticket_wt, &["rebase", "-q", into.branch])? {
        let log = logs.join(format!("{slug}.conflict.log"));
        if let Some(reason) = resolve(into, ticket_wt, ticket_branch, session, &log)? {
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
