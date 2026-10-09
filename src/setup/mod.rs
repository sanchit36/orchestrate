//! Setup: detect, propose and confirm, verify on one real spec, then write the repo
//! config (and the machine config when there is none). Answers are read line by line,
//! so a script can feed them on stdin; an empty answer takes the `[default]`.

mod compile;
mod detect;

use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use toml::{Table, Value};

use crate::config::{self, MaxParallel, RepoConfig, TrackerKind};
use crate::tracker::commands;
use crate::{planner, spec, tracker};

const KINDS: [&str; 3] = ["github", "local-markdown", "commands"];

pub fn setup(top: &Path, input: &mut dyn BufRead) -> Result<()> {
    let repo_path = top.join(config::REPO_CONFIG);
    let machine_path = config::machine_config_path()?;
    let mut repo = config::read(&repo_path)?.unwrap_or_default();
    repo.insert("version".into(), Value::Integer(1));
    let mut ask = Ask { input };

    let tracker = table(&mut repo, "tracker")?;
    let current_kind = text(tracker, "kind");
    let kind = loop {
        let default = current_kind.clone().unwrap_or(detect::kind(top).into());
        let kind = ask.ask(
            "Tracker kind (github, local-markdown or commands)",
            &default,
        )?;
        match KINDS.contains(&kind.as_str()) {
            true => break kind,
            false => println!("Pick one of: {}", KINDS.join(", ")),
        }
    };
    let location = match current_kind.as_deref() == Some(&kind) {
        true => text(tracker, "location"),
        false => detect::location(top, &kind),
    };
    let what = match kind.as_str() {
        "github" => "owner/repo",
        "commands" => "project, board or URL the templates reach",
        _ => "folder of spec folders",
    };
    let location = ask.ask(
        &format!("Tracker location ({what})"),
        &location.unwrap_or_default(),
    )?;
    let ready = text(tracker, "ready")
        .or_else(|| detect::ready(top))
        .unwrap_or("ready-for-agent".into());
    let ready = ask.ask("Ready marker", &ready)?;
    let current_commands = tracker
        .remove("commands")
        .filter(|_| current_kind.as_deref() == Some("commands"));
    tracker.insert("kind".into(), kind.clone().into());
    tracker.insert("location".into(), location.clone().into());
    tracker.insert("ready".into(), ready.clone().into());

    let headings = strings(&repo, "criteria_headings")
        .unwrap_or(vec!["Acceptance criteria".into(), "Acceptance".into()]);
    let headings = ask.list("Acceptance criteria headings", &headings)?;
    if kind == "commands" {
        let source = compile::Source {
            location,
            ready,
            headings: headings.clone(),
        };
        let commands = compile::templates(&mut ask, top, &source, current_commands)?;
        table(&mut repo, "tracker")?.insert("commands".into(), commands);
    }
    repo.insert("criteria_headings".into(), headings.into());

    let gates = ask_gates(&mut ask, top, repo.get("gates"))?;
    match gates.is_empty() {
        true => _ = repo.remove("gates"),
        false => _ = repo.insert("gates".into(), Value::Array(gates)),
    }

    let seeds = match strings(&repo, "seed_files") {
        Some(seeds) => seeds,
        None => detect::seed_files(top)?,
    };
    println!("Seed files are local files copied into each fresh worktree.");
    let seeds = ask.list("Seed files", &seeds)?;
    repo.insert("seed_files".into(), seeds.into());

    let session = table(&mut repo, "session")?;
    println!(
        "Sessions run Claude Code with `--permission-mode bypassPermissions` by default: \
         they never stop to ask before running commands or editing files in their worktree. \
         This is a repo setting, `[session] permission_mode`."
    );
    let mode = text(session, "permission_mode").unwrap_or("bypassPermissions".into());
    let mode = ask.ask("Session permission mode", &mode)?;
    session.insert("permission_mode".into(), mode.into());

    let machine = match machine_path.exists() {
        true => None,
        false => Some(ask_machine(&mut ask)?),
    };

    let config = config::repo_config(&repo_path, repo.clone())?;
    let spec = ask.ask("Spec id to verify the tracker on, or `skip`", "")?;
    match spec.as_str() {
        "skip" => eprintln!(
            "orchestrate: warning: verification skipped; the first `plan` or `run` checks the tracker"
        ),
        _ => verify(&config, top, &spec)?,
    }

    write(&repo_path, &repo)?;
    println!("Wrote {}; commit it.", repo_path.display());
    if let Some(machine) = machine {
        write(&machine_path, &machine)?;
        println!("Wrote {}.", machine_path.display());
    }
    Ok(())
}

