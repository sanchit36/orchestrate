//! The adapter contract suite (spec Seam 2): one set of checks every adapter must pass
//! against the same fixture spec. An adapter plugs in with a [`Harness`] that loads
//! [`TICKETS`] into its backend, then `crate::tracker::contract::suite!(MyHarness);`
//! in a test module generates one test per check.

use super::{Tracker, Unsupported};
use crate::markdown;

pub const SPEC_TITLE: &str = "Login";
pub const HEADINGS: &[&str] = &["Acceptance criteria"];
/// A blocker reference that names no ticket on the tracker.
pub const UNKNOWN: &str = "99-ghost";

#[derive(Clone, Copy, PartialEq)]
pub enum Status {
    Open,
    Ready,
    Finished,
}

pub struct FixtureTicket {
    /// A name, not an id: the harness maps it to its tracker's id with [`Harness::id`].
    pub name: &'static str,
    pub title: &'static str,
    pub status: Status,
    /// Names of the tickets blocking this one (may include [`UNKNOWN`]).
    pub blocked_by: &'static [&'static str],
    pub criteria: &'static [&'static str],
}

/// The fixture spec's tickets, in tracker order.
pub const TICKETS: &[FixtureTicket] = &[
    FixtureTicket {
        name: "01-schema",
        title: "Add the schema",
        status: Status::Finished,
        blocked_by: &[],
        criteria: &["the users table exists"],
    },
    FixtureTicket {
        name: "02-form",
        title: "Build the login form",
        status: Status::Ready,
        blocked_by: &["01-schema"],
        criteria: &["the form renders", "bad passwords show an error"],
    },
    FixtureTicket {
        name: "03-session",
        title: "Keep the session",
        status: Status::Ready,
        blocked_by: &["02-form", UNKNOWN],
        criteria: &["a reload keeps you logged in"],
    },
    FixtureTicket {
        name: "04-logout",
        title: "Log out",
        status: Status::Open,
        blocked_by: &[],
        criteria: &["the session ends"],
    },
];

/// The ticket body in the shared Markdown forms: a `Blocked by:` line naming
/// `blocker_ids` (pass none when the harness uses native links) and the criteria.
pub fn body(t: &FixtureTicket, blocker_ids: &[String]) -> String {
    let mut text = String::new();
    if !blocker_ids.is_empty() {
        text.push_str(&format!("Blocked by: {}\n\n", blocker_ids.join(", ")));
    }
    text.push_str("## Acceptance criteria\n\n");
    for c in t.criteria {
        text.push_str(&format!("- [ ] {c}\n"));
    }
    text
}

/// One adapter under test. `new` loads the fixture spec into a fresh backend.
pub trait Harness {
    fn new() -> Self;
    fn tracker(&self) -> &dyn Tracker;
    fn spec_id(&self) -> String;
    /// The tracker id of the fixture ticket `name` (also for [`UNKNOWN`]).
    fn id(&self, name: &str) -> String;
    /// Edits a criterion's text on the backend, behind the adapter's back.
    fn edit_criterion(&self, name: &str, from: &str, to: &str);
    /// Whether the backend holds a comment on `name` containing `text`.
    fn has_comment(&self, name: &str, text: &str) -> bool;
    /// Every later backend call errors (command exits non-zero, files gone, ...).
    fn fail(&self);
    /// Every later backend call answers with schema-invalid output.
    fn garble(&self);
}

fn reread<H: Harness>(h: &H, name: &str) -> super::Ticket {
    let id = h.id(name);
    let tickets = h.tracker().list_tickets(&h.spec_id()).unwrap();
    tickets.into_iter().find(|t| t.id == id).unwrap()
}

/// `(text, met)` of the ticket's criteria as the tracker now shows them.
fn criteria<H: Harness>(h: &H, name: &str) -> Vec<(String, bool)> {
    let body = reread(h, name).body;
    let found = markdown::criteria(&body, HEADINGS).expect("criteria heading kept");
    found.into_iter().map(|c| (c.text, c.met)).collect()
}

fn fixture(name: &str) -> &'static FixtureTicket {
    TICKETS.iter().find(|t| t.name == name).unwrap()
}

pub fn spec_and_tickets_match_the_fixture<H: Harness>(h: H) {
    let t = h.tracker();
    let spec = t.get_spec(&h.spec_id()).unwrap();
    assert_eq!((spec.id, spec.title.as_str()), (h.spec_id(), SPEC_TITLE));
    let tickets = t.list_tickets(&h.spec_id()).unwrap();
    let got: Vec<_> = tickets
        .iter()
        .map(|t| (t.id.clone(), t.title.as_str()))
        .collect();
    let want: Vec<_> = TICKETS.iter().map(|f| (h.id(f.name), f.title)).collect();
    assert_eq!(got, want, "ids and titles in tracker order");
    for f in TICKETS {
        let want: Vec<_> = f.criteria.iter().map(|c| (c.to_string(), false)).collect();
        assert_eq!(criteria(&h, f.name), want, "criteria of {}", f.name);
    }
}

pub fn ids_are_opaque_strings_that_round_trip<H: Harness>(h: H) {
    let tickets = h.tracker().list_tickets(&h.spec_id()).unwrap();
    let mut ids: Vec<_> = tickets.iter().map(|t| t.id.clone()).collect();
    assert!(ids.iter().all(|id| !id.is_empty()));
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), TICKETS.len(), "ids are unique");
    // Every listed id is accepted back by the other operations unchanged.
    for t in &tickets {
        h.tracker().list_blockers(&t.id).unwrap();
    }
}

