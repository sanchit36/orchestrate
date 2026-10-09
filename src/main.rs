mod config;
mod control;
mod gate;
mod git;
mod integrator;
mod lock;
mod markdown;
mod planner;
mod prompt;
mod report;
mod reporting;
mod run;
mod session;
mod spec;
mod state;
mod status;
mod tracker;
mod worktree;
mod writeback;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

/// Works a spec's tickets as unattended sessions and lands them on one integration branch.
#[derive(Parser)]
#[command(name = "orchestrate", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the frontier of a spec without starting anything.
    Plan {
        /// The spec id on the tracker.
        spec: String,
    },
    /// Work a spec's frontier and land each ticket on the integration branch.
    Run {
        /// The spec id on the tracker.
        spec: String,
        /// The integration branch: created from `HEAD`, or reused if it exists.
        #[arg(long)]
        branch: String,
        /// Sessions at once: `auto` (from the graph width and cores) or a number.
        /// Defaults to the machine config's `max_parallel`, else `auto`.
        #[arg(long)]
        max_parallel: Option<config::MaxParallel>,
        /// Keep running in the background after the terminal closes.
        #[arg(long)]
        detach: bool,
    },
    /// Stop the live run: kill its sessions and mark it stopped.
    Stop {
        /// The run to stop: its id or an id prefix.
        #[arg(long)]
        run: Option<String>,
    },
    /// Show where a run is, or diagnose one of its tickets. Reads run state only.
    Status {
        /// A ticket id: show its criteria, notes, gates, worktree and logs.
        ticket: Option<String>,
        /// A run id, unique id prefix or spec id; default the most recent run.
        #[arg(long)]
        run: Option<String>,
    },
    /// List this repo's runs, most recent first.
    Runs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Plan { spec } => plan(&spec),
        Cmd::Run {
            spec,
            branch,
            max_parallel,
            detach,
        } => git_toplevel().and_then(|top| match detach {
            true => control::detach(&top, &spec, &branch, max_parallel),
            false => run::run(&top, &spec, &branch, max_parallel),
        }),
        Cmd::Stop { run } => git_toplevel().and_then(|top| control::stop(&top, run.as_deref())),
        Cmd::Status { ticket, run } => {
            git_toplevel().and_then(|top| status::status(&top, run.as_deref(), ticket.as_deref()))
        }
        Cmd::Runs => git_toplevel().and_then(|top| status::runs(&top)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // A detached run's stderr may be gone; a failed write must not panic.
            let _ = writeln!(std::io::stderr(), "orchestrate: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn plan(spec_id: &str) -> Result<()> {
    let top = git_toplevel()?;
    let (config, _) = config::load(&top)?;
    for warning in tracker::check(&config, &top)? {
        eprintln!("orchestrate: warning: {warning}");
    }
    let tracker = tracker::open(&config, &top);
    let reading = spec::read(tracker.as_ref(), spec_id, &config.criteria_headings)?;
    let plan = planner::plan(&reading)?;
    println!(
        "Spec {}: {}\n\nFrontier:",
        reading.spec.id, reading.spec.title
    );
    for t in plan.frontier {
        println!("  {}  {}", t.id, t.title);
    }
    for (heading, list) in [("Waiting", plan.waiting), ("Needs attention", plan.held)] {
        if !list.is_empty() {
            println!("\n{heading}:");
        }
        for (t, why) in list {
            println!("  {}  {}  ({why})", t.id, t.title);
        }
    }
    if !reading.warnings.is_empty() {
        println!("\nWarnings:");
        for w in &reading.warnings {
            println!("  {w}");
        }
    }
    Ok(())
}

/// Config is found only here.
fn git_toplevel() -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!("not inside a git repository");
    }
    Ok(PathBuf::from(String::from_utf8(out.stdout)?.trim_end()))
}
