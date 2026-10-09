//! Spec reader: turns adapter answers into one reading of a spec and its tickets.

use anyhow::Result;

use crate::tracker::{Blocker, Spec, Ticket, Tracker};

pub struct Reading {
    pub spec: Spec,
    /// Tickets in tracker order, each with its blockers.
    pub tickets: Vec<(Ticket, Vec<Blocker>)>,
}

pub fn read(tracker: &dyn Tracker, spec: &str) -> Result<Reading> {
    let spec = tracker.get_spec(spec)?;
    let tickets = tracker
        .list_tickets(&spec.id)?
        .into_iter()
        .map(|t| {
            let blockers = tracker.list_blockers(&t.id)?;
            Ok((t, blockers))
        })
        .collect::<Result<_>>()?;
    Ok(Reading { spec, tickets })
}
