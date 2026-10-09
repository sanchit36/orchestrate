mod clean;
mod config;
mod control;
mod events;
mod gate;
mod git;
mod integrator;
mod lock;
mod markdown;
mod planner;
mod process;
mod prompt;
mod report;
mod resume;
mod run;
mod session;
mod setup;
mod spec;
mod state;
mod status;
mod tracker;
mod work;
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
    /// Detect, propose and confirm the repo's tracker, gates and settings, verify the
    /// tracker on one spec, then write the config. Answers are read from stdin.
    Setup,
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
        /// A run id, unique id prefix or spec id; default the live run.
        #[arg(long)]
        run: Option<String>,
    },
    /// Continue a stopped or crashed run, restarting the tickets it left in flight.
    Resume {
        /// A run id, unique id prefix or spec id; default the most recent run.
        #[arg(long)]
        run: Option<String>,
        /// Keep running in the background after the terminal closes.
        #[arg(long)]
        detach: bool,
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
    /// Remove a run's leftover worktrees and branches. Never touches the integration branch.
    Clean {
        /// A run id, unique id prefix or spec id; default the most recent run.
        #[arg(long, conflicts_with = "all")]
        run: Option<String>,
        /// Also remove failed and stuck tickets' worktrees and branches.
        #[arg(long)]
        force: bool,
        /// Clean every ended run of this repo.
        #[arg(long)]
        all: bool,
        /// Also delete the run directory (state and logs). Implies `--force`.
        #[arg(long)]
        purge: bool,
        /// Only remove the state of repos that are gone or moved, printing each.
        #[arg(long, conflicts_with_all = ["run", "force", "all", "purge"])]
        orphans: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Setup => {
            git_toplevel().and_then(|top| setup::setup(&top, &mut std::io::stdin().lock()))
        }
        Cmd::Plan { spec } => plan(&spec),
        Cmd::Run {
            spec,
            branch,
            max_parallel,
            detach,
        } => git_toplevel().and_then(|top| match detach {
            true => {
                let mut args = vec!["run".to_string(), spec, "--branch".into(), branch];
                if let Some(max) = max_parallel {
                    args.extend(["--max-parallel".into(), max.to_string()]);
                }
                control::detach(&top, &args)
            }
            false => run::run(&top, &spec, &branch, max_parallel),
        }),
        Cmd::Resume { run, detach } => git_toplevel().and_then(|top| match detach {
            true => {
                let mut args = vec!["resume".to_string()];
                args.extend(run.map(|r| ["--run".to_string(), r]).into_iter().flatten());
                control::detach(&top, &args)
            }
            false => resume::resume(&top, run.as_deref()),
        }),
        Cmd::Stop { run } => git_toplevel().and_then(|top| control::stop(&top, run.as_deref())),
        Cmd::Status { ticket, run } => {
            git_toplevel().and_then(|top| status::status(&top, run.as_deref(), ticket.as_deref()))
        }
        Cmd::Runs => git_toplevel().and_then(|top| status::runs(&top)),
        Cmd::Clean {
            run,
            force,
            all,
            purge,
            orphans,
        } => git_toplevel().and_then(|top| {
            let flags = clean::Flags {
                force,
                all,
                purge,
                orphans,
            };
            clean::clean(&top, run.as_deref(), &flags)
        }),
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
    let reading = spec::read_to_start(tracker.as_ref(), spec_id, &config.criteria_headings)?;
    planner::print(&reading, planner::plan(&reading)?);
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn every_command_and_argument_has_help() {
        let cli = super::Cli::command();
        assert!(cli.get_about().is_some());
        for cmd in cli.get_subcommands() {
            let name = cmd.get_name();
            assert!(cmd.get_about().is_some(), "`{name}` has no description");
            for arg in cmd.get_arguments() {
                let id = arg.get_id();
                assert!(arg.get_help().is_some(), "`{name} {id}` has no help");
            }
        }
    }
}
