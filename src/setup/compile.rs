//! Command templates for a tracker without a built-in adapter: a headless agent compiles
//! them from the tracker guide (or the user's description), and the user approves them.
//! This is the only place an agent touches tracker access.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use toml::Value;

use super::{Ask, detect};
use crate::config::Commands;
use crate::tracker::commands;

/// What the agent compiles from.
pub struct Source {
    pub location: String,
    pub ready: String,
    pub headings: Vec<String>,
}

/// The `[tracker.commands]` table: the `current` templates kept, or freshly compiled,
/// then shown and approved (once per content hash) on this machine.
pub fn templates(
    ask: &mut Ask,
    top: &Path,
    source: &Source,
    current: Option<Value>,
) -> Result<Value> {
    let guide = Path::new(detect::GUIDE);
    let guide_text = fs::read(top.join(guide)).ok();
    let current: Option<Commands> = current.and_then(|c| c.try_into().ok());
    let keep = match &current {
        Some(current) => {
            let changed = guide_text.as_ref().map(|g| commands::sha256(g)) != current.guide_hash;
            let default = if changed { "yes" } else { "no" };
            !yes(ask, "Recompile the command templates (yes or no)", default)?
        }
        None => false,
    };
    let templates = match (keep, current) {
        (true, Some(current)) => current,
        _ => {
            let (text, heading) = match &guide_text {
                Some(g) => (String::from_utf8_lossy(g).into_owned(), "Tracker guide"),
                None => {
                    let what = "Describe your tracker in one paragraph: what it is, how the \
                                shell reaches it, which environment variables hold credentials";
                    (ask.ask(what, "")?, "Tracker description")
                }
            };
            let mut compiled = compile(top, &prompt(source, heading, &text))?;
            compiled.guide = guide_text.as_ref().map(|_| PathBuf::from(guide));
            compiled.guide_hash = guide_text.as_ref().map(|g| commands::sha256(g));
            compiled
        }
    };
    commands::refuse_secrets(&templates)?;
    println!("Command templates (`{{id}}` becomes the quoted spec or ticket id):");
    for (name, template) in commands::named(&templates) {
        println!("  {name}: {template}");
    }
    for name in ["comment", "tick"] {
        if !commands::named(&templates).iter().any(|(n, _)| *n == name) {
            println!("  {name}: none; runs skip this write and note it");
        }
    }
    let hash = commands::approval_hash(&templates);
    if !commands::approved(&hash)? {
        if !yes(
            ask,
            "Approve these templates to run against your tracker (yes or no)",
            "",
        )? {
            bail!("command templates not approved; nothing written");
        }
        commands::approve(&hash)?;
    }
    Ok(Value::try_from(templates)?)
}

fn yes(ask: &mut Ask, question: &str, default: &str) -> Result<bool> {
    loop {
        match ask.ask(question, default)?.as_str() {
            "yes" => return Ok(true),
            "no" => return Ok(false),
            _ => println!("Answer yes or no."),
        }
    }
}

/// Runs headless `claude -p` in `top` with `prompt` on stdin, and parses the JSON
/// object in its answer.
/// shortcut: no timeout, the user is watching and can interrupt; add one if setup runs unattended.
fn compile(top: &Path, prompt: &str) -> Result<Commands> {
    println!("Compiling command templates with `claude -p`...");
    let mut child = Command::new("claude")
        .arg("-p")
        .current_dir(top)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("launching claude to compile the command templates")?;
    let mut stdin = child.stdin.take().expect("piped");
    stdin.write_all(prompt.as_bytes())?;
    drop(stdin);
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "compiling the command templates: claude exited {}",
            out.status.code().unwrap_or(-1)
        );
    }
    let answer = String::from_utf8_lossy(&out.stdout);
    let json = match (answer.find('{'), answer.rfind('}')) {
        (Some(start), Some(end)) if start < end => &answer[start..=end],
        _ => bail!("the compiling agent's answer holds no JSON object:\n{answer}"),
    };
    serde_json::from_str(json)
        .with_context(|| format!("the compiling agent's templates are not valid:\n{json}"))
}

fn prompt(source: &Source, heading: &str, text: &str) -> String {
    format!(
        r#"Compile shell command templates that let orchestrate reach the tracker described below.
Do not change anything on the tracker while you work.
Answer with one JSON object of string values and nothing else. Its keys:

- get_spec: prints {{"title": string, "body": Markdown string}} for spec {{id}}.
- list_tickets: prints [{{"id": string, "title": string, "body": Markdown string, "finished": bool, "ready": bool}}] for the tickets of spec {{id}}, in tracker order. A ticket is ready when it is marked `{ready}`; finished when it is done or closed.
- list_blockers: prints [{{"id": string, "finished": bool or null when not found}}] for the tickets blocking ticket {{id}}, from the tracker's native links and from a `Blocked by:` line or `## Blocked by` section in its body.
- mark_finished: marks ticket {{id}} finished and prints a JSON object such as {{}}.
- comment (optional): posts the Markdown it reads on stdin as a comment on ticket {{id}} and prints {{}}.
- tick (optional): reads a JSON array of criterion texts on stdin, re-reads ticket {{id}}, ticks the unticked checkboxes with exactly those texts under the headings {headings}, and prints {{"missing": [texts no checkbox has]}}.

Leave out comment or tick if the tracker cannot do it.
Each template runs with `sh -c` in the repo's top level; `{{id}}` is replaced by the already single-quoted id, so do not quote it again.
A template exits non-zero on failure.
Bodies are Markdown; convert other formats.
Reference credentials only through environment variables such as `$TRACKER_TOKEN`; never write a literal secret.

Tracker location: {location}

# {heading}

{text}
"#,
        ready = source.ready,
        headings = source.headings.join(", "),
        location = source.location,
    )
}
