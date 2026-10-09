//! Child processes that run in their own process group and are killed as a group when
//! they outlive their time: sessions, gates, `prepare` and tracker calls.

use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// How long a killed group's leader gets to be reaped before it is left behind.
const REAP: Duration = Duration::from_secs(5);

/// Start `cmd` as the leader of a new process group; its id is the group's id.
pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.process_group(0).spawn()
}

/// Write `stdin` to `child` (when its stdin is piped), then wait for it to exit,
/// collecting whatever output is piped. `None` when it outlived `timeout` and its
/// whole process group was killed.
pub fn wait(mut child: Child, stdin: &[u8], timeout: Duration) -> Result<Option<Output>> {
    let pgid = child.id();
    let (pipe, input) = (child.stdin.take(), stdin.to_vec());
    let (tx, rx) = mpsc::channel();
    // Waits on its own thread, so a child that never reads its stdin or never closes
    // its output still times out.
    thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            // A child may exit without reading its stdin; its exit status tells.
            let _ = pipe.write_all(&input);
        }
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(out) => Ok(Some(out.context("waiting for the process")?)),
        Err(RecvTimeoutError::Timeout) => {
            kill_group(pgid);
            let _ = rx.recv_timeout(REAP);
            Ok(None)
        }
        Err(RecvTimeoutError::Disconnected) => bail!("lost the process"),
    }
}

/// Run `cmd` with `stdin` and its stdout and stderr piped back; an error when it
/// outlived `timeout`, after its process group was killed.
pub fn output(cmd: &mut Command, stdin: &[u8], timeout: Duration) -> Result<Output> {
    let what = format!("{:?}", cmd.get_program());
    let child = spawn(cmd.stdout(Stdio::piped()).stderr(Stdio::piped()))
        .with_context(|| format!("launching {what}"))?;
    match wait(child, stdin, timeout)? {
        Some(out) => Ok(out),
        None => bail!("timed out after {timeout:?}"),
    }
}

/// Run `sh -c <cmd>` in `dir` with its output to `log`. Its exit status, or `None`
/// when it outlived `timeout` and its process group was killed.
pub fn sh(cmd: &str, dir: &Path, log: &File, timeout: Duration) -> Result<Option<ExitStatus>> {
    let child = spawn(
        Command::new("sh")
            .args(["-c", cmd])
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?),
    )?;
    Ok(wait(child, &[], timeout)?.map(|out| out.status))
}

/// SIGKILL every process in group `pgid`.
pub fn kill_group(pgid: u32) {
    // A group already gone is fine.
    unsafe { libc::kill(-(pgid as libc::pid_t), libc::SIGKILL) };
}
