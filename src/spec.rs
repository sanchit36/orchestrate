//! Spec reader: turns adapter answers into one reading of a spec and its tickets,
//! and classifies what it finds: refuse (an error), hold back a ticket, or warn.

use anyhow::{Result, bail};

use crate::markdown::{self, Criterion};
use crate::tracker::{Blocker, Spec, Ticket, Tracker};

pub struct Reading {
    pub spec: Spec,
    /// Tickets in tracker order.
    pub tickets: Vec<Entry>,
    /// Problems that never stop or hold anything, such as missing criteria.
    pub warnings: Vec<String>,
}

pub struct Entry {
    pub ticket: Ticket,
    pub blockers: Vec<Blocker>,
    pub criteria: Vec<Criterion>,
    /// Why this ticket is held back (with every ticket blocked by it), if it is.
    pub hold: Option<String>,
}

/// A reading for a run already under way, where every ticket having landed is normal.
pub fn reread(tracker: &dyn Tracker, spec: &str, criteria_headings: &[String]) -> Result<Reading> {
    let spec = tracker.get_spec(spec)?;
    let mut warnings = Vec::new();
    let tickets = tracker
        .list_tickets(&spec.id)?
        .into_iter()
        .map(|ticket| {
            let blockers = tracker.list_blockers(&ticket.id)?;
            let criteria = markdown::criteria(&ticket.body, criteria_headings);
            if !ticket.finished {
                match &criteria {
                    None => warnings.push(format!("{}: no acceptance criteria", ticket.id)),
                    Some(c) if c.is_empty() => warnings.push(format!(
                        "{}: acceptance criteria heading has no checkboxes",
                        ticket.id
                    )),
                    Some(_) => {}
                }
            }
            let unknown = blockers.iter().find(|b| b.finished.is_none());
            let hold = if ticket.body.trim().is_empty() {
                Some("empty body".to_string())
            } else {
                unknown.map(|b| format!("blocker {} not found", b.id))
            };
            Ok(Entry {
                hold,
                ticket,
                blockers,
                criteria: criteria.unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<Entry>>>()?;
    if tickets.is_empty() {
        bail!("spec `{}` has no tickets", spec.id);
    }
    Ok(Reading {
        spec,
        tickets,
        warnings,
    })
}

/// [`reread`], and refuses a spec with no open ready ticket: the checks before a start.
pub fn read(tracker: &dyn Tracker, spec: &str, criteria_headings: &[String]) -> Result<Reading> {
    let reading = reread(tracker, spec, criteria_headings)?;
    let tickets = &reading.tickets;
    if !tickets.iter().any(|e| e.ticket.ready && !e.ticket.finished) {
        bail!("spec `{}` has no ready tickets", reading.spec.id);
    }
    Ok(reading)
}
