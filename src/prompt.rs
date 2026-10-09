//! The built-in, agent-neutral session prompt.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::SessionConfig;
use crate::report::REPORT_FILE;

/// Render the prompt for one ticket. The repo's instructions file, if any, is
/// appended; it never replaces the template.
pub fn render(
    session: &SessionConfig,
    top: &Path,
    spec_body: &str,
    ticket_body: &str,
    branch: &str,
) -> Result<String> {
    let mut prompt = format!(
        "You are working one ticket of a spec, unattended, in a git worktree of this repo.
Follow the repo's AGENTS.md / CLAUDE.md.
Work only on branch `{branch}`, which is already checked out here. Commit your work on it.
Do not push, do not switch branches, and never read or write the issue tracker.

# Spec

{spec_body}

# Ticket

{ticket_body}

# Report

When you finish, write `{REPORT_FILE}` at the root of this worktree (it is git-excluded; do not commit it):

{{
  \"status\": \"done | stuck\",
  \"acceptance_criteria\": [{{ \"text\": \"...\", \"met\": true, \"evidence\": \"...\" }}],
  \"notes\": \"...\"
}}

Use `done` only when every acceptance criterion is met; list each criterion with its evidence.
Use `stuck` when you cannot finish, and say why in `notes`.
"
    );
    if !session.skills.is_empty() {
        prompt.push_str(&format!(
            "\nUse these skills where they apply: {}.\n",
            session.skills.join(", ")
        ));
    }
    if let Some(file) = &session.instructions {
        let path = top.join(file);
        let extra =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        prompt.push_str(&format!("\n# Repo instructions\n\n{extra}"));
    }
    Ok(prompt)
}
