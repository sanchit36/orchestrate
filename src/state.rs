//! Run state store: where a run keeps its state, worktrees and logs, and its `state.json`.
//! Nothing is written in or beside the working tree.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::MachineConfig;
use crate::git::git;
use crate::lock::{self, Lock};
use crate::report::Criterion;

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
#[derive(Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub run_id: String,
    pub spec: String,
    pub branch: String,
    /// When the run started, as a jiff timestamp.
    pub started: String,
    /// `running`, `stopped` or `ended`.
    pub status: String,
    /// Why a running run is paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<String>,
    pub tickets: BTreeMap<String, TicketState>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct TicketState {
    pub title: String,
    pub outcome: String,
    /// Why a ticket failed, timed out, got stuck, or was held or skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The criteria a stuck session reported unmet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmet: Vec<String>,
    /// When its session started, as a jiff timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    /// The criteria as the session reported them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<Criterion>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The gates run on its landing attempt, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<GateResult>,
    /// Criteria whose text changed on the tracker since dispatch, so were not ticked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mismatches: Vec<String>,
    /// Ticks and comments that failed or were skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writeback: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct GateResult {
    pub name: String,
    pub code: i32,
}

impl State {
    pub fn new(run_id: &str, spec: &str, branch: &str) -> Self {
        Self {
            version: VERSION,
            run_id: run_id.into(),
            spec: spec.into(),
            branch: branch.into(),
            started: jiff::Timestamp::now().to_string(),
            status: "running".into(),
            paused: None,
            tickets: BTreeMap::new(),
        }
    }

    /// The `state.json` in `run_dir`.
    pub fn load(run_dir: &Path) -> Result<Self> {
        let path = run_dir.join("state.json");
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    fn save(&self, run_dir: &Path) -> Result<()> {
        let path = run_dir.join("state.json");
        let tmp = run_dir.join("state.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }
}

/// A run's state, saved to `state.json` after every change. Shareable between the
/// run loop and the tracker's pause hook.
pub struct Store {
    run_dir: PathBuf,
    state: Mutex<State>,
}

impl Store {
    pub fn create(dirs: &RunDirs, state: State) -> Result<Self> {
        state.save(&dirs.run)?;
        Ok(Self {
            run_dir: dirs.run.clone(),
            state: Mutex::new(state),
        })
    }

    /// Apply `f` and save.
    pub fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> Result<R> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut state);
        state.save(&self.run_dir)?;
        Ok(r)
    }

    /// Mark the run stopped and hold off every later save in this process: the caller
    /// is about to exit.
    pub fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.status = "stopped".into();
        let saved = state.save(&self.run_dir);
        std::mem::forget(state);
        saved
    }
}

/// Every run of the repo whose state is in `repo_dir`, most recent first.
pub fn runs(repo_dir: &Path) -> Result<Vec<State>> {
    let dir = repo_dir.join("runs");
    let entries = match fs::read_dir(&dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        r => r.with_context(|| format!("reading {}", dir.display()))?,
    };
    let mut runs = Vec::new();
    for e in entries {
        runs.push(State::load(&e?.path())?);
    }
    let started = |s: &State| s.started.parse::<jiff::Timestamp>().ok();
    runs.sort_by_key(|s| std::cmp::Reverse(started(s)));
    Ok(runs)
}

/// The run `--run` names: an exact id, a unique id prefix, a spec id (its most recent
/// run), or, when omitted, the most recent run. `runs` is most recent first.
pub fn resolve(runs: Vec<State>, arg: Option<&str>) -> Result<State> {
    let Some(arg) = arg else {
        return runs.into_iter().next().context("no runs in this repo");
    };
    let prefixed: Vec<&State> = runs.iter().filter(|s| s.run_id.starts_with(arg)).collect();
    let pick = match prefixed.iter().position(|s| s.run_id == arg) {
        Some(i) => Some(prefixed[i]),
        None if prefixed.len() == 1 => Some(prefixed[0]),
        None => runs.iter().find(|s| s.spec == arg),
    };
    let Some(id) = pick.map(|s| s.run_id.clone()) else {
        if prefixed.is_empty() {
            bail!("no run matches {arg}");
        }
        let ids: Vec<_> = prefixed.iter().map(|s| s.run_id.as_str()).collect();
        bail!("{arg} matches several runs: {}", ids.join(", "));
    };
    Ok(runs
        .into_iter()
        .find(|s| s.run_id == id)
        .expect("picked from runs"))
}
