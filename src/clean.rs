//! `clean`: remove what runs left behind once the human chooses. Never touches an
//! integration branch, and never a live run.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::git::git;
use crate::state::{self, State};
use crate::{config, lock, worktree};

pub struct Flags {
    pub force: bool,
    pub all: bool,
    pub purge: bool,
    pub orphans: bool,
}

/// `clean [--run ID] [--force] [--all] [--purge] [--orphans]`.
pub fn clean(top: &Path, run: Option<&str>, flags: &Flags) -> Result<()> {
    let (_, machine) = config::load(top)?;
    if flags.orphans {
        return orphans(&state::state_root(&machine)?);
    }
    let repo_dir = state::repo_dir(&machine, top)?;
    let runs = state::runs(&repo_dir)?;
    let live = lock::live(&repo_dir)?.map(|h| h.run);
    let targets = match flags.all {
        true => runs.into_iter().filter(|s| s.status == "ended").collect(),
        false => vec![state::resolve(runs, run)?],
    };
    for s in &targets {
        if live.as_ref() == Some(&s.run_id) {
            bail!(
                "run {} is live; stop it with `orchestrate stop` first",
                s.run_id
            );
        }
        one(top, &repo_dir, s, flags.force || flags.purge)?;
        if flags.purge {
            remove_dir(&repo_dir.join("runs").join(&s.run_id))?;
        }
    }
    Ok(())
}

/// Remove run `s`'s worktrees and `orchestrate/<run-id>/*` branches, keeping its failed
/// and stuck tickets' current attempts unless `force`.
fn one(top: &Path, repo_dir: &Path, s: &State, force: bool) -> Result<()> {
    let dir = repo_dir.join("worktrees").join(&s.run_id);
    // Worktree name -> (ticket, outcome) of each ticket's latest attempt kept as evidence.
    let kept: HashMap<String, (&str, &str)> = (s.tickets.iter())
        .filter(|(_, t)| !force && ["failed", "timeout", "stuck"].contains(&t.outcome.as_str()))
        .filter_map(|(id, t)| {
            let name = t.worktree.as_ref()?.file_name()?.to_string_lossy();
            Some((name.into_owned(), (id.as_str(), t.outcome.as_str())))
        })
        .collect();
    let prefix = format!("orchestrate/{}/", s.run_id);
    let branches = git(
        top,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/heads/{prefix}"),
        ],
    )?;
    let mut removed = (0, 0);
    for name in entries(&dir)? {
        if !kept.contains_key(&name) {
            let path = dir.join(&name);
            // A dir git no longer knows as a worktree still goes.
            if worktree::remove_worktree(top, &path).is_err() {
                remove_dir(&path)?;
            }
            removed.0 += 1;
        }
    }
    // Forget worktrees whose dirs are gone, so their branches can be deleted.
    git(top, &["worktree", "prune"])?;
    for branch in branches.lines() {
        let name = &branch[prefix.len()..];
        // The integration branch is never ours to delete, whatever its name.
        if !kept.contains_key(name) && branch != s.branch {
            git(top, &["branch", "-D", branch])?;
            removed.1 += 1;
        }
    }
    if kept.is_empty() {
        let _ = fs::remove_dir(&dir);
    }
    println!(
        "run {}: removed {} worktrees and {} branches",
        s.run_id, removed.0, removed.1
    );
    let mut kept: Vec<_> = kept.into_iter().collect();
    kept.sort();
    for (name, (id, outcome)) in &kept {
        println!("  kept {id} ({outcome}): {}", dir.join(name).display());
    }
    if !kept.is_empty() {
        println!("pass --force to remove them too");
    }
    Ok(())
}

/// Remove every repo state dir under `root` whose recorded repo is gone, or is no longer
/// the git repo the dir was made for.
fn orphans(root: &Path) -> Result<()> {
    for name in entries(root)? {
        let dir = root.join(&name);
        let Ok(text) = fs::read_to_string(dir.join("repo.toml")) else {
            continue;
        };
        let path: toml::Table = toml::from_str(&text)
            .with_context(|| format!("reading {}/repo.toml", dir.display()))?;
        let Some(path) = path.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        let same = Path::new(path).is_dir()
            && state::repo_dir_name(Path::new(path)).is_ok_and(|n| n == name);
        if !same && lock::live(&dir)?.is_none() {
            remove_dir(&dir)?;
            println!("removed {}", dir.display());
        }
    }
    Ok(())
}

/// The names in `dir`; none if it is missing.
fn entries(dir: &Path) -> Result<Vec<String>> {
    let read = match fs::read_dir(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        r => r.with_context(|| format!("reading {}", dir.display()))?,
    };
    read.map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
        .collect()
}

fn remove_dir(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", dir.display()))
        }
        _ => Ok(()),
    }
}
