mod common;

use common::Scratch;

/// A ticket file with an arbitrary body after the title and `Status:` line.
fn raw_ticket(s: &Scratch, spec: &str, id: &str, status: &str, rest: &str) {
    s.write(
        &format!("specs/{spec}/{id}.md"),
        &format!("# Title {id}\n\nStatus: {status}\n\n{rest}"),
    );
}

const CRITERIA: &str = "## Acceptance criteria\n\n- [ ] it works\n";

#[test]
fn both_body_blocked_by_forms_are_read_as_ids_or_links_and_combined() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    s.ticket("auth", "01", "Done one", "done", &[]);
    s.ticket("auth", "02", "Open one", "needs-triage", &[]);
    raw_ticket(
        &s,
        "auth",
        "03",
        "ready-for-agent",
        &format!("Blocked by: 01\n\n## Blocked by\n\n- [Open one](02.md)\n\n{CRITERIA}"),
    );
    raw_ticket(
        &s,
        "auth",
        "04",
        "ready-for-agent",
        &format!(
            "Blocked by: [Done](./01.md)\n\n## Blocked by\n\nSome prose.\n\n- 01\n\n{CRITERIA}"
        ),
    );
    raw_ticket(
        &s,
        "auth",
        "05",
        "ready-for-agent",
        &format!("## Blocked by\n\n- 02\n\n{CRITERIA}"),
    );

    let out = s.run(&["plan", "auth"]).ok();

    assert!(out.contains("Frontier:\n  04  Title 04\n"), "{out}");
    assert!(!out.contains("  03  Title 03\n"), "{out}");
    assert!(!out.contains("  05  Title 05\n"), "{out}");
}

#[test]
fn criteria_are_only_checkboxes_directly_under_a_matching_heading() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    let ready = "ready-for-agent";
    raw_ticket(
        &s,
        "auth",
        "01",
        ready,
        "Just prose.\n\n- [ ] a stray checkbox\n",
    );
    raw_ticket(
        &s,
        "auth",
        "02",
        ready,
        "### Acceptance\n\n- plain bullet\n1. numbered\n\n### Notes\n\n- [ ] after the section\n",
    );
    raw_ticket(
        &s,
        "auth",
        "03",
        ready,
        "#### acceptance criteria\n\n##### Details\n\n- [x] under a deeper heading\n",
    );
    raw_ticket(&s, "auth", "04", ready, CRITERIA);

    let out = s.run(&["plan", "auth"]).ok();

    assert!(
        out.ends_with(
            "\nWarnings:\n  01: no acceptance criteria\n  02: acceptance criteria heading has no checkboxes\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("  01  Title 01\n"),
        "warnings never hold back: {out}"
    );
}

#[test]
fn criteria_heading_names_are_configurable() {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\ncriteria_headings = [\"Done when\"]\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n",
    );
    s.spec("auth", "Login rework");
    raw_ticket(
        &s,
        "auth",
        "01",
        "ready-for-agent",
        "## Done when\n\n- [ ] x\n",
    );
    raw_ticket(&s, "auth", "02", "ready-for-agent", CRITERIA);

    let out = s.run(&["plan", "auth"]).ok();

    assert!(
        out.ends_with("\nWarnings:\n  02: no acceptance criteria\n"),
        "{out}"
    );
}

#[test]
fn plan_refuses_a_missing_spec_a_spec_without_tickets_or_without_ready_tickets() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("empty", "No tickets");
    s.spec("idle", "Nothing ready");
    s.ticket("idle", "01", "Done", "done", &[]);
    s.ticket("idle", "02", "Triage", "needs-triage", &[]);

    let err = s.run(&["plan", "nope"]).fails();
    assert!(err.contains("spec `nope` not found"), "{err}");
    let err = s.run(&["plan", "empty"]).fails();
    assert!(err.contains("spec `empty` has no tickets"), "{err}");
    let err = s.run(&["plan", "idle"]).fails();
    assert!(err.contains("spec `idle` has no ready tickets"), "{err}");
}

#[test]
fn plan_refuses_a_blocking_cycle_and_names_its_tickets() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    s.ticket("auth", "01", "Start", "ready-for-agent", &[]);
    s.ticket("auth", "02", "A", "ready-for-agent", &["01", "04"]);
    s.ticket("auth", "03", "B", "needs-triage", &["02"]);
    s.ticket("auth", "04", "C", "ready-for-agent", &["03"]);

    let run = s.run(&["plan", "auth"]);

    let err = run.fails();
    assert!(
        err.contains("blocking cycle: 02 -> 04 -> 03 -> 02"),
        "{err}"
    );
    assert_eq!(run.stdout(), "");
}

#[test]
fn an_empty_body_or_unknown_blocker_holds_the_ticket_and_its_dependents() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    raw_ticket(&s, "auth", "01", "ready-for-agent", "\n  \n");
    s.ticket("auth", "02", "Two", "ready-for-agent", &["01"]);
    s.ticket("auth", "03", "Three", "ready-for-agent", &["ghost"]);
    s.ticket("auth", "04", "Four", "needs-triage", &["02"]);
    s.ticket("auth", "05", "Five", "ready-for-agent", &[]);

    let out = s.run(&["plan", "auth"]).ok();

    assert_eq!(
        out,
        "Spec auth: Login rework\n\nFrontier:\n  05  Five\n\n\
         Needs attention:\n  \
         01  Title 01  (empty body)\n  \
         02  Two  (blocked by held-back 01)\n  \
         03  Three  (blocker ghost not found)\n  \
         04  Four  (blocked by held-back 02)\n\n\
         Warnings:\n  01: no acceptance criteria\n"
    );
}

#[test]
fn a_blocker_outside_the_spec_gates_by_its_finished_state_and_is_never_worked() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    s.spec("other", "Elsewhere");
    s.ticket("other", "x1", "Open elsewhere", "ready-for-agent", &[]);
    s.ticket("other", "x2", "Done elsewhere", "done", &[]);
    s.ticket("auth", "01", "Gated", "ready-for-agent", &["x1"]);
    s.ticket("auth", "02", "Free", "ready-for-agent", &["x2"]);

    let out = s.run(&["plan", "auth"]).ok();

    assert_eq!(out, "Spec auth: Login rework\n\nFrontier:\n  02  Free\n");
}

#[test]
fn a_ticket_blocked_by_a_ticket_that_is_not_ready_waits_and_plan_says_why() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    s.ticket("auth", "01", "Untriaged", "needs-triage", &[]);
    s.ticket("auth", "02", "Waits", "ready-for-agent", &["01"]);
    s.ticket("auth", "03", "Free", "ready-for-agent", &[]);

    let out = s.run(&["plan", "auth"]).ok();

    assert_eq!(
        out,
        "Spec auth: Login rework\n\nFrontier:\n  03  Free\n\n\
         Waiting:\n  02  Waits  (blocked by 01, which is not ready)\n"
    );
}