/// Current gates (or detected ones) one by one, then hand-entered ones.
fn ask_gates(ask: &mut Ask, top: &Path, current: Option<&Value>) -> Result<Vec<Value>> {
    let proposed: Vec<Table> = match current {
        Some(gates) => gates
            .as_array()
            .context("`gates` must be an array")?
            .iter()
            .map(|g| g.as_table().cloned().context("each gate must be a table"))
            .collect::<Result<_>>()?,
        None => detect::gates(top)
            .into_iter()
            .map(|(name, run)| {
                Table::from_iter([("name".into(), name.into()), ("run".into(), run.into())])
            })
            .collect(),
    };
    println!("Gates run in a ticket's rebased worktree and must pass before it lands.");
    let mut gates = Vec::new();
    for mut gate in proposed {
        let name = text(&gate, "name").unwrap_or_default();
        let run = text(&gate, "run").unwrap_or_default();
        let run = ask.ask(&format!("Gate `{name}` (- drops it)"), &run)?;
        if run != "-" {
            gate.insert("run".into(), run.into());
            gates.push(Value::Table(gate));
        }
    }
    loop {
        let line = ask.line("Another gate as `name: command` (Enter to finish)")?;
        if line.is_empty() {
            return Ok(gates);
        }
        match line.split_once(':').map(|(n, r)| (n.trim(), r.trim())) {
            Some((name, run)) if !name.is_empty() && !run.is_empty() => {
                let gate =
                    Table::from_iter([("name".into(), name.into()), ("run".into(), run.into())]);
                gates.push(Value::Table(gate));
            }
            _ => println!("Write it as `name: command`."),
        }
    }
}

/// Machine preferences, asked only when the machine config does not exist yet.
fn ask_machine(ask: &mut Ask) -> Result<Table> {
    println!("No machine config yet; these preferences are yours, not the repo's.");
    let max_parallel = loop {
        let answer = ask.ask("Sessions at once (auto or a number)", "auto")?;
        match answer.parse::<MaxParallel>() {
            Ok(MaxParallel(Some(n))) => break Value::Integer(n.get() as i64),
            Ok(MaxParallel(None)) => break "auto".into(),
            Err(e) => println!("{e}"),
        }
    };
    let notifications = loop {
        match ask
            .ask("Desktop notifications (yes or no)", "yes")?
            .as_str()
        {
            "yes" => break true,
            "no" => break false,
            _ => println!("Answer yes or no."),
        }
    };
    Ok(Table::from_iter([
        ("version".into(), Value::Integer(1)),
        ("max_parallel".into(), max_parallel),
        ("notifications".into(), notifications.into()),
    ]))
}

/// Run the three reads on `spec`, print the plan, and show (never run) mark-finished.
fn verify(config: &RepoConfig, top: &Path, spec_id: &str) -> Result<()> {
    for warning in tracker::check(config, top)? {
        eprintln!("orchestrate: warning: {warning}");
    }
    let tracker = tracker::open(config, top);
    let reading = spec::read(tracker.as_ref(), spec_id, &config.criteria_headings)
        .context("verification failed; nothing written")?;
    println!();
    let plan = planner::plan(&reading).context("verification failed; nothing written")?;
    // `spec::read` refuses a spec without a ready ticket, so the frontier or the list has one.
    let sample = (plan.frontier.first().copied())
        .unwrap_or(&reading.tickets[0].ticket)
        .id
        .clone();
    planner::print(&reading, plan);
    let location = &config.tracker.location;
    let command = match config.tracker.kind {
        TrackerKind::GitHub => format!(
            "gh api -X PATCH repos/{location}/issues/{sample} --input - \
             <<< '{{\"state\":\"closed\",\"state_reason\":\"completed\"}}'"
        ),
        TrackerKind::LocalMarkdown => format!(
            "set `Status: done` in {location}/{spec_id}/{sample}.md and commit it on the integration branch"
        ),
        TrackerKind::Commands => {
            let templates = config.tracker.commands.as_ref().expect("checked at load");
            commands::render(&templates.mark_finished, &sample)
        }
    };
    println!("\nMarking a ticket finished would run (not run now):\n  {command}\n");
    Ok(())
}

/// Reads one answer per line; at end of input it fails rather than guess.
struct Ask<'a> {
    input: &'a mut dyn BufRead,
}

impl Ask<'_> {
    fn line(&mut self, question: &str) -> Result<String> {
        print!("{question}: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if self.input.read_line(&mut line)? == 0 {
            println!();
            bail!("no answer for: {question}");
        }
        Ok(line.trim().to_string())
    }

    /// An answer, `default` when empty; asks again while both are empty.
    fn ask(&mut self, question: &str, default: &str) -> Result<String> {
        loop {
            let q = match default {
                "" => question.to_string(),
                _ => format!("{question} [{default}]"),
            };
            let answer = self.line(&q)?;
            match (answer.is_empty(), default.is_empty()) {
                (false, _) => return Ok(answer),
                (true, false) => return Ok(default.to_string()),
                (true, true) => {}
            }
        }
    }

    /// A comma-separated list; `-` is the empty list.
    fn list(&mut self, question: &str, default: &[String]) -> Result<Vec<String>> {
        let default = match default.is_empty() {
            true => "-".to_string(),
            false => default.join(", "),
        };
        let answer = self.ask(
            &format!("{question}, comma-separated (- for none)"),
            &default,
        )?;
        Ok(answer
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "-")
            .map(String::from)
            .collect())
    }
}

fn table<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    parent
        .entry(key)
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .with_context(|| format!("`{key}` must be a table"))
}

fn text(table: &Table, key: &str) -> Option<String> {
    table.get(key)?.as_str().map(String::from)
}

fn strings(table: &Table, key: &str) -> Option<Vec<String>> {
    let list = table.get(key)?.as_array()?;
    Some(
        list.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
    )
}

/// `version` first, then the rest as TOML.
fn write(path: &Path, table: &Table) -> Result<()> {
    let mut rest = table.clone();
    rest.remove("version");
    let text = format!("version = 1\n{}", toml::to_string(&rest)?);
    fs::create_dir_all(path.parent().context("config path has a parent")?)?;
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}
