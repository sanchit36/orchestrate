//! Run state store: where a run keeps its state, worktrees and logs, and its `state.json`.
//! Nothing is written in or beside the working tree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::MachineConfig;
use crate::git::git;
use crate::lock::{self, Lock};

const VERSION: u32 = 1;

/// Paths of one run under `<root>/<repo-folder>-<hash>/`.
pub struct RunDirs {
    pub id: String,
    pub run: PathBuf,
    pub logs: PathBuf,
    pub worktrees: PathBuf,
    pub lock: Lock,
}

impl RunDirs {
    /// Create the per-repo dir (with `repo.toml`), take the repo lock, and create the
    /// run dirs for a new run of `spec`.
    pub fn create(machine: &MachineConfig, top: &Path, spec: &str) -> Result<Self> {
        let repo_dir = repo_dir(machine, top)?;
        fs::create_dir_all(&repo_dir)
            .with_context(|| format!("creating {}", repo_dir.display()))?;
        let path = toml::Value::String(top.to_string_lossy().into_owned());
        fs::write(repo_dir.join("repo.toml"), format!("path = {path}\n"))?;
        // Ids are to the second: a run of the same spec within the same second as the
        // last one waits for the next second rather than reuse its dirs.
        let id = loop {
            let now = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S");
            let id = format!("{}-{now}", slug(spec));
            if !repo_dir.join("runs").join(&id).exists() {
                break id;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let lock = lock::acquire(&repo_dir, &id)?;
        let run = repo_dir.join("runs").join(&id);
        let logs = run.join("logs");
        fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
        Ok(Self {
            worktrees: repo_dir.join("worktrees").join(&id),
            id,
            run,
            logs,
            lock,
        })
    }
}

/// The per-repo state dir: `<root>/<repo-folder>-<hash>`.
pub fn repo_dir(machine: &MachineConfig, top: &Path) -> Result<PathBuf> {
    Ok(state_root(machine)?.join(repo_dir_name(top)?))
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
    /// `running`, `stopped` or `ended`.
    pub status: String,
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
    /// Criteria edited since dispatch, which landing left unticked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mismatches: Vec<String>,
    /// Ticks and comments that failed or were skipped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub writeback: Vec<String>,
}

impl State {
    pub fn new(run_id: &str, spec: &str, branch: &str) -> Self {
        Self {
            version: VERSION,
            run_id: run_id.into(),
            spec: spec.into(),
            branch: branch.into(),
            status: "running".into(),
            tickets: BTreeMap::new(),
        }
    }

    pub fn save(&self, dirs: &RunDirs) -> Result<()> {
        let _saving = SAVES.lock().unwrap();
        write(&dirs.run, &serde_json::to_value(self)?)
    }
}

/// Serialises `state.json` writes; a stop takes it for good.
static SAVES: Mutex<()> = Mutex::new(());

/// Mark the run in `run` stopped, and hold off every later save in this process.
pub fn mark_stopped(run: &Path) -> Result<()> {
    std::mem::forget(SAVES.lock().unwrap());
    let path = run.join("state.json");
    let mut state: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    state["status"] = "stopped".into();
    write(run, &state)
}

fn write(run: &Path, state: &serde_json::Value) -> Result<()> {
    let path = run.join("state.json");
    let tmp = run.join("state.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(state)?)?;
    fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
}
