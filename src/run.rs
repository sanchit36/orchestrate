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
use crate::state::{self, RunDirs, State, TicketState};
use crate::tracker::{self, Ticket, Tracker};
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

    // The tracker reads and writes back in the integration worktree, never the user's checkout.
    let tracker = tracker::open(&config, &integration_wt);
    let into = Integration {
        worktree: &integration_wt,
        branch,
    };
    let run = Run {
        top,
        config: &config,
        dirs: &dirs,
        into: &into,
        tracker: tracker.as_ref(),
    };
    let mut tried = HashSet::new();
    loop {
        let reading = spec::reread(tracker.as_ref(), spec_id, headings)?;
        for t in reading.tickets.iter().map(|e| &e.ticket) {
            let seen = state.tickets.entry(t.id.clone()).or_insert(TicketState {
                title: t.title.clone(),
                outcome: if t.finished { "finished" } else { "pending" }.into(),
            });
            seen.title = t.title.clone();
        }
        let next = planner::plan(&reading)?
            .frontier
            .into_iter()
            .find(|t| !tried.contains(&t.id));
        let Some(ticket) = next else { break };
        tried.insert(ticket.id.clone());
        set_outcome(&mut state, &dirs, &ticket.id, "running")?;
        let outcome = run.work(&reading, ticket, &mut events)?;
        set_outcome(&mut state, &dirs, &ticket.id, outcome)?;
    }

    let count = |o: &str| state.tickets.values().filter(|t| t.outcome == o).count();
    events.event(&format!(
        "run {} ended: {} finished, {} failed, {} stuck; integration branch {branch}",
        dirs.id,
        count("finished"),
        count("failed"),
        count("stuck"),
    ));
    events.event(&format!("push it with: git push -u origin {branch}"));
    Ok(())
}

fn set_outcome(state: &mut State, dirs: &RunDirs, id: &str, outcome: &str) -> Result<()> {
    if let Some(t) = state.tickets.get_mut(id) {
        t.outcome = outcome.into();
    }
    state.save(dirs)
}

struct Run<'a> {
    top: &'a Path,
    config: &'a RepoConfig,
    dirs: &'a RunDirs,
    into: &'a Integration<'a>,
    tracker: &'a dyn Tracker,
}

impl Run<'_> {
    /// Work one ticket from worktree to landing; returns its outcome.
    fn work(
        &self,
        reading: &Reading,
        ticket: &Ticket,
        events: &mut Events,
    ) -> Result<&'static str> {
        let slug = state::slug(&ticket.id);
        let wt = self.dirs.worktrees.join(&slug);
        let branch = format!("orchestrate/{}/{slug}", self.dirs.id);
        worktree::create(self.top, &wt, &branch, self.into.branch)?;
        events.event(&format!("ticket {} started: {}", ticket.id, ticket.title));
        let prepare_log = self.dirs.logs.join(format!("{slug}.prepare.log"));
        if let Some(reason) = worktree::prepare(self.top, &wt, self.config, &prepare_log)? {
            events.event(&format!("ticket {} failed: {reason}", ticket.id));
            return Ok("failed");
        }

        let session = &self.config.session;
        let prompt = prompt::render(session, self.top, &reading.spec.body, &ticket.body, &branch)?;
        let log = self.dirs.logs.join(format!("{slug}.session.log"));
        let code = session::run(&wt, &prompt, &session.permission_mode, &log)?;

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
            Ok(report) if !report.is_done() => {
                events.event(&format!("ticket {} stuck", ticket.id));
                return Ok("stuck");
            }
            Ok(_) if new_commits == "0" => Some("no new commits".into()),
            Ok(_) => None,
        };
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
                Ok("finished")
            }
            Landing::Failed(reason) => {
                events.event(&format!("ticket {} failed: {reason}", ticket.id));
                Ok("failed")
            }
        }
    }
}
