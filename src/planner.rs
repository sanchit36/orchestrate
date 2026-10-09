//! Planner: decides what can start from a spec reading.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::spec::{Entry, Reading};
use crate::tracker::Ticket;

pub struct Plan<'a> {
    /// Open, ready tickets whose blockers have all finished, in tracker order.
    pub frontier: Vec<&'a Ticket>,
    /// Open tickets held back, with why: a problem of their own or a held-back blocker.
    pub held: Vec<(&'a Ticket, String)>,
    /// Ready tickets blocked by an open ticket that is not ready, with why.
    pub waiting: Vec<(&'a Ticket, String)>,
}

/// Refuses a blocking cycle among open tickets, naming its tickets.
pub fn plan(reading: &Reading) -> Result<Plan<'_>> {
    if let Some(cycle) = find_cycle(reading) {
        bail!("blocking cycle: {} -> {}", cycle.join(" -> "), cycle[0]);
    }
    let open: Vec<&Entry> = reading
        .tickets
        .iter()
        .filter(|e| !e.ticket.finished)
        .collect();
    let mut held: HashMap<&str, String> = open
        .iter()
        .filter_map(|e| Some((e.ticket.id.as_str(), e.hold.clone()?)))
        .collect();
    // Spread holds to dependents until nothing changes; no cycles remain, so this ends.
    loop {
        let before = held.len();
        for e in &open {
            if held.contains_key(e.ticket.id.as_str()) {
                continue;
            }
            if let Some(b) = e.blockers.iter().find(|b| held.contains_key(b.id.as_str())) {
                held.insert(&e.ticket.id, format!("blocked by held-back {}", b.id));
            }
        }
        if held.len() == before {
            break;
        }
    }
    let not_ready: HashSet<&str> = open
        .iter()
        .filter(|e| !e.ticket.ready)
        .map(|e| e.ticket.id.as_str())
        .collect();

    let mut plan = Plan {
        frontier: Vec::new(),
        held: Vec::new(),
        waiting: Vec::new(),
    };
    for e in open {
        let t = &e.ticket;
        if let Some(why) = held.remove(t.id.as_str()) {
            plan.held.push((t, why));
        } else if !t.ready {
            continue;
        } else if let Some(b) = e
            .blockers
            .iter()
            .find(|b| not_ready.contains(b.id.as_str()))
        {
            let why = format!("blocked by {}, which is not ready", b.id);
            plan.waiting.push((t, why));
        } else if e.blockers.iter().all(|b| b.finished == Some(true)) {
            plan.frontier.push(t);
        }
    }
    Ok(plan)
}

/// The first cycle of open tickets blocked by each other, in tracker order.
fn find_cycle(reading: &Reading) -> Option<Vec<&str>> {
    let open: HashMap<&str, Vec<&str>> = reading
        .tickets
        .iter()
        .filter(|e| !e.ticket.finished)
        .map(|e| {
            let ids = e.blockers.iter().map(|b| b.id.as_str()).collect();
            (e.ticket.id.as_str(), ids)
        })
        .collect();
    let mut done = HashSet::new();
    reading
        .tickets
        .iter()
        .find_map(|e| visit(&e.ticket.id, &open, &mut Vec::new(), &mut done))
}

fn visit<'a>(
    id: &'a str,
    open: &HashMap<&'a str, Vec<&'a str>>,
    stack: &mut Vec<&'a str>,
    done: &mut HashSet<&'a str>,
) -> Option<Vec<&'a str>> {
    if let Some(at) = stack.iter().position(|s| *s == id) {
        return Some(stack[at..].to_vec());
    }
    let blockers = open.get(id)?;
    if !done.insert(id) {
        return None;
    }
    stack.push(id);
    let cycle = blockers.iter().find_map(|b| visit(b, open, stack, done));
    stack.pop();
    cycle
}
