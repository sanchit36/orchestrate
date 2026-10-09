//! Working one ticket on its own thread: worktree, preparation, session, report, and
//! landing on the integration branch under the integration lock, then write-back.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;

use crate::config::{Gate, RepoConfig};
use crate::events::Events;
use crate::git::{self, git};
use crate::integrator::{self, Integration, Landing};
use crate::report::{self, Report};
use crate::spec::Reading;
use crate::state::{self, GateResult, Outcome, RunDirs, Store, TicketState};
use crate::tracker::{Ticket, Tracker};
use crate::writeback::{self, Written};
use crate::{prompt, session, worktree};

/// How a ticket's work ended: finished, failed, timeout (a kind of failed) or stuck,
/// with the cause.
pub struct Ending {
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub unmet: Vec<String>,
    pub written: Written,
}

impl Ending {
    fn new(outcome: Outcome) -> Self {
        Self {
            outcome,
            reason: None,
            unmet: Vec::new(),
            written: Written::default(),
        }
    }

    pub fn because(outcome: Outcome, reason: String) -> Self {
        Self {
            reason: Some(reason),
            ..Self::new(outcome)
        }
    }
}

/// What a ticket's thread shares with the rest of the run.
pub struct Work<'a> {
    pub top: &'a Path,
    pub config: &'a RepoConfig,
    pub dirs: &'a RunDirs,
    pub into: &'a Integration<'a>,
    pub tracker: &'a dyn Tracker,
    pub cx: &'a writeback::Context<'a>,
    pub store: &'a Store,
    /// The integration lock: one landing (through write-back and teardown) at a time.
    pub landing: &'a Mutex<()>,
}

