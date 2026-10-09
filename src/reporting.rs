//! Reporting: one timestamped line per event, printed and appended to `run.log`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

pub struct Events {
    log: File,
}

impl Events {
    pub fn open(path: &Path) -> Result<Self> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(Self { log })
    }

    /// Print `msg` as one line and append the same line to `run.log`.
    pub fn event(&mut self, msg: &str) {
        let line = format!("{} {msg}", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S"));
        println!("{line}");
        // A failed log write must not stop the run; the line is already on screen.
        // One append per line, so lines from several threads' handles never interleave.
        let _ = self.log.write_all(format!("{line}\n").as_bytes());
    }
}
