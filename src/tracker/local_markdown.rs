//! Local markdown tracker. Layout under the configured `location`:
//! `<spec>/spec.md` is the spec, every other `<spec>/<ticket>.md` is a ticket
//! (id = file stem, tracker order = file name order). A ticket's `Status:` line
//! decides `ready` (equal to the ready marker) and `finished` (`done`); its
//! `Blocked by:` line lists blocker ids.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{Blocker, Spec, Ticket, Tracker};

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
            let body = read(&path)?;
            let status = line_value(&body, "Status:").unwrap_or_default();
            tickets.push(Ticket {
                title: title(&body).unwrap_or(&id).to_string(),
                finished: status == "done",
                ready: status == self.ready,
                id,
                body,
            });
        }
        Ok(tickets)
    }

    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>> {
        let Some(path) = self.find_ticket(ticket)? else {
            bail!("ticket `{ticket}` not found under {}", self.root.display());
        };
        let body = read(&path)?;
        let ids = line_value(&body, "Blocked by:").unwrap_or_default();
        let mut blockers = Vec::new();
        for id in ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            // shortcut: an unknown blocker counts as unfinished; ticket #17 holds it back instead.
            let finished = match self.find_ticket(id)? {
                Some(p) => line_value(&read(&p)?, "Status:") == Some("done"),
                None => false,
            };
            blockers.push(Blocker {
                id: id.to_string(),
                finished,
            });
        }
        Ok(blockers)
    }
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
