//! The report a session leaves at its worktree root.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

pub const REPORT_FILE: &str = ".orchestrate-result.json";

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Done,
    Stuck,
}

#[derive(Deserialize)]
pub struct Criterion {
    pub met: bool,
}

#[derive(Deserialize)]
pub struct Report {
    pub status: Status,
    pub acceptance_criteria: Vec<Criterion>,
}

impl Report {
    /// The report in `worktree`; an error when it is missing or malformed.
    pub fn read(worktree: &Path) -> Result<Self> {
        let path = worktree.join(REPORT_FILE);
        let text = fs::read_to_string(&path).context("no report")?;
        serde_json::from_str(&text).context("malformed report")
    }

    /// `done` with every criterion met.
    pub fn is_done(&self) -> bool {
        matches!(self.status, Status::Done) && self.acceptance_criteria.iter().all(|c| c.met)
    }
}
