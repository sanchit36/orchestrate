//! Run loop: a dispatcher that re-reads the tracker each time it picks tickets, works
//! up to the session ceiling at once, and lands each ticket's gated work on the
//! integration branch, one landing at a time.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;

use anyhow::{Result, bail};

use crate::config::{self, MaxParallel, RepoConfig};
use crate::git::git;
use crate::integrator::{self, Integration, Landing};
use crate::report::Report;
use crate::reporting::Events;
use crate::spec::{self, Reading};
use crate::state::{self, RunDirs, State, TicketState};
use crate::tracker::{self, Msg, Pausing, Remote, Ticket, Tracker};
use crate::{planner, prompt, session, worktree};

/// What a worker sends back when its ticket's work ends.
type Done = (String, Result<Outcome>);

pub fn run(top: &Path, spec_id: &str, branch: &str, max: Option<MaxParallel>) -> Result<()> {
    let (config, machine) = config::load(top)?;
    if config.gates.is_empty() {
        bail!("no gate configured; add a [[gates]] entry to .orchestrate/config.toml");
    }
    let max = max.unwrap_or(machine.max_parallel);
    let headings = &config.criteria_headings;
    // Refuse a spec that cannot start before anything is created.
    let start = spec::read(tracker::open(&config, top).as_ref(), spec_id, headings)?;
    planner::plan(&start)?;

    session::kill_sessions_on_interrupt()?;
    let dirs = RunDirs::create(&machine, top, spec_id)?;
    let mut events = Events::open(&dirs.logs.join("run.log"))?;
    let mut state = State::new(&dirs.id, spec_id, branch);
    state.save(&dirs)?;
    let integration_wt = dirs.worktrees.join("_integration");
    worktree::integration(top, &integration_wt, branch)?;
    events.event(&format!(
        "run {} started: spec {spec_id}, integration branch {branch}",
        dirs.id
    ));
    events.event(&format!("max parallel: {}", planner::ceiling(max, &start)));

    // The tracker reads and writes back in the integration worktree, never the user's checkout.
    // A tracker failure pauses the run until the tracker answers again. Only this
    // (the dispatcher) thread calls it; workers reach it through a `Remote`.
    let tracker = Pausing::new(
        tracker::open(&config, &integration_wt),
        &config.tracker,
        Events::open(&dirs.logs.join("run.log"))?,
    );
    let into = Integration {
        worktree: &integration_wt,
        branch,
    };
    let landing = Mutex::new(());
    let (config, dirs, into, landing) = (&config, &dirs, &into, &landing);
    let mut tried = HashSet::new();
    let mut running = 0;
    // Whether the run is paused on a blocking cycle, and the wait before its next re-read.
    let mut cycle = false;
    let mut wait = config.tracker.probe_backoff;
    thread::scope(|scope| -> Result<()> {
        let (tx, rx) = mpsc::channel::<Msg<Done>>();
        loop {
            let reading = spec::reread(&tracker, spec_id, headings)?;
            for t in reading.tickets.iter().map(|e| &e.ticket) {
                let seen = state.tickets.entry(t.id.clone()).or_insert(TicketState {
                    title: t.title.clone(),
                    outcome: if t.finished { "finished" } else { "pending" }.into(),
                    reason: None,
                    unmet: Vec::new(),
                });
                seen.title = t.title.clone();
            }
            match planner::plan(&reading) {
                Err(e) => {
                    // Like a pause: nothing new starts; running sessions finish.
                    if !cycle {
                        events.event(&format!("run paused: {e:#}"));
                        cycle = true;
                    }
                }
                Ok(plan) => {
                    if cycle {
                        events.event("run recovered: blocking cycle resolved");
                        (cycle, wait) = (false, config.tracker.probe_backoff);
                    }
                    let free = planner::ceiling(max, &reading).saturating_sub(running);
                    // Tracker order; no model call.
                    let next: Vec<_> = plan
                        .frontier
                        .into_iter()
                        .filter(|t| !tried.contains(&t.id))
                        .take(free)
                        .collect();
                    for ticket in next {
                        tried.insert(ticket.id.clone());
                        set_outcome(&mut state, &ticket.id, Outcome::new("running"));
                        running += 1;
                        let (reading, ticket) = (reading.clone(), ticket.clone());
                        let tx = tx.clone();
                        scope.spawn(move || {
                            let remote = Remote(tx.clone());
                            let run = Run {
                                top,
                                config,
                                dirs,
                                into,
                                tracker: &remote,
                                landing,
                            };
                            let outcome = Events::open(&dirs.logs.join("run.log"))
                                .and_then(|mut events| run.work(&reading, &ticket, &mut events));
                            let _ = tx.send(Msg::Done((ticket.id, outcome)));
                        });
                    }
                    state.save(dirs)?;
                }
            }
            if running == 0 {
                if !cycle {
                    break;
                }
                thread::sleep(wait);
                wait = (wait * 2).min(config.tracker.probe_backoff_max);
                continue;
            }
            // Serve the workers' tracker calls until one ticket's work ends.
            let (id, outcome) = loop {
                match rx.recv()? {
                    Msg::Call(call) => call(&tracker),
                    Msg::Done(done) => break done,
                }
            };
            running -= 1;
            let outcome = outcome?;
            let kind = outcome.kind;
            set_outcome(&mut state, &id, outcome);
            if kind != "finished" {
                block_dependents(&mut state, &reading, &id, kind, &mut events);
            }
            state.save(dirs)?;
        }
        Ok(())
    })?;

    let count = |o: &str| state.tickets.values().filter(|t| t.outcome == o).count();
    events.event(&format!(
        "run {} ended: {} finished, {} failed, {} stuck; integration branch {branch}",
        dirs.id,
        count("finished"),
        count("failed") + count("timeout"),
        count("stuck"),
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
    /// The integration lock: one landing (through mark-finished and teardown) at a time.
    landing: &'a Mutex<()>,
}

impl Run<'_> {
    /// Work one ticket from worktree to landing; returns its outcome.
    fn work(&self, reading: &Reading, ticket: &Ticket, events: &mut Events) -> Result<Outcome> {
        let slug = state::slug(&ticket.id);
        let wt = self.dirs.worktrees.join(&slug);
        let branch = format!("orchestrate/{}/{slug}", self.dirs.id);
        worktree::create(self.top, &wt, &branch, self.into.branch)?;
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
        let failure = match Report::read(&wt) {
            _ if code != 0 => Some(format!("session exited {code}")),
            Err(e) => Some(format!("{e:#}")),
            Ok(report) if !report.is_done() => return Ok(stuck(ticket, report, events)),
            Ok(_) if new_commits == "0" => Some("no new commits".into()),
            Ok(_) => None,
        };
        let _landing = self.landing.lock().unwrap_or_else(|e| e.into_inner());
        let landing = match failure {
            Some(reason) => Landing::Failed(reason),
            None => {
                let message = format!("Merge ticket {}: {}", ticket.id, ticket.title);
                let gates = &self.config.gates;
                integrator::land(
                    self.into,
                    &wt,
                    &branch,
                    gates,
                    &self.dirs.logs,
                    &slug,
                    &message,
                )?
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
