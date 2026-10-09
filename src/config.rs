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
const REPO_KEYS: &[&str] = &["tracker", "criteria_headings"];
const MACHINE_KEYS: &[&str] = &["max_parallel", "notifications", "state_dir"];

#[derive(Deserialize)]
pub struct RepoConfig {
    pub tracker: TrackerConfig,
    #[serde(default = "default_criteria_headings")]
    pub criteria_headings: Vec<String>,
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
pub fn load(top: &Path) -> Result<RepoConfig> {
    let repo_path = top.join(REPO_CONFIG);
    let machine_path = machine_config_path()?;
    let Some(repo) = read(&repo_path)? else {
        bail!(
            "no repo config at {}; run `orchestrate setup` first",
            repo_path.display()
        );
    };
    validate(&repo_path, &repo, REPO_KEYS, MACHINE_KEYS, &machine_path)?;
    if let Some(machine) = read(&machine_path)? {
        validate(&machine_path, &machine, MACHINE_KEYS, REPO_KEYS, &repo_path)?;
    }
    repo.try_into()
        .with_context(|| format!("{}", repo_path.display()))
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
