//! Run state store: where a run keeps its state, worktrees and logs, and its `state.json`.
//! Nothing is written in or beside the working tree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::MachineConfig;
use crate::git::git;

const VERSION: u32 = 1;

/// Paths of one run under `<root>/<repo-folder>-<hash>/`.
pub struct RunDirs {
    pub id: String,
    pub run: PathBuf,
    pub logs: PathBuf,
    pub worktrees: PathBuf,
}

impl RunDirs {
    /// Create the per-repo dir (with `repo.toml`) and the run dirs for a new run of `spec`.
    pub fn create(machine: &MachineConfig, top: &Path, spec: &str) -> Result<Self> {
        let repo_dir = state_root(machine)?.join(repo_dir_name(top)?);
        fs::create_dir_all(&repo_dir)
            .with_context(|| format!("creating {}", repo_dir.display()))?;
        let path = toml::Value::String(top.to_string_lossy().into_owned());
        fs::write(repo_dir.join("repo.toml"), format!("path = {path}\n"))?;
        let now = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S");
        let id = format!("{}-{now}", slug(spec));
        let run = repo_dir.join("runs").join(&id);
        let logs = run.join("logs");
        fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
        Ok(Self {
            worktrees: repo_dir.join("worktrees").join(&id),
            id,
            run,
            logs,
        })
    }
}

/// `state_dir`, else `$XDG_STATE_HOME/orchestrate`, else `~/.local/state/orchestrate`.
fn state_root(machine: &MachineConfig) -> Result<PathBuf> {
    if let Some(dir) = &machine.state_dir {
        return Ok(dir.clone());
    }
    match std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        Some(xdg) => Ok(PathBuf::from(xdg).join("orchestrate")),
        None => {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            Ok(PathBuf::from(home).join(".local/state/orchestrate"))
        }
    }
}

/// `<repo-folder>-<8-char hash of the canonical git common dir>`.
fn repo_dir_name(top: &Path) -> Result<String> {
    let common = git(
        top,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = fs::canonicalize(&common).with_context(|| format!("resolving {common}"))?;
    let folder = top.file_name().unwrap_or_default().to_string_lossy();
    let hash = format!("{:016x}", fnv1a(common.to_string_lossy().as_bytes()));
    Ok(format!("{folder}-{}", &hash[..8]))
}

/// FNV-1a 64: stable across builds, unlike std's hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

/// Lowercase; each run of non-alphanumerics becomes `-`; trimmed; at most 40 characters.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let capped: String = out.trim_matches('-').chars().take(40).collect();
    capped.trim_end_matches('-').to_string()
}

/// `state.json`: the run and the latest tickets seen.
#[derive(Serialize)]
pub struct State {
    pub version: u32,
    pub run_id: String,
    pub spec: String,
    pub branch: String,
    pub tickets: BTreeMap<String, TicketState>,
}

#[derive(Serialize)]
pub struct TicketState {
    pub title: String,
    pub outcome: String,
    /// Why a ticket failed, timed out, got stuck, or was held or skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The criteria a stuck session reported unmet.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unmet: Vec<String>,
}

impl State {
    pub fn new(run_id: &str, spec: &str, branch: &str) -> Self {
        Self {
            version: VERSION,
            run_id: run_id.into(),
            spec: spec.into(),
            branch: branch.into(),
            tickets: BTreeMap::new(),
        }
    }

    pub fn save(&self, dirs: &RunDirs) -> Result<()> {
        let path = dirs.run.join("state.json");
        let tmp = dirs.run.join("state.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }
}
