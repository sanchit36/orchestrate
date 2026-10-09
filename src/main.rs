mod config;
mod git;
mod integrator;
mod planner;
mod prompt;
mod report;
mod reporting;
mod run;
mod session;
mod spec;
mod state;
mod tracker;
mod worktree;

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
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Plan { spec } => plan(&spec),
        Cmd::Run { spec, branch } => git_toplevel().and_then(|top| run::run(&top, &spec, &branch)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("orchestrate: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn plan(spec_id: &str) -> Result<()> {
    let top = git_toplevel()?;
    let (config, _) = config::load(&top)?;
    let tracker = tracker::open(&config, &top);
    let reading = spec::read(tracker.as_ref(), spec_id)?;
    println!(
        "Spec {}: {}\n\nFrontier:",
        reading.spec.id, reading.spec.title
    );
    for t in planner::frontier(&reading) {
        println!("  {}  {}", t.id, t.title);
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
