//! Local markdown tracker. Layout under the configured `location`:
//! `<spec>/spec.md` is the spec, every other `<spec>/<ticket>.md` is a ticket
//! (id = file stem, tracker order = file name order). A ticket's `Status:` line
//! decides `ready` (equal to the ready marker) and `finished` (`done`). There are
//! no native links, so blockers are only the body forms, as ids or links to ticket files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{Blocker, Spec, Ticket, Tracker};
use crate::markdown;

const SPEC_FILE: &str = "spec.md";

pub struct LocalMarkdown {
    pub root: PathBuf,
    pub ready: String,
}

impl LocalMarkdown {
    fn spec_dir(&self, spec: &str) -> Result<PathBuf> {
        let dir = self.root.join(spec);
        if !dir.join(SPEC_FILE).is_file() {
            bail!(
                "spec `{spec}` not found: no {}",
                dir.join(SPEC_FILE).display()
            );
        }
        Ok(dir)
    }

    /// shortcut: the first `<spec>/<id>.md` across all specs wins; ids are assumed unique in `location`.
    fn find_ticket(&self, id: &str) -> Result<Option<PathBuf>> {
        let file = format!("{id}.md");
        for entry in read_dir_sorted(&self.root)? {
            let path = entry.join(&file);
            if file != SPEC_FILE && path.is_file() {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }
}

impl Tracker for LocalMarkdown {
    fn get_spec(&self, spec: &str) -> Result<Spec> {
        let body = read(&self.spec_dir(spec)?.join(SPEC_FILE))?;
        Ok(Spec {
            id: spec.to_string(),
            title: title(&body).unwrap_or(spec).to_string(),
            body,
        })
    }

    fn list_tickets(&self, spec: &str) -> Result<Vec<Ticket>> {
        let mut tickets = Vec::new();
        for path in read_dir_sorted(&self.spec_dir(spec)?)? {
            let is_md = path.extension().is_some_and(|e| e == "md");
            if !is_md || path.file_name().is_some_and(|n| n == SPEC_FILE) {
                continue;
            }
            let id = path.file_stem().unwrap().to_string_lossy().into_owned();
            let text = read(&path)?;
            let status = line_value(&text, "Status:").unwrap_or_default();
            tickets.push(Ticket {
                title: title(&text).unwrap_or(&id).to_string(),
                finished: status == "done",
                ready: status == self.ready,
                id,
                body: ticket_body(&text),
            });
        }
        Ok(tickets)
    }

    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>> {
        let Some(path) = self.find_ticket(ticket)? else {
            bail!("ticket `{ticket}` not found under {}", self.root.display());
        };
        let mut blockers = Vec::new();
        for r in markdown::blocked_by(&read(&path)?) {
            let id = local_id(&r).to_string();
            let finished = match self.find_ticket(&id)? {
                Some(p) => Some(line_value(&read(&p)?, "Status:") == Some("done")),
                None => None,
            };
            blockers.push(Blocker { id, finished });
        }
        Ok(blockers)
    }
}

/// The ticket id a blocker reference names: a bare id, or a link or path to its file.
fn local_id(r: &str) -> &str {
    let name = r.rsplit('/').next().unwrap_or(r);
    name.strip_suffix(".md").unwrap_or(name)
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    let mut paths = entries
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

/// The file without its title and `Status:` lines, which are tracker fields, not body.
fn ticket_body(text: &str) -> String {
    let (mut title, mut status) = (false, false);
    let keep = |l: &&str| {
        if !title && l.starts_with("# ") {
            title = true;
        } else if !status && l.trim_start().starts_with("Status:") {
            status = true;
        } else {
            return true;
        }
        false
    };
    text.lines().filter(keep).collect::<Vec<_>>().join("\n")
}

/// The text of the first `# ` heading.
fn title(body: &str) -> Option<&str> {
    body.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(str::trim)
}

/// The trimmed value of the first line starting with `prefix`.
fn line_value<'a>(body: &'a str, prefix: &str) -> Option<&'a str> {
    body.lines()
        .find_map(|l| l.trim_start().strip_prefix(prefix))
        .map(str::trim)
}
