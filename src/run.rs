//! Run loop: a dispatcher that re-reads the tracker each time it picks tickets, works
//! up to the session ceiling at once, and lands each ticket's gated work on the
//! integration branch, one landing at a time.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use anyhow::{Result, bail};

use crate::config::{self, Gate, MachineConfig, MaxParallel, RepoConfig};
use crate::git::{self, git};
use crate::integrator::{self, Integration, Landing};
use crate::report::{self, Report};
use crate::reporting::Events;
use crate::spec::{self, Reading};
use crate::state::{self, GateResult, RunDirs, State, Store, TicketState};
use crate::tracker::{self, Msg, Pausing, Remote, Ticket, Tracker};
use crate::writeback::{self, Written};
use crate::{control, planner, prompt, session, worktree};

/// What a worker sends back when its ticket's work ends.
type Done = (String, Result<Outcome>);

pub fn run(top: &Path, spec_id: &str, branch: &str, max: Option<MaxParallel>) -> Result<()> {
    let (config, machine) = config::load(top)?;
    check_tracker(top, &config)?;
    preflight(top, &config, branch)?;
    let max = max.unwrap_or(machine.max_parallel);
    // Refuse a spec that cannot start before anything is created.
    let start = spec::read(
        tracker::open(&config, top).as_ref(),
        spec_id,
        &config.criteria_headings,
    )?;
    planner::plan(&start)?;

    let dirs = RunDirs::create(&machine, top, spec_id)?;
    let started = [
        format!(
            "run {} started: spec {spec_id}, integration branch {branch}",
            dirs.id
        ),
        format!("max parallel: {}", planner::ceiling(max, &start)),
    ];
    let state = State::new(&dirs.id, spec_id, branch);
    let fresh =
        |_: &dyn Tracker, _: &writeback::Context, _: &Store, _: &mut Events| Ok(HashSet::new());
    drive(top, &config, &machine, max, dirs, state, &started, fresh)
}

/// Refuse a tracker that must not be used, and print its warnings.
pub fn check_tracker(top: &Path, config: &RepoConfig) -> Result<()> {
    for warning in tracker::check(config, top)? {
        eprintln!("orchestrate: warning: {warning}");
    }
    Ok(())
}

/// The safety checks before `run` or `resume` works toward integration branch `branch`.
pub fn preflight(top: &Path, config: &RepoConfig, branch: &str) -> Result<()> {
    if config.gates.is_empty() {
        bail!("no gate configured; add a [[gates]] entry to .orchestrate/config.toml");
    }
    let protected = match &config.protected_branches {
        Some(list) => list.clone(),
        None => git::origin_default_branch(top)?.into_iter().collect(),
    };
    if protected.iter().any(|b| b == branch) {
        bail!("`{branch}` is a protected branch; pass another --branch");
    }
    Ok(())
}

/// Run the loop of the run in `dirs`, from `state`, until nothing runs and nothing can
/// start. `before` runs once the tracker is up, before anything starts, and returns the
/// tickets not to work again.
#[allow(clippy::too_many_arguments)]
pub fn drive(
    top: &Path,
    config: &RepoConfig,
    machine: &MachineConfig,
    max: MaxParallel,
    dirs: RunDirs,
    state: State,
    started: &[String],
    before: impl FnOnce(
        &dyn Tracker,
        &writeback::Context,
        &Store,
        &mut Events,
    ) -> Result<HashSet<String>>,
) -> Result<()> {
    let (spec_id, branch) = (&state.spec.clone(), &state.branch.clone());
    let headings = &config.criteria_headings;
    let notify = machine.notifications != Some(false);
    let spec_slug = state::slug(spec_id);
    let mut events = Events::open(&dirs.logs.join("run.log"))?.notifying(notify, &spec_slug);
    let store = Arc::new(Store::create(&dirs, state)?);
    control::stop_on_signal(&dirs, Arc::clone(&store))?;
    let integration_wt = dirs.worktrees.join("_integration");
    if !integration_wt.exists() {
        worktree::integration(top, &integration_wt, branch)?;
    }
    for line in started {
        events.event(line);
    }

    // The tracker reads and writes back in the integration worktree, never the user's checkout.
    // A tracker failure pauses the run until the tracker answers again. Only this
    // (the dispatcher) thread calls it; workers reach it through a `Remote`.
    let tracker = Pausing::new(
        tracker::open(config, &integration_wt),
        &config.tracker,
        Events::open(&dirs.logs.join("run.log"))?.notifying(notify, &spec_slug),
        Box::new(|paused| store.update(|s| s.paused = paused)),
    );
    let into = Integration {
        worktree: &integration_wt,
        branch,
    };
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut private = vec![(top, "<repo>"), (state_dir(&dirs), "<state>")];
    private.extend(home.as_deref().map(|h| (h, "~")));
    let cx = writeback::Context {
        run_id: &dirs.id,
        branch,
        private,
    };
    let mut tried = before(&tracker, &cx, &store, &mut events)?;
    let landing = Mutex::new(());
    let (dirs, into, cx, store_ref, landing) = (&dirs, &into, &cx, &*store, &landing);
    let mut running = 0;
    // Whether the run is paused on a blocking cycle, and the wait before its next re-read.
    let mut cycle = false;
    let mut wait = config.tracker.probe_backoff;
    thread::scope(|scope| -> Result<()> {
        let (tx, rx) = mpsc::channel::<Msg<Done>>();
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
                        store.update(|s| {
                            s.tickets.get_mut(&ticket.id).map(|t| {
                                t.outcome = "running".into();
                                t.started = Some(jiff::Timestamp::now().to_string());
                            })
                        })?;
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
                                cx,
                                store: store_ref,
                                landing,
                            };
                            let outcome = Events::open(&dirs.logs.join("run.log"))
                                .and_then(|mut events| run.work(&reading, &ticket, &mut events));
                            let _ = tx.send(Msg::Done((ticket.id, outcome)));
                        });
                    }
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
            let title = store.update(|s| s.tickets.get(&id).map(|t| t.title.clone()))?;
            events.notify(&format!("{id} {}: {kind}", title.unwrap_or_default()));
            store.update(|state| {
                set_outcome(state, &id, outcome);
                if kind != "finished" {
                    block_dependents(state, &reading, &id, kind, &mut events);
                }
            })?;
        }
        Ok(())
    })?;
    worktree::remove_worktree(top, &integration_wt)?;

    let (counts, problems) = store.update(|state| {
        state.status = "ended".into();
        let count = |o: &str| state.tickets.values().filter(|t| t.outcome == o).count();
        let counts = (
            count("finished"),
            count("failed") + count("timeout"),
            count("stuck"),
        );
        let problems: Vec<String> = (state.tickets.iter())
            .flat_map(|(id, t)| t.writeback.iter().map(move |p| format!("ticket {id}: {p}")))
            .collect();
        (counts, problems)
    })?;
    let ended = format!(
        "run {} ended: {} finished, {} failed, {} stuck; integration branch {branch}",
        dirs.id, counts.0, counts.1, counts.2,
    );
    events.event(&ended);
    events.notify(&ended);
    for problem in &problems {
        events.event(problem);
    }
    events.event(&format!("push it with: git push -u origin {branch}"));
    Ok(())
}

