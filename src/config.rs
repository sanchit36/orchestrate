//! Config: loads and validates the repo and machine config files.
//! Both carry `version = 1`; each key is valid in exactly one file.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer};

const REPO_CONFIG: &str = ".orchestrate/config.toml";
const VERSION: i64 = 1;

/// Top-level keys of each file, besides `version`.
const REPO_KEYS: &[&str] = &[
    "tracker",
    "criteria_headings",
    "gates",
    "session",
    "seed_files",
    "prepare",
    "protected_branches",
];
const MACHINE_KEYS: &[&str] = &["max_parallel", "notifications", "state_dir"];

#[derive(Deserialize)]
pub struct RepoConfig {
    pub tracker: TrackerConfig,
    #[serde(default = "default_criteria_headings")]
    pub criteria_headings: Vec<String>,
    #[serde(default)]
    pub gates: Vec<Gate>,
    #[serde(default)]
    pub session: SessionConfig,
    /// Local files (relative to the top level) copied into each fresh worktree.
    #[serde(default)]
    pub seed_files: Vec<PathBuf>,
    /// A shell command run in each fresh worktree before its session.
    pub prepare: Option<String>,
    /// Branches a run never writes; `None` means the remote's default branch.
    pub protected_branches: Option<Vec<String>>,
}

/// `[[gates]]`: a shell command run in the rebased ticket worktree; exit 0 passes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub name: String,
    pub run: String,
    /// Globs against the ticket's changed files; empty means all files.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Folders (`*` allowed per segment); the gate runs once in each one the change touched.
    #[serde(default)]
    pub dirs: Vec<String>,
    /// Fixed working directory relative to the top level, for a gate without `dirs`.
    pub cwd: Option<String>,
    /// How long the gate may run before it is killed and fails.
    #[serde(default = "default_gate_timeout", deserialize_with = "duration")]
    pub timeout: Duration,
    /// A shell command run after the gate in the same directory, even after a timeout.
    pub teardown: Option<String>,
}

fn default_gate_timeout() -> Duration {
    Duration::from_secs(20 * 60)
}

/// `[session]`: how sessions are launched and prompted.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// A repo file (relative to the top level) appended to the built-in prompt.
    pub instructions: Option<PathBuf>,
    /// Skill names the prompt points the session at.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Wall-clock limit of a session.
    #[serde(default = "default_session_timeout", deserialize_with = "duration")]
    pub timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            permission_mode: default_permission_mode(),
            instructions: None,
            skills: Vec::new(),
            timeout: default_session_timeout(),
        }
    }
}

fn default_session_timeout() -> Duration {
    Duration::from_secs(45 * 60)
}

fn default_permission_mode() -> String {
    "bypassPermissions".into()
}

/// The machine file; keys not parsed yet are only validated by name.
#[derive(Deserialize, Default)]
pub struct MachineConfig {
    pub state_dir: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackerConfig {
    pub kind: TrackerKind,
    pub location: String,
    #[serde(default = "default_ready")]
    pub ready: String,
    /// Wait between the quick retries of a failed tracker operation.
    #[serde(default = "default_retry_delay", deserialize_with = "duration")]
    pub retry_delay: Duration,
    /// First wait between probes of a paused run; it doubles up to `probe_backoff_max`.
    #[serde(default = "default_probe_backoff", deserialize_with = "duration")]
    pub probe_backoff: Duration,
    #[serde(default = "default_probe_backoff_max", deserialize_with = "duration")]
    pub probe_backoff_max: Duration,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrackerKind {
    LocalMarkdown,
}

fn default_ready() -> String {
    "ready-for-agent".into()
}

fn default_retry_delay() -> Duration {
    Duration::from_secs(2)
}

fn default_probe_backoff() -> Duration {
    Duration::from_secs(60)
}

fn default_probe_backoff_max() -> Duration {
    Duration::from_secs(5 * 60)
}

/// A duration such as `"2s"`, `"5m"` or `"PT1M"`.
fn duration<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let text = String::deserialize(d)?;
    let parsed: jiff::SignedDuration = text.parse().map_err(serde::de::Error::custom)?;
    Duration::try_from(parsed).map_err(serde::de::Error::custom)
}

fn default_criteria_headings() -> Vec<String> {
    vec!["Acceptance criteria".into(), "Acceptance".into()]
}

/// Load both config files for the repo whose git top level is `top`.
/// The repo config is required; the machine config is optional.
pub fn load(top: &Path) -> Result<(RepoConfig, MachineConfig)> {
    let repo_path = top.join(REPO_CONFIG);
    let machine_path = machine_config_path()?;
    let Some(repo) = read(&repo_path)? else {
        bail!(
            "no repo config at {}; run `orchestrate setup` first",
            repo_path.display()
        );
    };
    validate(&repo_path, &repo, REPO_KEYS, MACHINE_KEYS, &machine_path)?;
    let machine = match read(&machine_path)? {
        Some(machine) => {
            validate(&machine_path, &machine, MACHINE_KEYS, REPO_KEYS, &repo_path)?;
            machine
                .try_into()
                .with_context(|| format!("{}", machine_path.display()))?
        }
        None => MachineConfig::default(),
    };
    let repo: RepoConfig = repo
        .try_into()
        .with_context(|| format!("{}", repo_path.display()))?;
    for gate in &repo.gates {
        check_gate(gate).with_context(|| format!("{}", repo_path.display()))?;
    }
    Ok((repo, machine))
}

fn check_gate(gate: &Gate) -> Result<()> {
    if !gate.dirs.is_empty() && gate.cwd.is_some() {
        bail!("gate {}: set `dirs` or `cwd`, not both", gate.name);
    }
    for pattern in gate.paths.iter().chain(&gate.dirs) {
        crate::gate::glob(pattern).with_context(|| format!("gate {}", gate.name))?;
    }
    Ok(())
}

fn machine_config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/orchestrate/config.toml"))
}

/// The parsed file, or `None` when it does not exist.
fn read(path: &Path) -> Result<Option<toml::Table>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let table = text
        .parse()
        .with_context(|| format!("{}", path.display()))?;
    Ok(Some(table))
}

fn validate(
    path: &Path,
    table: &toml::Table,
    own: &[&str],
    other: &[&str],
    other_path: &Path,
) -> Result<()> {
    let path = path.display();
    match table.get("version") {
        Some(toml::Value::Integer(VERSION)) => {}
        None => bail!("{path}: missing `version = {VERSION}`"),
        Some(v) => bail!("{path}: unsupported version {v}, expected `version = {VERSION}`"),
    }
    for key in table.keys().filter(|k| *k != "version") {
        if other.contains(&key.as_str()) {
            bail!(
                "{path}: key `{key}` belongs in {}, not here",
                other_path.display()
            );
        }
        if !own.contains(&key.as_str()) {
            bail!("{path}: unknown key `{key}`");
        }
    }
    Ok(())
}
