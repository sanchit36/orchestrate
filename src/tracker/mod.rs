//! Tracker adapter: the one contract every tracker meets (ADR 0001).
//! Ids are opaque strings; any translation stays inside the adapter.

mod local_markdown;

use std::path::Path;

use anyhow::Result;

use crate::config::{RepoConfig, TrackerKind};

pub struct Spec {
    pub id: String,
    pub title: String,
    pub body: String,
}

pub struct Ticket {
    pub id: String,
    pub title: String,
    pub body: String,
    pub finished: bool,
    pub ready: bool,
}

pub struct Blocker {
    #[expect(dead_code, reason = "contract field, first read by run (#17, #18)")]
    pub id: String,
    pub finished: bool,
}

/// The read operations of the adapter contract. Tickets come back in tracker order.
pub trait Tracker {
    fn get_spec(&self, spec: &str) -> Result<Spec>;
    fn list_tickets(&self, spec: &str) -> Result<Vec<Ticket>>;
    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>>;
    /// Record that the ticket landed on the integration branch.
    fn mark_finished(&self, ticket: &str) -> Result<()>;
}

/// The adapter the repo config names, rooted at the repo's top level.
pub fn open(config: &RepoConfig, top: &Path) -> Box<dyn Tracker> {
    match config.tracker.kind {
        TrackerKind::LocalMarkdown => Box::new(local_markdown::LocalMarkdown {
            root: top.join(&config.tracker.location),
            ready: config.tracker.ready.clone(),
        }),
    }
}
