//! Session runner: the only Claude-specific code. In: worktree, prompt, timeout, log
//! path. Out: exit code.

use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Process groups of the sessions running now, killed if the run is stopped.
static LIVE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Kill every running session's process group, and start no more in this process:
/// the caller is about to exit.
pub fn kill_all() {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    for pgid in live.iter() {
        kill_group(*pgid);
    }
    std::mem::forget(live);
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
    let mut child = Command::new("claude")
        .args(["-p", "--permission-mode", permission_mode])
        .current_dir(worktree)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()
        .context("launching claude")?;
    let pgid = child.id();
    // Registered under the lock it was spawned under, so an interrupt cannot miss it.
    live.push(pgid);
    drop(live);
    let mut stdin = child.stdin.take().unwrap();
    // A session may exit without reading its prompt; that is its outcome, not ours.
    let _ = stdin.write_all(prompt.as_bytes());
    drop(stdin);
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for claude")? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            kill_group(pgid);
            child.wait().context("waiting for claude")?;
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|p| *p != pgid);
    Ok(status.map(|s| s.code().unwrap_or(-1)))
}

fn kill_group(pgid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pgid}")])
        .status();
}
