//! Tracker adapter: the one contract every tracker meets (ADR 0001).
//! Ids are opaque strings; any translation stays inside the adapter.

#[cfg(test)]
pub(crate) mod contract;
mod local_markdown;
mod pausing;
mod remote;

use std::path::Path;

use anyhow::Result;

use crate::config::{RepoConfig, TrackerKind};

pub use pausing::Pausing;
pub use remote::{Msg, Remote};

#[derive(Clone)]
pub struct Spec {
    pub id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone)]
pub struct Ticket {
    pub id: String,
    pub title: String,
    pub body: String,
    pub finished: bool,
    pub ready: bool,
}

#[derive(Clone)]
pub struct Blocker {
    pub id: String,
    /// `None` when the blocker is not found on the tracker.
    pub finished: Option<bool>,
}

/// The read operations of the adapter contract. Tickets come back in tracker order.
/// `list_blockers` combines native links with the body forms (`markdown::blocked_by`).
pub trait Tracker {
    fn get_spec(&self, spec: &str) -> Result<Spec>;
    fn list_tickets(&self, spec: &str) -> Result<Vec<Ticket>>;
    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>>;
    /// Record that the ticket landed on the integration branch.
    fn mark_finished(&self, ticket: &str) -> Result<()>;

    /// Optional: add a Markdown comment to the ticket. Fails with [`Unsupported`] by default.
    fn comment(&self, _ticket: &str, _markdown: &str) -> Result<()> {
        Err(Unsupported.into())
    }

    /// Optional: re-read the ticket and tick its unticked criteria whose text is in
    /// `criteria`. Returns the texts no criterion has any more (edited since they were
    /// read). Fails with [`Unsupported`] by default.
    fn tick(&self, _ticket: &str, _criteria: &[String]) -> Result<Vec<String>> {
        Err(Unsupported.into())
    }
}

/// The error of an optional operation an adapter does not have.
#[derive(Debug)]
pub struct Unsupported;

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("not supported by this tracker")
    }
}

impl std::error::Error for Unsupported {}

/// The adapter the repo config names, rooted at the repo's top level.
pub fn open(config: &RepoConfig, top: &Path) -> Box<dyn Tracker> {
    match config.tracker.kind {
        TrackerKind::LocalMarkdown => Box::new(local_markdown::LocalMarkdown {
            root: top.join(&config.tracker.location),
            ready: config.tracker.ready.clone(),
            criteria_headings: config.criteria_headings.clone(),
        }),
    }
}
