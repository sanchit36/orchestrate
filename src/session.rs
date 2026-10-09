//! Session runner: the only Claude-specific code. In: worktree, prompt, log path.
//! Out: exit code.

use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use anyhow::{Context, Result};

/// Run headless `claude -p` in `worktree`, in its own process group, with the prompt
/// on stdin and its raw output in `log`. The exit code, or -1 when killed by a signal.
pub fn run(worktree: &Path, prompt: &str, permission_mode: &str, log: &Path) -> Result<i32> {
    let out = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    let mut child = {
        // Spawn under the registry lock, so `kill_all` never misses a session.
        let mut live = LIVE.lock().unwrap();
        let child = Command::new("claude")
            .args(["-p", "--permission-mode", permission_mode])
            .current_dir(worktree)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(out.try_clone()?)
            .stderr(out)
            .spawn()
            .context("launching claude")?;
        live.push(child.id() as i32);
        child
    };
    let mut stdin = child.stdin.take().unwrap();
    // A session may exit without reading its prompt; that is its outcome, not ours.
    let _ = stdin.write_all(prompt.as_bytes());
    drop(stdin);
    let status = child.wait().context("waiting for claude")?;
    LIVE.lock().unwrap().retain(|&p| p != child.id() as i32);
    Ok(status.code().unwrap_or(-1))
}

/// Process groups of the running sessions.
static LIVE: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Kill every running session's process group, and start no more in this process:
/// the caller is about to exit.
pub fn kill_all() {
    let live = LIVE.lock().unwrap();
    for &pgid in live.iter() {
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    std::mem::forget(live);
}
