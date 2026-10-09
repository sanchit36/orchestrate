//! What setup proposes from the repo: tracker kind and location, ready marker, gates
//! and seed files. Reads only; never writes the tracker guide or triage labels.

use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::git;

const GUIDE: &str = "docs/agents/issue-tracker.md";
const TRIAGE_LABELS: &str = "docs/agents/triage-labels.md";

/// `github` or `local-markdown`: from the tracker guide's title, else the `origin` remote.
pub fn kind(top: &Path) -> &'static str {
    let title = read(top, GUIDE)
        .and_then(|g| {
            g.lines()
                .find_map(|l| l.strip_prefix("# ").map(str::to_lowercase))
        })
        .unwrap_or_default();
    if title.contains("github") {
        "github"
    } else if title.contains("markdown") || title.contains("local") {
        "local-markdown"
    } else if origin_repo(top).is_some() {
        "github"
    } else {
        "local-markdown"
    }
}

/// GitHub: `owner/repo` of a GitHub `origin`. Local markdown: the first backticked
/// directory (`dir/`) the guide names, else `specs`.
pub fn location(top: &Path, kind: &str) -> Option<String> {
    if kind == "github" {
        return origin_repo(top);
    }
    let guide = read(top, GUIDE).unwrap_or_default();
    let dir = guide
        .split('`')
        .skip(1)
        .step_by(2)
        .find(|s| s.len() > 1 && s.ends_with('/') && !s.contains([' ', '<', '{']));
    Some(dir.map_or("specs", |d| d.trim_end_matches('/')).to_string())
}

fn origin_repo(top: &Path) -> Option<String> {
    let url = git::git(top, &["remote", "get-url", "origin"]).ok()?;
    let rest = url.split_once("github.com")?.1;
    let rest = rest.trim_start_matches([':', '/']).trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    (rest.matches('/').count() == 1).then(|| rest.to_string())
}

/// What `ready-for-agent` maps to in the triage labels table, if the file has it.
pub fn ready(top: &Path) -> Option<String> {
    let labels = read(top, TRIAGE_LABELS)?;
    labels.lines().find_map(|l| {
        let mut cells = l.split('|').skip(1).map(|c| c.trim().trim_matches('`'));
        (cells.next()? == "ready-for-agent")
            .then(|| cells.next().map(String::from))
            .flatten()
            .filter(|c| !c.is_empty())
    })
}

/// Proposed gates as `(name, command)`, from the detection set where its files exist.
pub fn gates(top: &Path) -> Vec<(String, String)> {
    let mut gates = Vec::new();
    let mut add = |name: &str, run: &str| gates.push((name.to_string(), run.to_string()));
    if let Some(package) = read(top, "package.json") {
        let scripts = serde_json::from_str::<serde_json::Value>(&package)
            .ok()
            .and_then(|p| p.get("scripts").cloned())
            .unwrap_or_default();
        for script in ["typecheck", "lint", "test"] {
            if scripts.get(script).is_some() {
                add(&format!("npm-{script}"), &format!("npm run {script}"));
            }
        }
    }
    if let Some(makefile) = read(top, "Makefile") {
        for target in ["test", "lint"] {
            if makefile
                .lines()
                .any(|l| l.starts_with(&format!("{target}:")))
            {
                add(&format!("make-{target}"), &format!("make {target}"));
            }
        }
    }
    if top.join("Cargo.toml").exists() {
        add("cargo-test", "cargo test");
    }
    if top.join("go.mod").exists() {
        add("go-test", "go test ./...");
    }
    let pyproject = read(top, "pyproject.toml").unwrap_or_default();
    if top.join("pytest.ini").exists()
        || top.join("conftest.py").exists()
        || pyproject.contains("[tool.pytest")
    {
        add("pytest", "pytest");
    }
    gates
}

/// Ignored local files worth copying into worktrees: top-level `.env*` and
/// `.claude/settings.local.json`.
pub fn seed_files(top: &Path) -> Result<Vec<String>> {
    let mut candidates: Vec<String> = fs::read_dir(top)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(".env"))
        .collect();
    candidates.sort();
    candidates.push(".claude/settings.local.json".into());
    let mut seeds = Vec::new();
    for file in candidates {
        if top.join(&file).is_file() && git::succeeds(top, &["check-ignore", "-q", &file])? {
            seeds.push(file);
        }
    }
    Ok(seeds)
}

fn read(top: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(top.join(rel)).ok()
}
