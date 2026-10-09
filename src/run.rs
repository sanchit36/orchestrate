//! Run loop: re-reads the tracker, works the frontier one session at a time and
//! lands each ticket's gated work on the integration branch.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, bail};

use crate::config::{self, RepoConfig};
use crate::git::git;
use crate::integrator::{self, Integration, Landing};
use crate::report::Report;
use crate::reporting::Events;
use crate::spec::{self, Reading};
use crate::state::{self, RunDirs, State, Store, TicketState};
use crate::tracker::{self, Pausing, Ticket, Tracker};
use crate::{planner, prompt, session, worktree};

pub fn run(top: &Path, spec_id: &str, branch: &str) -> Result<()> {
    let (config, machine) = config::load(top)?;
    if config.gates.is_empty() {
        bail!("no gate configured; add a [[gates]] entry to .orchestrate/config.toml");
    }
    let headings = &config.criteria_headings;
    // Refuse a spec that cannot start before anything is created.
    planner::plan(&spec::read(
        tracker::open(&config, top).as_ref(),
        spec_id,
        headings,
    )?)?;

    session::kill_sessions_on_interrupt()?;
    let dirs = RunDirs::create(&machine, top, spec_id)?;
    let mut events = Events::open(&dirs.logs.join("run.log"))?;
    let store = Store::create(&dirs, State::new(&dirs.id, spec_id, branch))?;
    let integration_wt = dirs.worktrees.join("_integration");
    worktree::integration(top, &integration_wt, branch)?;
    events.event(&format!(
        "run {} started: spec {spec_id}, integration branch {branch}",
        dirs.id
    ));

    // The tracker reads and writes back in the integration worktree, never the user's checkout.
    // A tracker failure pauses the run until the tracker answers again.
    let tracker = Pausing::new(
        tracker::open(&config, &integration_wt),
        &config.tracker,
        Events::open(&dirs.logs.join("run.log"))?,
        Box::new(|paused| store.update(|s| s.paused = paused)),
    );
    let into = Integration {
        worktree: &integration_wt,
        branch,
    };
    let run = Run {
        top,
        config: &config,
        dirs: &dirs,
        into: &into,
        tracker: &tracker,
        store: &store,
    };
    let mut tried = HashSet::new();
    loop {
        let reading = spec::reread(&tracker, spec_id, headings)?;
        store.update(|state| {
            for t in reading.tickets.iter().map(|e| &e.ticket) {
                let seen = state
                    .tickets
                    .entry(t.id.clone())
                    .or_insert_with(|| TicketState {
                        outcome: if t.finished { "finished" } else { "pending" }.into(),
                        ..TicketState::default()
                    });
                seen.title = t.title.clone();
            }
        })?;
        let next = planner::plan(&reading)?
            .frontier
            .into_iter()
            .find(|t| !tried.contains(&t.id));
        let Some(ticket) = next else { break };
        tried.insert(ticket.id.clone());
        run.ticket(&ticket.id, |t| {
            t.outcome = "running".into();
            t.started = Some(jiff::Timestamp::now().to_string());
        })?;
        let outcome = run.work(&reading, ticket, &mut events)?;
        store.update(|state| {
            let kind = outcome.kind;
            set_outcome(state, &ticket.id, outcome);
            if kind != "finished" {
                block_dependents(state, &reading, &ticket.id, kind, &mut events);
            }
        })?;
    }

    let counts = store.update(|state| {
        state.state = "ended".into();
        let count = |o: &str| state.tickets.values().filter(|t| t.outcome == o).count();
        (
            count("finished"),
            count("failed") + count("timeout"),
            count("stuck"),
        )
    })?;
    events.event(&format!(
        "run {} ended: {} finished, {} failed, {} stuck; integration branch {branch}",
        dirs.id, counts.0, counts.1, counts.2,
    ));
    events.event(&format!("push it with: git push -u origin {branch}"));
    Ok(())
}

/// How a ticket's work ended: `finished`, `failed`, `timeout` (a kind of failed) or
/// `stuck`, with the cause.
struct Outcome {
    kind: &'static str,
    reason: Option<String>,
    unmet: Vec<String>,
}

impl Outcome {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            reason: None,
            unmet: Vec::new(),
        }
    }

    fn because(kind: &'static str, reason: String) -> Self {
        Self {
            reason: Some(reason),
            ..Self::new(kind)
        }
    }
}

fn set_outcome(state: &mut State, id: &str, outcome: Outcome) {
    if let Some(t) = state.tickets.get_mut(id) {
        t.outcome = outcome.kind.into();
        t.reason = outcome.reason;
        t.unmet = outcome.unmet;
    }
}

