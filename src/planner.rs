//! Planner: decides what can start from a spec reading.

use crate::spec::Reading;
use crate::tracker::Ticket;

/// Open, ready tickets whose blockers have all finished, in tracker order.
pub fn frontier(reading: &Reading) -> Vec<&Ticket> {
    reading
        .tickets
        .iter()
        .filter(|(t, blockers)| !t.finished && t.ready && blockers.iter().all(|b| b.finished))
        .map(|(t, _)| t)
        .collect()
}
