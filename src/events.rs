//! Reporting: one timestamped line per event, printed and appended to `run.log`, and
//! desktop notifications for the events a user wants to be told about.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};

/// Set once a notification could not be shown; the rest of the run skips them.
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Notifications being shown on their own threads.
static SHOWING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

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
        let Some(title) = self.title.clone() else {
            return;
        };
        if UNAVAILABLE.load(Ordering::Relaxed) {
            return;
        }
        // notify-rust posts as Finder by default, which macOS drops without an error, so
        // post as the app the run was launched from. Only the first call takes effect.
        #[cfg(target_os = "macos")]
        let _ = notify_rust::set_application(
            &std::env::var("__CFBundleIdentifier").unwrap_or_else(|_| "com.apple.Terminal".into()),
        );
        let (body, log) = (body.to_string(), self.log.try_clone());
        // Showing one can take seconds (on macOS), so the run never waits on it.
        let showing = thread::spawn(move || {
            let shown = notify_rust::Notification::new()
                .summary(&title)
                .body(&body)
                .show();
            if let (Err(e), Ok(log)) = (shown, log)
                && !UNAVAILABLE.swap(true, Ordering::Relaxed)
            {
                Events { log, title: None }.event(&format!("notifications skipped: {e}"));
            }
        });
        SHOWING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(showing);
    }
}

/// Wait for the notifications still being shown, so the process can exit without
/// losing the last ones.
pub fn shown() {
    let showing = std::mem::take(&mut *SHOWING.lock().unwrap_or_else(|e| e.into_inner()));
    for notification in showing {
        let _ = notification.join();
    }
}
