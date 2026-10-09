//! Template approvals: `~/.config/orchestrate/approvals.toml`, keyed by the templates'
//! content hash only, so an edited template is a new, unapproved one.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config;

/// `~/.config/orchestrate/approvals.toml`: `[[approved]]` entries with a `hash`.
#[derive(Deserialize, Default)]
struct Approvals {
    #[serde(default)]
    approved: Vec<Approval>,
}

#[derive(Deserialize)]
struct Approval {
    hash: String,
}

pub fn path() -> Result<PathBuf> {
    Ok(config::machine_dir()?.join("approvals.toml"))
}

/// Whether `hash` (a [`super::commands::approval_hash`]) is in the approvals file.
pub fn approved(hash: &str) -> Result<bool> {
    Ok(approvals(&path()?)?.approved.iter().any(|a| a.hash == hash))
}

/// Appends `hash` to the approvals file as one `[[approved]]` entry.
pub fn approve(hash: &str) -> Result<()> {
    let path = path()?;
    fs::create_dir_all(config::machine_dir()?)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(file, "[[approved]]\nhash = \"{hash}\"")
        .with_context(|| format!("writing {}", path.display()))
}

fn approvals(path: &Path) -> Result<Approvals> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("{}", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Approvals::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}