/// Every pending ticket blocked, directly or not, by `id` becomes `held` behind a stuck
/// ticket or `skipped` behind a failed one.
fn block_dependents(
    state: &mut State,
    reading: &Reading,
    id: &str,
    kind: &str,
    events: &mut Events,
) {
    let (outcome, cause) = match kind {
        "stuck" => ("held", "stuck"),
        _ => ("skipped", "failed"),
    };
    let reason = format!("blocked by {cause} {id}");
    let mut blocked = HashSet::from([id.to_string()]);
    loop {
        let next = reading.tickets.iter().find(|e| {
            let pending = state
                .tickets
                .get(&e.ticket.id)
                .is_some_and(|t| t.outcome == "pending");
            pending && e.blockers.iter().any(|b| blocked.contains(&b.id))
        });
        let Some(e) = next else { break };
        let t = &e.ticket;
        set_outcome(state, &t.id, Outcome::because(outcome, reason.clone()));
        events.event(&format!("ticket {} {outcome}: {reason}", t.id));
        blocked.insert(t.id.clone());
    }
}

struct Run<'a> {
    top: &'a Path,
    config: &'a RepoConfig,
    dirs: &'a RunDirs,
    into: &'a Integration<'a>,
    tracker: &'a dyn Tracker,
    store: &'a Store,
}

impl Run<'_> {
    /// Change ticket `id`'s recorded state.
    fn ticket(&self, id: &str, f: impl FnOnce(&mut TicketState)) -> Result<()> {
        self.store.update(|s| s.tickets.get_mut(id).map(f))?;
        Ok(())
    }

    /// Work one ticket from worktree to landing; returns its outcome.
    fn work(&self, reading: &Reading, ticket: &Ticket, events: &mut Events) -> Result<Outcome> {
        let slug = state::slug(&ticket.id);
        let wt = self.dirs.worktrees.join(&slug);
        let branch = format!("orchestrate/{}/{slug}", self.dirs.id);
        worktree::create(self.top, &wt, &branch, self.into.branch)?;
        self.ticket(&ticket.id, |t| t.worktree = Some(wt.clone()))?;
        events.event(&format!("ticket {} started: {}", ticket.id, ticket.title));
        let prepare_log = self.dirs.logs.join(format!("{slug}.prepare.log"));
        if let Some(reason) = worktree::prepare(self.top, &wt, self.config, &prepare_log)? {
            events.event(&format!("ticket {} failed: {reason}", ticket.id));
            return Ok(Outcome::because("failed", reason));
        }

        let session = &self.config.session;
        let prompt = prompt::render(session, self.top, &reading.spec.body, &ticket.body, &branch)?;
        let log = self.dirs.logs.join(format!("{slug}.session.log"));
        let mode = &session.permission_mode;
        let Some(code) = session::run(&wt, &prompt, mode, session.timeout, &log)? else {
            let reason = format!("no exit after {:?}", session.timeout);
            events.event(&format!("ticket {} timeout: {reason}", ticket.id));
            return Ok(Outcome::because("timeout", reason));
        };

        let new_commits = git(
            &wt,
            &[
                "rev-list",
                "--count",
                &format!("{}..HEAD", self.into.branch),
            ],
        )?;
        let report = Report::read(&wt);
        if let Ok(r) = &report {
            self.ticket(&ticket.id, |t| {
                t.criteria = r.acceptance_criteria.clone();
                t.notes = r.notes.clone();
            })?;
        }
        let failure = match report {
            _ if code != 0 => Some(format!("session exited {code}")),
            Err(e) => Some(format!("{e:#}")),
            Ok(report) if !report.is_done() => return Ok(stuck(ticket, report, events)),
            Ok(_) if new_commits == "0" => Some("no new commits".into()),
            Ok(_) => None,
        };
        let landing = match failure {
            Some(reason) => Landing::Failed(reason),
            None => {
                self.ticket(&ticket.id, |t| t.outcome = "integrating".into())?;
                let message = format!("Merge ticket {}: {}", ticket.id, ticket.title);
                let mut results = Vec::new();
                let landing = integrator::land(
                    self.into,
                    &wt,
                    &branch,
                    &self.config.gates,
                    &self.dirs.logs,
                    &slug,
                    &message,
                    &mut results,
                )?;
                self.ticket(&ticket.id, |t| t.gates = results)?;
                landing
            }
        };
        match landing {
            Landing::Merged(sha) => {
                self.tracker.mark_finished(&ticket.id)?;
                worktree::remove(self.top, &wt, &branch)?;
                events.event(&format!(
                    "ticket {} finished: merged {}",
                    ticket.id,
                    &sha[..12]
                ));
                Ok(Outcome::new("finished"))
            }
            Landing::Failed(reason) => {
                events.event(&format!("ticket {} failed: {reason}", ticket.id));
                Ok(Outcome::because("failed", reason))
            }
        }
    }
}

/// A stuck session: its notes are the reason; unmet criteria are kept beside them.
fn stuck(ticket: &Ticket, report: Report, events: &mut Events) -> Outcome {
    let unmet = report.unmet();
    let notes = report.notes.trim();
    let mut cause: Vec<String> = notes.lines().next().map(String::from).into_iter().collect();
    if !unmet.is_empty() {
        cause.push(format!("unmet: {}", unmet.join(", ")));
    }
    events.event(&format!("ticket {} stuck: {}", ticket.id, cause.join("; ")));
    Outcome {
        kind: "stuck",
        reason: (!notes.is_empty()).then(|| notes.to_string()),
        unmet,
    }
}
