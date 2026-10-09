//! Gates: the checks a rebased ticket must pass before it lands.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobMatcher};

use crate::config::Gate;
use crate::process::sh;
use crate::state::GateResult;

/// A path glob: `*` stays within one path segment, `**` crosses them.
pub fn glob(pattern: &str) -> Result<GlobMatcher> {
    let glob = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("bad glob `{pattern}`"))?;
    Ok(glob.compile_matcher())
}

/// Run `gates` in order in the ticket worktree `wt`, given the ticket's `changed`
/// files. Each gate's output goes to `<logs>/<slug>.gate-<name>.log`, and each gate
/// that runs gets one entry in `results`: its first failing exit code (-1 for a
/// timeout), else 0. The first failure's reason, or `None` when every gate passed or
/// was skipped and at least one ran: nothing lands unchecked.
pub fn run_all(
    gates: &[Gate],
    wt: &Path,
    changed: &[String],
    logs: &Path,
    slug: &str,
    results: &mut Vec<GateResult>,
) -> Result<Option<String>> {
    let mut ran = false;
    for gate in gates {
        let places = places(gate, wt, changed)?;
        if places.is_empty() {
            continue;
        }
        ran = true;
        let log = logs.join(format!("{slug}.gate-{}.log", gate.name));
        let out = File::create(&log).with_context(|| format!("creating {}", log.display()))?;
        let mut failure = None;
        for place in places {
            let dir = wt.join(place);
            let status = sh(&gate.run, &dir, &out, gate.timeout)
                .with_context(|| format!("running gate {}", gate.name))?;
            if let Some(teardown) = &gate.teardown {
                // Its outcome does not decide the gate; its output is in the log.
                sh(teardown, &dir, &out, gate.timeout)
                    .with_context(|| format!("running teardown of gate {}", gate.name))?;
            }
            failure = match status {
                Some(s) if s.success() => continue,
                Some(s) => {
                    let code = s.code().unwrap_or(-1);
                    Some((code, format!("gate {} exited {code}", gate.name)))
                }
                None => {
                    let after = gate.timeout;
                    Some((-1, format!("gate {} timed out after {after:?}", gate.name)))
                }
            };
            break;
        }
        results.push(GateResult {
            name: gate.name.clone(),
            code: failure.as_ref().map_or(0, |(code, _)| *code),
        });
        if let Some((_, reason)) = failure {
            return Ok(Some(reason));
        }
    }
    Ok((!ran).then(|| "no gate matched its change".to_string()))
}

/// The working directories, relative to the worktree, `gate` runs in; none when its
/// `paths` match no changed file. With `dirs`, each listed folder holding a matching
/// changed file; otherwise its `cwd` or the top level.
fn places(gate: &Gate, wt: &Path, changed: &[String]) -> Result<Vec<String>> {
    let globs = gate
        .paths
        .iter()
        .map(|p| glob(p))
        .collect::<Result<Vec<_>>>()?;
    let changed: Vec<&String> = changed
        .iter()
        .filter(|f| globs.is_empty() || globs.iter().any(|g| g.is_match(f)))
        .collect();
    if gate.dirs.is_empty() {
        // A gate without `paths` checks every change, even one that touches no file.
        let runs = globs.is_empty() || !changed.is_empty();
        return Ok(runs
            .then(|| gate.cwd.clone().unwrap_or_default())
            .into_iter()
            .collect());
    }
    let mut dirs = BTreeSet::new();
    for pattern in &gate.dirs {
        let pattern = pattern.trim_end_matches('/');
        // shortcut: a dirs entry matches folders at its own depth, so `**` is not supported.
        let depth = pattern.split('/').count();
        let dir = glob(pattern)?;
        for file in &changed {
            let parts: Vec<&str> = file.split('/').collect();
            if parts.len() > depth {
                let prefix = parts[..depth].join("/");
                // A folder the change deleted has nothing left to check.
                if dir.is_match(&prefix) && wt.join(&prefix).is_dir() {
                    dirs.insert(prefix);
                }
            }
        }
    }
    Ok(dirs.into_iter().collect())
}
