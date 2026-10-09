//! A tracker for worker threads: each call is sent to the dispatcher thread, which runs
//! it on the run's (pausing) tracker and sends the answer back. So a paused tracker
//! blocks only the dispatcher and the worker waiting on it; other sessions carry on.

use std::sync::mpsc::{self, Sender};

use anyhow::{Result, anyhow};

use super::{Blocker, Spec, Ticket, Tracker};

/// A tracker call for the dispatcher thread to run.
pub type Call = Box<dyn FnOnce(&dyn Tracker) + Send>;

/// What a worker thread sends the dispatcher: a tracker call, or the end of its work.
pub enum Msg<T> {
    Call(Call),
    Done(T),
}

pub struct Remote<T>(pub Sender<Msg<T>>);

impl<T: Send> Remote<T> {
    fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn Tracker) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let (reply, answer) = mpsc::channel();
        let call: Call = Box::new(move |t| {
            let _ = reply.send(f(t));
        });
        let stopped = || anyhow!("the run stopped before the tracker answered");
        self.0.send(Msg::Call(call)).map_err(|_| stopped())?;
        answer.recv().map_err(|_| stopped())?
    }
}

impl<T: Send> Tracker for Remote<T> {
    fn get_spec(&self, spec: &str) -> Result<Spec> {
        let spec = spec.to_string();
        self.call(move |t| t.get_spec(&spec))
    }

    fn list_tickets(&self, spec: &str) -> Result<Vec<Ticket>> {
        let spec = spec.to_string();
        self.call(move |t| t.list_tickets(&spec))
    }

    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>> {
        let ticket = ticket.to_string();
        self.call(move |t| t.list_blockers(&ticket))
    }

    fn mark_finished(&self, ticket: &str) -> Result<()> {
        let ticket = ticket.to_string();
        self.call(move |t| t.mark_finished(&ticket))
    }

    fn comment(&self, ticket: &str, markdown: &str) -> Result<()> {
        let (ticket, markdown) = (ticket.to_string(), markdown.to_string());
        self.call(move |t| t.comment(&ticket, &markdown))
    }

    fn tick(&self, ticket: &str, criteria: &[String]) -> Result<Vec<String>> {
        let (ticket, criteria) = (ticket.to_string(), criteria.to_vec());
        self.call(move |t| t.tick(&ticket, &criteria))
    }
}
