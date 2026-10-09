//! `resume`: continue a stopped or crashed run once its state matches reality.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, bail};

use crate::config::{self, RepoConfig};
use crate::git;
use crate::report;
use crate::reporting::Events;
use crate::run::{self, gate_lines};
use crate::spec;
use crate::state::{self, RunDirs, State, Store};
use crate::tracker::Tracker;
use crate::writeback;

pub fn resume(top: &Path, run: Option<&str>) -> Result<()> {
    let (config, machine) = config::load(top)?;
    run::check_tracker(top, &config)?;
    let repo_dir = state::repo_dir(&machine, top)?;
    let recorded = state::resolve(state::runs(&repo_dir)?, run)?;
    run::preflight(top, &config, &recorded.branch)?;
    let id = recorded.run_id;
    let dirs = RunDirs::open(&repo_dir, id)?;
    // Read again under the lock: a run that was live a moment ago has saved its last state.
    let mut state = State::load(&dirs.run)?;
    if state.status == "ended" {
        bail!(
            "run {} has ended; start a new one with `orchestrate run`",
            dirs.id
        );
    }
    (state.status, state.paused) = ("running".into(), None);
    let started = [format!(
        "run {} resumed: spec {}, integration branch {}",
        dirs.id, state.spec, state.branch
    )];
    let integration = dirs.worktrees.join("_integration");
    let spec_id = state.spec.clone();
    run::drive(
        top,
        &config,
        &machine,
        machine.max_parallel,
        dirs,
        state,
        &started,
        |tracker, cx, store, events| {
            reconcile(tracker, cx, store, events, &config, &integration, &spec_id)
        },
    )
}

/// Bring the run's recorded tickets in line with the integration branch and the
/// tracker; returns the tickets not to work again.
fn reconcile(
    tracker: &dyn Tracker,
    cx: &writeback::Context,
    store: &Store,
    events: &mut Events,
    config: &RepoConfig,
    integration: &Path,
    spec_id: &str,
) -> Result<HashSet<String>> {
    let reading = spec::reread(tracker, spec_id, &config.criteria_headings)?;
    let recorded: Vec<(String, String, Option<String>)> = store.update(|s| {
        (s.tickets.iter())
            .map(|(id, t)| (id.clone(), t.outcome.clone(), t.merged.clone()))
            .collect()
    })?;
    let mut tried = HashSet::new();
    for (id, outcome, merged) in recorded {
        let entry = reading.tickets.iter().find(|e| e.ticket.id == id);
        let on_tracker = entry.is_some_and(|e| e.ticket.finished);
        let (kind, reason) = match merged {
            Some(sha)
                if !git::succeeds(integration, &["merge-base", "--is-ancestor", &sha, "HEAD"])? =>
            {
                let reason = format!(
                    "merge {} is missing from the integration branch",
                    &sha[..12]
                );
                ("failed", Some(reason))
            }
            Some(_) if outcome == "finished" => ("finished", None),
            // Landed, but the run went before the tracker heard of it.
            Some(sha) => {
                if !on_tracker {
                    let criteria: Vec<String> = entry
                        .iter()
                        .flat_map(|e| &e.criteria)
                        .map(|c| c.text.clone())
                        .collect();
                    let (criteria, gates) = store.update(|s| {
                        let t = &s.tickets[&id];
                        let met = report::met(&t.criteria, &criteria);
                        (met, gate_lines(&config.gates, &t.gates))
                    })?;
                    let written = writeback::landed(tracker, cx, &id, &criteria, &sha, &gates)?;
                    store.update(|s| {
                        let t = s.tickets.get_mut(&id).unwrap();
                        (t.mismatches, t.writeback) = (written.mismatches, written.problems);
                    })?;
                }
                ("finished", Some(format!("merged {}", &sha[..12])))
            }
            None if outcome != "finished" && on_tracker => {
                ("finished", Some("finished on the tracker".into()))
            }
            None if outcome == "running" || outcome == "integrating" => {
                // Failed, but not tried: it restarts in a fresh worktree.
                let reason = "its process is gone".to_string();
                events.event(&format!("ticket {id} failed: {reason}"));
                store.update(|s| {
                    let t = s.tickets.get_mut(&id).unwrap();
                    (t.outcome, t.reason) = ("failed".into(), Some(reason));
                })?;
                continue;
            }
            None => (outcome.as_str(), None),
        };
        if kind != outcome {
            events.event(&format!(
                "ticket {id} {kind}: {}",
                reason.as_deref().unwrap_or_default()
            ));
            store.update(|s| {
                let t = s.tickets.get_mut(&id).unwrap();
                t.outcome = kind.into();
                if kind == "failed" {
                    t.reason = reason;
                }
            })?;
        }
        // Held and skipped tickets start again once their blockers are finished.
        if !["pending", "held", "skipped"].contains(&kind) {
            tried.insert(id);
        }
    }
    Ok(tried)
}
