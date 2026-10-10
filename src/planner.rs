//! Planner: decides what can start from a spec reading.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::config::MaxParallel;
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

/// Print a plan the way `plan` shows it: frontier, then waiting, held and warnings.
pub fn print(reading: &Reading, plan: Plan) {
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

/// How many sessions may run at once: the fixed number, or for `auto`
/// `min(graph width, max(2, min(6, cores / 2)))`. Assumes no cycle among open tickets.
pub fn ceiling(max: MaxParallel, reading: &Reading) -> usize {
    if let Some(n) = max.0 {
        return n.get();
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    width(reading).min(2.max(6.min(cores / 2)))
}

/// The most open tickets on one level, a ticket's level being the length of the
/// longest chain of open blockers under it.
fn width(reading: &Reading) -> usize {
    let open: HashMap<&str, &Entry> = reading
        .tickets
        .iter()
        .filter(|e| !e.ticket.finished)
        .map(|e| (e.ticket.id.as_str(), e))
        .collect();
    let mut levels = HashMap::new();
    let mut per_level: HashMap<usize, usize> = HashMap::new();
    for e in open.values() {
        *per_level.entry(level(e, &open, &mut levels)).or_default() += 1;
    }
    per_level.into_values().max().unwrap_or(0)
}

fn level<'a>(
    e: &'a Entry,
    open: &HashMap<&str, &'a Entry>,
    levels: &mut HashMap<&'a str, usize>,
) -> usize {
    if let Some(l) = levels.get(e.ticket.id.as_str()) {
        return *l;
    }
    let l = e
        .blockers
        .iter()
        .filter_map(|b| open.get(b.id.as_str()))
        .map(|b| level(b, open, levels) + 1)
        .max()
        .unwrap_or(0);
    levels.insert(&e.ticket.id, l);
    l
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
