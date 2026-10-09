//! Reporting from `state.json` alone: `status`, `status <ticket>` and `runs`. Read-only.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config;
use crate::state::{self, Outcome, RunStatus, State, TicketState};

/// Outcomes in the order `status` counts them; a timeout counts as failed.
const OUTCOMES: &[Outcome] = &[
    Outcome::Pending,
    Outcome::Running,
    Outcome::Integrating,
    Outcome::Finished,
    Outcome::Failed,
    Outcome::Stuck,
    Outcome::Held,
    Outcome::Skipped,
];

/// `status [<ticket>] [--run ID]`.
pub fn status(top: &Path, run: Option<&str>, ticket: Option<&str>) -> Result<()> {
    let repo_dir = repo_dir(top)?;
    let state = state::resolve(state::runs(&repo_dir)?, run)?;
    let out = match ticket {
        None => overview(&state),
        Some(id) => {
            let t = state
                .tickets
                .get(id)
                .with_context(|| format!("no ticket {id} in run {}", state.run_id))?;
            let logs = repo_dir.join("runs").join(&state.run_id).join("logs");
            detail(id, t, &logs)?
        }
    };
    print!("{out}");
    Ok(())
}

/// `runs`: this repo's runs, most recent first.
pub fn runs(top: &Path) -> Result<()> {
    let runs = state::runs(&repo_dir(top)?)?;
    if runs.is_empty() {
        println!("no runs in this repo");
    }
    for s in runs {
        println!("{}  spec {}  {}", s.run_id, s.spec, run_state(&s));
    }
    Ok(())
}

fn repo_dir(top: &Path) -> Result<PathBuf> {
    let (_, machine) = config::load(top)?;
    state::repo_dir(&machine, top)
}

fn overview(s: &State) -> String {
    let mut out = format!(
        "Run {}: spec {}, integration branch {}, {}\n",
        s.run_id,
        s.spec,
        s.branch,
        run_state(s)
    );
    let counts: Vec<String> = OUTCOMES
        .iter()
        .map(|o| (o, s.tickets.values().filter(|t| shown(t) == *o).count()))
        .filter(|(_, n)| *n > 0)
        .map(|(o, n)| format!("{n} {o}"))
        .collect();
    out.push_str(&counts.join(", "));
    out.push_str("\n\n");
    for (id, t) in &s.tickets {
        let _ = writeln!(out, "  {id}  {}  {}", t.title, outcome(t));
    }
    out
}

fn detail(id: &str, t: &TicketState, logs: &Path) -> Result<String> {
    let mut out = format!("Ticket {id}: {}\nOutcome: {}\n", t.title, outcome(t));
    if let Some(reason) = t.reason.as_ref().filter(|r| **r != t.notes) {
        let _ = writeln!(out, "Reason: {reason}");
    }
    if !t.criteria.is_empty() {
        out.push_str("Criteria:\n");
    }
    for c in &t.criteria {
        let mark = if c.met { "x" } else { " " };
        let _ = writeln!(out, "  [{mark}] {}: {}", c.text, c.evidence);
    }
    if !t.notes.is_empty() {
        out.push_str("Notes:\n");
        for line in t.notes.lines() {
            let _ = writeln!(out, "  {line}");
        }
    }
    if !t.gates.is_empty() {
        out.push_str("Gates:\n");
    }
    for g in &t.gates {
        let _ = writeln!(out, "  {}: {}", g.name, g.ended());
    }
    if !t.mismatches.is_empty() {
        out.push_str("Mismatches:\n");
    }
    for m in &t.mismatches {
        let _ = writeln!(out, "  {m}");
    }
    let Some(wt) = &t.worktree else {
        return Ok(out);
    };
    let gone = if wt.exists() { "" } else { " (removed)" };
    let _ = writeln!(out, "Worktree: {}{gone}", wt.display());
    // Logs are named after the worktree; names never contain a `.` before the suffix.
    let prefix = format!("{}.", wt.file_name().unwrap_or_default().to_string_lossy());
    let mut paths: Vec<PathBuf> = fs::read_dir(logs)
        .with_context(|| format!("reading {}", logs.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
        })
        .collect();
    paths.sort();
    if !paths.is_empty() {
        out.push_str("Logs:\n");
    }
    for p in paths {
        let _ = writeln!(out, "  {}", p.display());
    }
    Ok(out)
}

/// `running`, `paused (<reason>)`, `stopped` or `ended`.
fn run_state(s: &State) -> String {
    match &s.paused {
        Some(reason) if s.status == RunStatus::Running => format!("paused ({reason})"),
        _ => s.status.to_string(),
    }
}

/// The outcome as counted: a timeout is a kind of failure.
fn shown(t: &TicketState) -> Outcome {
    match t.outcome {
        Outcome::Timeout => Outcome::Failed,
        o => o,
    }
}

/// The outcome with its cause or elapsed time.
fn outcome(t: &TicketState) -> String {
    let reason = t.reason.as_deref().unwrap_or_default();
    match t.outcome {
        Outcome::Running => format!("running ({})", elapsed(t)),
        Outcome::Timeout => "failed (timeout)".into(),
        Outcome::Failed => format!("failed ({})", cause(reason)),
        o @ (Outcome::Held | Outcome::Skipped) if !reason.is_empty() => format!("{o} ({reason})"),
        o => o.to_string(),
    }
}

/// A failure's cause: `prepare`, `gate <name>`, `conflict`, or the reason itself.
fn cause(reason: &str) -> String {
    let prepare = ["submodule init failed", "seeding failed", "prepare "];
    if prepare.iter().any(|p| reason.starts_with(p)) {
        return "prepare".into();
    }
    if reason.starts_with("gate ") {
        return reason.split(' ').take(2).collect::<Vec<_>>().join(" ");
    }
    if reason.contains("conflict") {
        return "conflict".into();
    }
    reason.lines().next().unwrap_or_default().into()
}

/// Time since the ticket's session started, to the second.
fn elapsed(t: &TicketState) -> String {
    let started = t
        .started
        .as_deref()
        .and_then(|s| s.parse::<jiff::Timestamp>().ok());
    let Some(started) = started else {
        return "?".into();
    };
    let secs = jiff::Timestamp::now().duration_since(started).as_secs();
    format!("{:#}", jiff::SignedDuration::from_secs(secs))
}
