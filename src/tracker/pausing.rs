//! A tracker that fails closed and heals itself (ADR 0001): a failed operation is
//! retried briefly, then the run pauses and probes it with backoff until it answers.
//! The caller blocks meanwhile, so nothing new starts while the run is paused.

use std::cell::RefCell;
use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;

use super::{Blocker, Spec, Ticket, Tracker};
use crate::config::TrackerConfig;
use crate::reporting::Events;

const ATTEMPTS: u32 = 3;

pub struct Pausing<'a> {
    inner: Box<dyn Tracker>,
    config: &'a TrackerConfig,
    events: RefCell<Events>,
}

impl<'a> Pausing<'a> {
    pub fn new(inner: Box<dyn Tracker>, config: &'a TrackerConfig, events: Events) -> Self {
        Self {
            inner,
            config,
            events: RefCell::new(events),
        }
    }

    /// `f`'s answer, however long the tracker takes to give one.
    fn call<T>(&self, op: &str, f: impl Fn(&dyn Tracker) -> Result<T>) -> Result<T> {
        let tracker = self.inner.as_ref();
        let mut result = f(tracker);
        for _ in 1..ATTEMPTS {
            if result.is_ok() {
                break;
            }
            sleep(self.config.retry_delay);
            result = f(tracker);
        }
        let err = match result {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let mut events = self.events.borrow_mut();
        let mut wait = self.config.probe_backoff;
        let paused = format!("run paused: {op} failed: {}", one_line(&err));
        events.event(&format!("{paused}; next probe in {}", human(wait)));
        events.notify(&paused);
        loop {
            sleep(wait);
            match f(tracker) {
                Ok(v) => {
                    let recovered = format!("run recovered: {op} succeeded");
                    events.event(&recovered);
                    events.notify(&recovered);
                    return Ok(v);
                }
                Err(err) => {
                    wait = (wait * 2).min(self.config.probe_backoff_max);
                    events.event(&format!(
                        "tracker probe failed: {}; next probe in {}",
                        one_line(&err),
                        human(wait)
                    ));
                }
            }
        }
    }
}

impl Tracker for Pausing<'_> {
    fn get_spec(&self, spec: &str) -> Result<Spec> {
        self.call(&format!("get spec {spec}"), |t| t.get_spec(spec))
    }

    fn list_tickets(&self, spec: &str) -> Result<Vec<Ticket>> {
        self.call(&format!("list tickets of {spec}"), |t| t.list_tickets(spec))
    }

    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>> {
        self.call(&format!("list blockers of {ticket}"), |t| {
            t.list_blockers(ticket)
        })
    }

    fn mark_finished(&self, ticket: &str) -> Result<()> {
        self.call(&format!("mark {ticket} finished"), |t| {
            t.mark_finished(ticket)
        })
    }
}

/// An error chain as one event line.
fn one_line(err: &anyhow::Error) -> String {
    format!("{err:#}")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A duration in jiff's friendly form, such as `1m` or `100ms`.
fn human(d: Duration) -> String {
    format!(
        "{:#}",
        jiff::SignedDuration::try_from(d).unwrap_or_default()
    )
}
