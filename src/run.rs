//! Run loop: a dispatcher that re-reads the tracker each time it picks tickets, works
//! up to the session ceiling at once (each ticket on its own thread, see `work`), and
//! ends the run when nothing runs and nothing can start.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use anyhow::{Result, bail};

use crate::config::{self, MachineConfig, MaxParallel, RepoConfig};
use crate::events::{self, Events};
use crate::git;
use crate::integrator::Integration;
use crate::spec::{self, Reading};
use crate::state::{self, Outcome, RunDirs, RunStatus, State, Store, TicketState};
use crate::tracker::{self, Msg, Pausing, Remote, Tracker};
use crate::work::{Ending, Work};
use crate::writeback;
use crate::{control, planner, session, worktree};

/// What a ticket's thread sends back when its work ends.
type Ended = (String, Result<Ending>);

pub fn run(top: &Path, spec_id: &str, branch: &str, max: Option<MaxParallel>) -> Result<()> {
    let (config, machine) = config::load(top)?;
    check_tracker(top, &config)?;
    preflight(top, &config, branch)?;
    let max = max.unwrap_or(machine.max_parallel);
    // Refuse a spec that cannot start before anything is created.
    let start = spec::read_to_start(
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
        // Without a remote default branch, the branch the user has checked out.
        None => (git::origin_default_branch(top)?)
            .or_else(|| git::current_branch(top))
            .into_iter()
            .collect(),
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
    // First, so a detached run keeps the notice in its log.
    for line in dirs.lock.taken_over.iter().chain(started) {
        events.event(line);
    }

    // The tracker reads and writes back in the integration worktree, never the user's checkout.
    // A tracker failure pauses the run until the tracker answers again. Only this
    // (the dispatcher) thread calls it; ticket threads reach it through a `Remote`.
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
    // Without its trailing `/`, so `<tmp>/x` keeps the separator.
    let tmp: std::path::PathBuf = std::env::temp_dir().components().collect();
    let mut private = vec![
        (top, "<repo>"),
        (state_dir(&dirs), "<state>"),
        (&tmp, "<tmp>"),
    ];
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
        let (tx, rx) = mpsc::channel::<Msg<Ended>>();
        let mut dispatch = || -> Result<()> {
            loop {
                let reading = spec::read(&tracker, spec_id, headings)?;
                store.update(|state| {
                    for t in reading.tickets.iter().map(|e| &e.ticket) {
                        let seen = state.tickets.entry(t.id.clone()).or_insert_with(|| {
                            let outcome = match t.finished {
                                true => Outcome::Finished,
                                false => Outcome::Pending,
                            };
                            TicketState {
                                outcome,
                                ..TicketState::default()
                            }
                        });
                        seen.title = t.title.clone();
                    }
                })?;
                match planner::plan(&reading) {
                    Err(e) => {
                        // Like a pause: nothing new starts; running sessions finish.
                        if !cycle {
                            let paused = format!("run paused: {e:#}");
                            events.event(&paused);
                            events.notify(&paused);
                            cycle = true;
                        }
                    }
                    Ok(plan) => {
                        if cycle {
                            let recovered = "run recovered: blocking cycle resolved";
                            events.event(recovered);
                            events.notify(recovered);
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
                                    t.outcome = Outcome::Running;
                                    t.started = Some(jiff::Timestamp::now().to_string());
                                })
                            })?;
                            running += 1;
                            let (reading, ticket) = (reading.clone(), ticket.clone());
                            let tx = tx.clone();
                            scope.spawn(move || {
                                let remote = Remote(tx.clone());
                                let work = Work {
                                    top,
                                    config,
                                    dirs,
                                    into,
                                    tracker: &remote,
                                    cx,
                                    store: store_ref,
                                    landing,
                                };
                                let ending = Events::open(&dirs.logs.join("run.log")).and_then(
                                    |mut events| work.ticket(&reading, &ticket, &mut events),
                                );
                                let _ = tx.send(Msg::Ended((ticket.id, ending)));
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
                // Serve the ticket threads' tracker calls until one ticket's work ends.
                let (id, ending) = loop {
                    match rx.recv()? {
                        Msg::Call(call) => call(&tracker),
                        Msg::Ended(ended) => break ended,
                    }
                };
                running -= 1;
                let ending = ending?;
                let outcome = ending.outcome;
                let title = store.update(|s| s.tickets.get(&id).map(|t| t.title.clone()))?;
                events.notify(&format!("{id} {}: {outcome}", title.unwrap_or_default()));
                store.update(|state| {
                    set_ending(state, &id, ending);
                    if outcome != Outcome::Finished {
                        block_dependents(state, &reading, &id, outcome, &mut events);
                    }
                })?;
            }
            Ok(())
        };
        let dispatched = dispatch();
        if dispatched.is_err() {
            // The scope waits for every ticket's thread: end their sessions, not wait them out.
            session::kill_all();
        }
        dispatched
    })?;
    // A worktree left behind is for `clean`; the run itself has ended.
    if let Err(e) = worktree::remove_worktree(top, &integration_wt) {
        let e = format!("{e:#}").replace('\n', " ");
        events.event(&format!("could not remove the integration worktree: {e}"));
    }

    let (counts, problems) = store.update(|state| {
        state.status = RunStatus::Ended;
        let count = |o: Outcome| state.tickets.values().filter(|t| t.outcome == o).count();
        let counts = (
            // Landed by this run, not finished before it.
            (state.tickets.values())
                .filter(|t| t.outcome == Outcome::Finished && t.merged.is_some())
                .count(),
            count(Outcome::Failed) + count(Outcome::Timeout),
            count(Outcome::Stuck),
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
    events::shown();
    Ok(())
}

fn set_ending(state: &mut State, id: &str, ending: Ending) {
    if let Some(t) = state.tickets.get_mut(id) {
        t.outcome = ending.outcome;
        t.reason = ending.reason;
        t.unmet = ending.unmet;
        t.mismatches = ending.written.mismatches;
        t.writeback = ending.written.problems;
    }
}

/// Every pending ticket blocked, directly or not, by `id` becomes held behind a stuck
/// ticket or skipped behind a failed one (`outcome` is how `id` ended).
pub fn block_dependents(
    state: &mut State,
    reading: &Reading,
    id: &str,
    outcome: Outcome,
    events: &mut Events,
) {
    let (blocked_outcome, cause) = match outcome {
        Outcome::Stuck => (Outcome::Held, "stuck"),
        _ => (Outcome::Skipped, "failed"),
    };
    let reason = format!("blocked by {cause} {id}");
    let mut blocked = HashSet::from([id.to_string()]);
    loop {
        let next = reading.tickets.iter().find(|e| {
            let pending =
                (state.tickets.get(&e.ticket.id)).is_some_and(|t| t.outcome == Outcome::Pending);
            pending && e.blockers.iter().any(|b| blocked.contains(&b.id))
        });
        let Some(e) = next else { break };
        let t = &e.ticket;
        set_ending(
            state,
            &t.id,
            Ending::because(blocked_outcome, reason.clone()),
        );
        events.event(&format!("ticket {} {blocked_outcome}: {reason}", t.id));
        blocked.insert(t.id.clone());
    }
}

/// The per-repo state dir holding this run's state, logs and worktrees.
fn state_dir(dirs: &RunDirs) -> &Path {
    dirs.worktrees
        .parent()
        .and_then(Path::parent)
        .unwrap_or(&dirs.run)
}