/// How a ticket's work ended: `finished`, `failed`, `timeout` (a kind of failed) or
/// `stuck`, with the cause.
struct Outcome {
    kind: &'static str,
    reason: Option<String>,
    unmet: Vec<String>,
    written: Written,
}

impl Outcome {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            reason: None,
            unmet: Vec::new(),
            written: Written::default(),
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
        t.mismatches = outcome.written.mismatches;
        t.writeback = outcome.written.problems;
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
    cx: &'a writeback::Context<'a>,
    store: &'a Store,
    /// The integration lock: one landing (through write-back and teardown) at a time.
    landing: &'a Mutex<()>,
}

impl Run<'_> {
    /// Change ticket `id`'s recorded state.
    fn ticket(&self, id: &str, f: impl FnOnce(&mut TicketState)) -> Result<()> {
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

    /// Work one ticket and write its outcome back to the tracker.
    fn work(&self, reading: &Reading, ticket: &Ticket, events: &mut Events) -> Result<Outcome> {
        let mut outcome = self.attempt(reading, ticket, events)?;
        if outcome.kind != "finished" {
            // The tracker may commit in the integration worktree, so wait out any landing.
            let _landing = self.landing.lock().unwrap_or_else(|e| e.into_inner());
            let reason = outcome.reason.as_deref();
            outcome.written =
                writeback::outcome(self.tracker, self.cx, &ticket.id, outcome.kind, reason);
        }
        Ok(outcome)
    }

    /// Work one ticket from worktree to landing; returns its outcome.
    fn attempt(&self, reading: &Reading, ticket: &Ticket, events: &mut Events) -> Result<Outcome> {
        let slug = self.fresh(state::slug(&ticket.id))?;
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
        // Seeded and prepared files are not the session's work, so landing allows them.
        let baseline = git::changes(&wt)?;

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
        let _landing = self.landing.lock().unwrap_or_else(|e| e.into_inner());
        let mut gates = Vec::new();
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
                    &baseline,
                    &self.config.gates,
                    &self.config.session,
                    &self.dirs.logs,
                    &slug,
                    &message,
                    &mut results,
                )?;
                gates = gate_lines(&self.config.gates, &results);
                self.ticket(&ticket.id, |t| t.gates = results)?;
                landing
            }
        };
        match landing {
            Landing::Merged(sha) => {
                self.ticket(&ticket.id, |t| t.merged = Some(sha.clone()))?;
                let criteria = report::met(&reported, &criteria);
                let written =
                    writeback::landed(self.tracker, self.cx, &ticket.id, &criteria, &sha, &gates)?;
                worktree::remove(self.top, &wt, &branch)?;
                events.event(&format!(
                    "ticket {} finished: merged {}",
                    ticket.id,
                    &sha[..12]
                ));
                Ok(Outcome {
                    written,
                    ..Outcome::new("finished")
                })
            }
            Landing::Failed(reason) => {
                events.event(&format!("ticket {} failed: {reason}", ticket.id));
                Ok(Outcome::because("failed", reason))
            }
        }
    }
}

/// Each gate as its landing ran it, for the landing comment; no result means skipped.
pub fn gate_lines(gates: &[Gate], results: &[GateResult]) -> Vec<String> {
    gates
        .iter()
        .map(|g| match results.iter().find(|r| r.name == g.name) {
            Some(r) if r.code == 0 => format!("{}: passed", g.name),
            Some(r) => format!("{}: exited {}", g.name, r.code),
            None => format!("{}: skipped", g.name),
        })
        .collect()
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
        reason: (!notes.is_empty()).then(|| notes.to_string()),
        unmet,
        ..Outcome::new("stuck")
    }
}

/// The per-repo state dir holding this run's state, logs and worktrees.
fn state_dir(dirs: &RunDirs) -> &Path {
    dirs.worktrees
        .parent()
        .and_then(Path::parent)
        .unwrap_or(&dirs.run)
}
