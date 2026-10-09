mod config;
mod markdown;
mod planner;
mod spec;
mod tracker;

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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Plan { spec } => plan(&spec),
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
    let config = config::load(&top)?;
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
