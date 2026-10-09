//! Run control: `stop`, `run --detach`, and how a live run answers a stop.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use crate::reporting::Events;
use crate::state::{self, RunDirs, Store};
use crate::{config, lock, session};

const POLL: Duration = Duration::from_millis(50);

/// On SIGTERM (from `stop`), SIGINT (Ctrl-C) or SIGHUP (terminal closed): kill the
/// sessions, mark the run stopped, let go of the lock and exit.
pub fn stop_on_signal(dirs: &RunDirs, store: Arc<Store>) -> Result<()> {
    let mut signals = Signals::new([SIGTERM, SIGINT, SIGHUP])?;
    let (id, lock) = (dirs.id.clone(), dirs.lock.path.clone());
    let log = dirs.logs.join("run.log");
    thread::spawn(move || {
        if signals.forever().next().is_none() {
            return;
        }
        // Saves first, so the run loop cannot record the killed session as failed.
        let marked = store.stop();
        session::kill_all();
        if let Ok(mut events) = Events::open(&log) {
            match marked {
                Ok(()) => events.event(&format!("run {id} stopped")),
                Err(e) => events.event(&format!("run {id} stopped; state not saved: {e:#}")),
            }
        }
        let _ = std::fs::remove_file(&lock);
        std::process::exit(130);
    });
    Ok(())
}

/// Signal the live run (when given, the one `--run` resolves to) and wait for it to stop.
pub fn stop(top: &Path, run: Option<&str>) -> Result<()> {
    let (_, machine) = config::load(top)?;
    let repo_dir = state::repo_dir(&machine, top)?;
    let Some(live) = lock::live(&repo_dir)? else {
        bail!("no live run in this repo");
    };
    if let Some(run) = run {
        let named = state::resolve(state::runs(&repo_dir)?, Some(run))
            .map_err(|e| anyhow::anyhow!("{e:#}; the live run is {}", live.run))?;
        if named.run_id != live.run {
            bail!(
                "run {} is not live; the live run is {}",
                named.run_id,
                live.run
            );
        }
    }
    if unsafe { libc::kill(live.pid, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error()).context("signalling the run");
    }
    while lock::live(&repo_dir)?.is_some_and(|h| h.run == live.run) {
        thread::sleep(POLL);
    }
    println!("run {} stopped", live.run);
    Ok(())
}

/// Re-launch this command (`run` or `resume`, as `args`) in its own session, apart
/// from the terminal, and return once it holds the repo lock, printing its run id and
/// `run.log` path.
pub fn detach(top: &Path, args: &[String]) -> Result<()> {
    let (_, machine) = config::load(top)?;
    let repo_dir = state::repo_dir(&machine, top)?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    let mut child = cmd.spawn().context("re-launching orchestrate")?;
    let pid = child.id() as i32;
    loop {
        if let Some(h) = lock::read(&repo_dir)?.filter(|h| h.pid == pid) {
            let log = repo_dir.join("runs").join(&h.run).join("logs/run.log");
            println!("run {} detached\nlog: {}", h.run, log.display());
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err)?;
            bail!("the detached run exited ({status}) before starting:\n{err}");
        }
        thread::sleep(POLL);
    }
}
