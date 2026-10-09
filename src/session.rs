//! Session runner: the only Claude-specific code. In: worktree, prompt, timeout, log
//! path. Out: exit code.

use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::process;

/// Process groups of the sessions running now, killed if the run stops; `None` once
/// it has, so no session starts after.
static LIVE: Mutex<Option<Vec<u32>>> = Mutex::new(Some(Vec::new()));

/// Kill every running session's process group, and start no more in this process.
pub fn kill_all() {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner()).take();
    for pgid in live.into_iter().flatten() {
        process::kill_group(pgid);
    }
}

/// Run headless `claude -p` in `worktree`, in its own process group, with the prompt
/// on stdin and its raw output in `log`. The exit code (-1 when killed by a signal),
/// or `None` when it ran past `timeout` and its whole process group was killed.
pub fn run(
    worktree: &Path,
    prompt: &str,
    permission_mode: &str,
    timeout: Duration,
    log: &Path,
) -> Result<Option<i32>> {
    let out = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pgids) = live.as_mut() else {
        bail!("the run is stopping");
    };
    let child = process::spawn(
        Command::new("claude")
            .args(["-p", "--permission-mode", permission_mode])
            .current_dir(worktree)
            .stdin(Stdio::piped())
            .stdout(out.try_clone()?)
            .stderr(out),
    )
    .context("launching claude")?;
    let pgid = child.id();
    // Registered under the lock it was spawned under, so a stop cannot miss it.
    pgids.push(pgid);
    drop(live);
    let ended = process::wait(child, prompt.as_bytes(), timeout).context("waiting for claude");
    if let Some(pgids) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        pgids.retain(|p| *p != pgid);
    }
    Ok(ended?.map(|out| out.status.code().unwrap_or(-1)))
}
