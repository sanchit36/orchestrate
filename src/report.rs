//! The report a session leaves at its worktree root.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const REPORT_FILE: &str = ".orchestrate-result.json";

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Done,
    Stuck,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Criterion {
    pub text: String,
    pub met: bool,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Deserialize)]
pub struct Report {
    pub status: Status,
    pub acceptance_criteria: Vec<Criterion>,
    #[serde(default)]
    pub notes: String,
}

impl Report {
    /// The report in `worktree`; an error when it is missing or malformed.
    pub fn read(worktree: &Path) -> Result<Self> {
        let path = worktree.join(REPORT_FILE);
        let text = fs::read_to_string(&path).context("no report")?;
        serde_json::from_str(&text).context("malformed report")
    }

    /// Count each of `criteria` (as read at dispatch) the report leaves out as unmet.
    pub fn cover(&mut self, criteria: &[String]) {
        for text in criteria {
            if !self.acceptance_criteria.iter().any(|c| &c.text == text) {
                self.acceptance_criteria.push(Criterion {
                    text: text.clone(),
                    met: false,
                    evidence: "left out of the report".into(),
                });
            }
        }
    }

    /// The texts of the criteria reported unmet.
    pub fn unmet(&self) -> Vec<String> {
        let unmet = self.acceptance_criteria.iter().filter(|c| !c.met);
        unmet.map(|c| c.text.clone()).collect()
    }

    /// `done` with every criterion met.
    pub fn is_done(&self) -> bool {
        matches!(self.status, Status::Done) && self.unmet().is_empty()
    }
}

/// Of `criteria`, the ones `reported` marks met: the only ones ever ticked.
pub fn met(reported: &[Criterion], criteria: &[String]) -> Vec<String> {
    let met = |t: &&String| reported.iter().any(|c| c.met && &c.text == *t);
    criteria.iter().filter(met).cloned().collect()
}