impl Work<'_> {
    /// Work one ticket and write its outcome back to the tracker.
    pub fn ticket(
        &self,
        reading: &Reading,
        ticket: &Ticket,
        events: &mut Events,
    ) -> Result<Ending> {
        let mut ending = self.attempt(reading, ticket, events)?;
        if ending.outcome != Outcome::Finished {
            // The tracker may commit in the integration worktree, so wait out any landing.
            let _landing = self.landing.lock().unwrap_or_else(|e| e.into_inner());
            let (outcome, reason) = (ending.outcome, ending.reason.as_deref());
            ending.written = writeback::outcome(self.tracker, self.cx, &ticket.id, outcome, reason);
        }
        Ok(ending)
    }

    /// Change ticket `id`'s recorded state.
    fn record(&self, id: &str, f: impl FnOnce(&mut TicketState)) -> Result<()> {
        self.store.update(|s| s.tickets.get_mut(id).map(f))?;
        Ok(())
    }

    /// `slug`, else `slug-2`, `slug-3`, ...: the first name whose worktree and branch are
    /// both free, so a restarted ticket keeps its old ones.
    fn fresh(&self, slug: String) -> Result<String> {
        let mut name = slug.clone();
        for n in 2.. {
            let branch = format!("refs/heads/orchestrate/{}/{name}", self.dirs.id);
            let taken = git::succeeds(self.top, &["rev-parse", "--verify", "--quiet", &branch])?;
            if !taken && !self.dirs.worktrees.join(&name).exists() {
                break;
            }
            name = format!("{slug}-{n}");
        }
        Ok(name)
    }

    /// Work one ticket from worktree to landing; returns how it ended.
    fn attempt(&self, reading: &Reading, ticket: &Ticket, events: &mut Events) -> Result<Ending> {
        let slug = self.fresh(state::slug(&ticket.id))?;
        let wt = self.dirs.worktrees.join(&slug);
        let branch = format!("orchestrate/{}/{slug}", self.dirs.id);
        worktree::create(self.top, &wt, &branch, self.into.branch)?;
        self.record(&ticket.id, |t| t.worktree = Some(wt.clone()))?;
        events.event(&format!("ticket {} started: {}", ticket.id, ticket.title));
        let prepare_log = self.dirs.logs.join(format!("{slug}.prepare.log"));
        if let Some(reason) = worktree::prepare(self.top, &wt, self.config, &prepare_log)? {
            events.event(&format!("ticket {} failed: {reason}", ticket.id));
            return Ok(Ending::because(Outcome::Failed, reason));
        }
        // Seeded and prepared files are not the session's work, so landing allows them.
        let baseline = git::changes(&wt)?;

        let session = &self.config.session;
        let prompt = prompt::render(session, self.top, &reading.spec.body, &ticket.body, &branch)?;
        let log = self.dirs.logs.join(format!("{slug}.session.log"));
        let mode = &session.permission_mode;
        let Some(code) = session::run(&wt, &prompt, mode, session.timeout, &log)? else {
            let reason = format!("no exit after {:?}", session.timeout);
            events.event(&format!("ticket {} timeout: {reason}", ticket.id));
            return Ok(Ending::because(Outcome::Timeout, reason));
        };

        let since = format!("{}..HEAD", self.into.branch);
        let new_commits = git(&wt, &["rev-list", "--count", &since])?;
        // The criteria read at dispatch: a report must cover every one of them.
        let criteria: Vec<String> = (reading.tickets.iter())
            .filter(|e| e.ticket.id == ticket.id)
            .flat_map(|e| &e.criteria)
            .map(|c| c.text.clone())
            .collect();
        let report = Report::read(&wt).map(|mut r| {
            r.cover(&criteria);
            r
        });
        let mut reported = Vec::new();
        if let Ok(r) = &report {
            reported = r.acceptance_criteria.clone();
            self.record(&ticket.id, |t| {
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
        let _landing = self.landing.lock().unwrap_or_else(|e| e.into_inner());
        let mut gates = Vec::new();
        let landing = match failure {
            Some(reason) => Landing::Failed(reason),
            None => {
                self.record(&ticket.id, |t| t.outcome = Outcome::Integrating)?;
                let message = format!("Merge ticket {}: {}", ticket.id, ticket.title);
                let mut results = Vec::new();
                let landing = integrator::land(
                    self.into,
                    &wt,
                    &branch,
                    &baseline,
                    &self.config.gates,
                    &self.config.session,
                    &self.dirs.logs,
                    &slug,
                    &message,
                    &mut results,
                )?;
                gates = gate_lines(&self.config.gates, &results);
                self.record(&ticket.id, |t| t.gates = results)?;
                landing
            }
        };
        match landing {
            Landing::Merged(sha) => {
                self.record(&ticket.id, |t| t.merged = Some(sha.clone()))?;
                let criteria = report::met(&reported, &criteria);
                let written =
                    writeback::landed(self.tracker, self.cx, &ticket.id, &criteria, &sha, &gates)?;
                worktree::remove(self.top, &wt, &branch)?;
                events.event(&format!(
                    "ticket {} finished: merged {}",
                    ticket.id,
                    &sha[..12]
                ));
                Ok(Ending {
                    written,
                    ..Ending::new(Outcome::Finished)
                })
            }
            Landing::Failed(reason) => {
                events.event(&format!("ticket {} failed: {reason}", ticket.id));
                Ok(Ending::because(Outcome::Failed, reason))
            }
        }
    }
}

/// Each gate as its landing ran it, for the landing comment; no result means skipped.
pub fn gate_lines(gates: &[Gate], results: &[GateResult]) -> Vec<String> {
    gates
        .iter()
        .map(|g| match results.iter().find(|r| r.name == g.name) {
            Some(r) if r.code == 0 && !r.timed_out => format!("{}: passed", g.name),
            Some(r) => format!("{}: {}", g.name, r.ended()),
            None => format!("{}: skipped", g.name),
        })
        .collect()
}

/// A stuck session: its notes are the reason; unmet criteria are kept beside them.
fn stuck(ticket: &Ticket, report: Report, events: &mut Events) -> Ending {
    let unmet = report.unmet();
    let notes = report.notes.trim();
    let mut cause: Vec<String> = notes.lines().next().map(String::from).into_iter().collect();
    if !unmet.is_empty() {
        cause.push(format!("unmet: {}", unmet.join(", ")));
    }
    events.event(&format!("ticket {} stuck: {}", ticket.id, cause.join("; ")));
    Ending {
        reason: (!notes.is_empty()).then(|| notes.to_string()),
        unmet,
        ..Ending::new(Outcome::Stuck)
    }
}
