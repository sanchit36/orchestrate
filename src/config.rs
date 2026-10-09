//! Config: loads and validates the repo and machine config files.
//! Both carry `version = 1`; each key is valid in exactly one file.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const REPO_CONFIG: &str = ".orchestrate/config.toml";
const VERSION: i64 = 1;

/// Top-level keys of each file, besides `version`.
const REPO_KEYS: &[&str] = &["tracker", "criteria_headings", "gates", "session"];
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
}

/// `[[gates]]`: a shell command run in the rebased ticket worktree; exit 0 passes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub name: String,
    pub run: String,
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
    /// Wall-clock limit of a session, in seconds.
    #[serde(default = "default_session_timeout")]
    pub timeout: u64,
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

fn default_session_timeout() -> u64 {
    45 * 60
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
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrackerKind {
    LocalMarkdown,
}

fn default_ready() -> String {
    "ready-for-agent".into()
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
    let repo = repo
        .try_into()
        .with_context(|| format!("{}", repo_path.display()))?;
    Ok((repo, machine))
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
