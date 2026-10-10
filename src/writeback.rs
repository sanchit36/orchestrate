//! Writing back to the tracker: on landing, tick the met criteria, comment, then mark
//! finished; on failed or stuck, only an outcome comment. Ticks and comments are
//! cosmetic: a failure or a missing operation is recorded, never fatal. Nothing posted
//! carries gate output or local paths.

use std::path::Path;

use anyhow::Result;

use crate::state::Outcome;
use crate::tracker::{Tracker, Unsupported};

/// The run a write-back belongs to, and the local paths to keep out of what is posted.
pub struct Context<'a> {
    pub run_id: &'a str,
    pub branch: &'a str,
    /// Replaced by `<label>` in posted text, longest first.
    pub private: Vec<(&'a Path, &'static str)>,
}

/// What a write-back left to record in run state.
#[derive(Default)]
pub struct Written {
    /// Criteria edited since dispatch, left unticked.
    pub mismatches: Vec<String>,
    /// Optional writes that failed or were skipped, such as `comment failed: <err>`.
    pub problems: Vec<String>,
}

/// Tick `criteria` (all met, since the ticket landed), comment how it landed (`gates`
/// as `<name>: <result>`), then mark it finished. Only mark-finished can fail, and the
/// tracker decides how (it pauses).
pub fn landed(
    tracker: &dyn Tracker,
    cx: &Context,
    ticket: &str,
    criteria: &[String],
    merge: &str,
    gates: &[String],
) -> Result<Written> {
    let mut written = Written::default();
    if !criteria.is_empty() {
        match tracker.tick(ticket, criteria) {
            Ok(missing) => written.mismatches = missing,
            Err(e) => written.problems.push(problem("tick", &e)),
        }
    }
    let mut text = format!(
        "Landed on `{}` in merge commit `{}` by orchestrate run `{}`.\n\nGates: {}.\n",
        cx.branch,
        &merge[..12],
        cx.run_id,
        gates.join(", ")
    );
    if !written.mismatches.is_empty() {
        text.push_str("\nCriteria edited since dispatch, left unticked:\n\n");
        for m in &written.mismatches {
            text.push_str(&format!("- {m}\n"));
        }
    }
    comment(tracker, cx, ticket, &text, &mut written);
    tracker.mark_finished(ticket)?;
    Ok(written)
}

/// Comment a failed, timed-out or stuck outcome and its reason; nothing else changes.
pub fn outcome(
    tracker: &dyn Tracker,
    cx: &Context,
    ticket: &str,
    outcome: Outcome,
    reason: Option<&str>,
) -> Written {
    let mut text = format!("Orchestrate run `{}`: {outcome}", cx.run_id);
    match reason {
        Some(r) if r.contains('\n') => text.push_str(&format!(".\n\n{r}\n")),
        Some(r) => text.push_str(&format!(": {r}\n")),
        None => text.push_str(".\n"),
    }
    let mut written = Written::default();
    comment(tracker, cx, ticket, &text, &mut written);
    written
}

fn comment(tracker: &dyn Tracker, cx: &Context, ticket: &str, text: &str, w: &mut Written) {
    if let Err(e) = tracker.comment(ticket, &scrub(text, &cx.private)) {
        w.problems.push(problem("comment", &e));
    }
}

fn problem(op: &str, err: &anyhow::Error) -> String {
    if err.is::<Unsupported>() {
        format!("{op} skipped: {err}")
    } else {
        format!("{op} failed: {}", one_line(err))
    }
}

fn one_line(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` with every private path replaced by its label.
fn scrub(text: &str, private: &[(&Path, &str)]) -> String {
    let mut paths: Vec<_> = private
        .iter()
        .map(|(p, label)| (p.to_string_lossy(), *label))
        .filter(|(p, _)| !p.is_empty() && p != "/")
        .collect();
    paths.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
    paths.iter().fold(text.to_string(), |t, (p, label)| {
        t.replace(p.as_ref(), label)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::{Blocker, Spec, Ticket};
    use std::cell::RefCell;

    /// An adapter with only the required operations.
    struct Minimal(RefCell<Vec<String>>);

    impl Tracker for Minimal {
        fn get_spec(&self, _: &str) -> Result<Spec> {
            unreachable!()
        }
        fn list_tickets(&self, _: &str) -> Result<Vec<Ticket>> {
            unreachable!()
        }
        fn list_blockers(&self, _: &str) -> Result<Vec<Blocker>> {
            unreachable!()
        }
        fn mark_finished(&self, t: &str) -> Result<()> {
            self.0.borrow_mut().push(format!("finished {t}"));
            Ok(())
        }
    }

    fn cx() -> Context<'static> {
        Context {
            run_id: "r1",
            branch: "int",
            private: vec![(Path::new("/home/me"), "~")],
        }
    }

    #[test]
    fn missing_optional_operations_are_skipped_and_noted() {
        let t = Minimal(RefCell::default());
        let texts = ["it works".to_string()];
        let w = landed(
            &t,
            &cx(),
            "a",
            &texts,
            "0123456789abcdef",
            &["check: passed".into()],
        )
        .unwrap();
        assert_eq!(t.0.borrow().as_slice(), ["finished a"]);
        assert_eq!(
            w.problems,
            [
                "tick skipped: not supported by this tracker",
                "comment skipped: not supported by this tracker"
            ]
        );
        let w = outcome(&t, &cx(), "a", Outcome::Failed, Some("gate check exited 1"));
        assert_eq!(
            w.problems,
            ["comment skipped: not supported by this tracker"]
        );
    }

    #[test]
    fn private_paths_are_scrubbed_longest_first() {
        let private = [
            (Path::new("/home/me"), "~"),
            (Path::new("/home/me/repo"), "<repo>"),
        ];
        let text = "see /home/me/repo/x and /home/me/y";
        assert_eq!(scrub(text, &private), "see <repo>/x and ~/y");
    }
}