pub fn ready_and_finished_follow_the_tracker<H: Harness>(h: H) {
    for f in TICKETS {
        let t = reread(&h, f.name);
        assert_eq!(t.ready, f.status == Status::Ready, "ready of {}", f.name);
        assert_eq!(
            t.finished,
            f.status == Status::Finished,
            "finished of {}",
            f.name
        );
    }
}

pub fn blockers_carry_ids_and_finished_state<H: Harness>(h: H) {
    for f in TICKETS {
        let blockers = h.tracker().list_blockers(&h.id(f.name)).unwrap();
        let got: Vec<_> = blockers.into_iter().map(|b| (b.id, b.finished)).collect();
        let want: Vec<_> = f
            .blocked_by
            .iter()
            .map(|&n| {
                let finished = (n != UNKNOWN).then(|| fixture(n).status == Status::Finished);
                (h.id(n), finished)
            })
            .collect();
        assert_eq!(got, want, "blockers of {}", f.name);
    }
}

pub fn mark_finished_is_seen_on_reread<H: Harness>(h: H) {
    h.tracker().mark_finished(&h.id("02-form")).unwrap();
    assert!(reread(&h, "02-form").finished);
    let blockers = h.tracker().list_blockers(&h.id("03-session")).unwrap();
    assert_eq!(blockers[0].finished, Some(true));
    assert!(
        !reread(&h, "04-logout").finished,
        "only that ticket changes"
    );
}

/// `None` when the adapter does not implement the optional operation.
fn optional<T>(r: anyhow::Result<T>) -> Option<T> {
    match r {
        Err(e) if e.is::<Unsupported>() => None,
        r => Some(r.unwrap()),
    }
}

pub fn comment_is_posted<H: Harness>(h: H) {
    let posted = h.tracker().comment(&h.id("02-form"), "Landed as `abc123`.");
    if optional(posted).is_none() {
        return;
    }
    assert!(h.has_comment("02-form", "Landed as `abc123`."));
    let want: Vec<_> = fixture("02-form")
        .criteria
        .iter()
        .map(|c| (c.to_string(), false))
        .collect();
    assert_eq!(
        criteria(&h, "02-form"),
        want,
        "a comment leaves the criteria alone"
    );
}

pub fn tick_rereads_and_matches_by_text<H: Harness>(h: H) {
    let t = h.tracker();
    let id = h.id("02-form");
    let texts = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let Some(missing) = optional(t.tick(&id, &texts(&["the form renders", "not a criterion"])))
    else {
        return;
    };
    assert_eq!(missing, ["not a criterion"]);
    let rendered = ("the form renders".to_string(), true);
    let error = ("bad passwords show an error".to_string(), false);
    assert_eq!(criteria(&h, "02-form"), [rendered.clone(), error]);

    // Already ticked still counts as found.
    assert!(
        t.tick(&id, &texts(&["the form renders"]))
            .unwrap()
            .is_empty()
    );

    // Edited since it was read: the old text is a mismatch and nothing is ticked.
    h.edit_criterion(
        "02-form",
        "bad passwords show an error",
        "bad passwords are rejected",
    );
    let missing = t
        .tick(&id, &texts(&["bad passwords show an error"]))
        .unwrap();
    assert_eq!(missing, ["bad passwords show an error"]);
    let edited = ("bad passwords are rejected".to_string(), false);
    assert_eq!(criteria(&h, "02-form"), [rendered, edited]);
}

pub fn unknown_spec_is_an_error<H: Harness>(h: H) {
    assert!(h.tracker().get_spec("no-such-spec").is_err());
}

/// Every operation returns `Err`: never a panic, never `Ok` with made-up data.
fn every_operation_fails<H: Harness>(h: &H) {
    let (t, spec, id) = (h.tracker(), h.spec_id(), h.id("02-form"));
    assert!(t.get_spec(&spec).is_err(), "get_spec");
    assert!(t.list_tickets(&spec).is_err(), "list_tickets");
    assert!(t.list_blockers(&id).is_err(), "list_blockers");
    assert!(t.mark_finished(&id).is_err(), "mark_finished");
    assert!(t.comment(&id, "x").is_err(), "comment");
    assert!(t.tick(&id, &["the form renders".into()]).is_err(), "tick");
}

pub fn backend_errors_are_adapter_failures<H: Harness>(h: H) {
    h.fail();
    every_operation_fails(&h);
}

pub fn schema_invalid_output_is_an_adapter_failure<H: Harness>(h: H) {
    h.garble();
    every_operation_fails(&h);
}

/// One `#[test]` per contract check for the harness type `$h`.
macro_rules! suite {
    ($h:ty) => {
        $crate::tracker::contract::suite!(@tests $h;
            spec_and_tickets_match_the_fixture,
            ids_are_opaque_strings_that_round_trip,
            ready_and_finished_follow_the_tracker,
            blockers_carry_ids_and_finished_state,
            mark_finished_is_seen_on_reread,
            comment_is_posted,
            tick_rereads_and_matches_by_text,
            unknown_spec_is_an_error,
            backend_errors_are_adapter_failures,
            schema_invalid_output_is_an_adapter_failure,
        );
    };
    (@tests $h:ty; $($check:ident),* $(,)?) => {
        $(
            #[test]
            fn $check() {
                use $crate::tracker::contract::Harness;
                $crate::tracker::contract::$check(<$h>::new());
            }
        )*
    };
}
pub(crate) use suite;
