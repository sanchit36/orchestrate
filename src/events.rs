//! Reporting: one timestamped line per event, printed and appended to `run.log`, and
//! desktop notifications for the events a user wants to be told about.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

/// Set once a notification could not be shown; the rest of the run skips them.
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);

pub struct Events {
    log: File,
    /// The notification title, or `None` when notifications are off.
    title: Option<String>,
}

impl Events {
    pub fn open(path: &Path) -> Result<Self> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(Self { log, title: None })
    }

    /// Let `notify` show desktop notifications titled `orchestrate · <spec_slug>` when `on`.
    pub fn notifying(self, on: bool, spec_slug: &str) -> Self {
        let title = on.then(|| format!("orchestrate · {spec_slug}"));
        Self { title, ..self }
    }

    /// Print `msg` as one line and append the same line to `run.log`.
    pub fn event(&mut self, msg: &str) {
        let line = format!("{} {msg}", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S"));
        println!("{line}");
        // A failed log write must not stop the run; the line is already on screen.
        // One append per line, so lines from several threads' handles never interleave.
        let _ = self.log.write_all(format!("{line}\n").as_bytes());
    }

    /// Show `body` as a desktop notification. Where none can be shown, the first
    /// failure is one event line and later notifications are skipped silently.
    pub fn notify(&mut self, body: &str) {
        let Some(title) = &self.title else { return };
        if UNAVAILABLE.load(Ordering::Relaxed) {
            return;
        }
        let shown = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .show();
        if let Err(e) = shown
            && !UNAVAILABLE.swap(true, Ordering::Relaxed)
        {
            self.event(&format!("notifications skipped: {e}"));
        }
    }
}
