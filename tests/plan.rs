mod common;

use common::Scratch;

#[test]
fn plan_prints_the_frontier_of_a_local_markdown_spec() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    s.ticket("auth", "01-schema", "Add schema", "done", &[]);
    s.ticket(
        "auth",
        "02-api",
        "Build API",
        "ready-for-agent",
        &["01-schema"],
    );
    s.ticket("auth", "03-ui", "Build UI", "ready-for-agent", &["02-api"]);
    s.ticket("auth", "04-docs", "Write docs", "needs-triage", &[]);
    s.ticket("auth", "05-cli", "Add CLI", "ready-for-agent", &[]);

    let out = s.run(&["plan", "auth"]).ok();

    assert_eq!(
        out,
        "Spec auth: Login rework\n\nFrontier:\n  02-api  Build API\n  05-cli  Add CLI\n"
    );
}

#[test]
fn ready_is_decided_by_the_configured_status_value() {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\nready = \"go\"\n",
    );
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Marked go", "go", &[]);
    s.ticket("auth", "b", "Default marker", "ready-for-agent", &[]);

    let out = s.run(&["plan", "auth"]).ok();

    assert!(out.contains("  a  Marked go\n"), "{out}");
    assert!(!out.contains("Default marker"), "{out}");
}

#[test]
fn ids_are_opaque_strings() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("PROJ-12", "Odd ids");
    s.ticket("PROJ-12", "007", "Leading zeros", "ready-for-agent", &["7"]);
    s.ticket("PROJ-12", "7", "Seven", "done", &[]);
    s.ticket("PROJ-12", "v1.2", "Dotted", "ready-for-agent", &["007"]);

    let out = s.run(&["plan", "PROJ-12"]).ok();

    assert_eq!(
        out,
        "Spec PROJ-12: Odd ids\n\nFrontier:\n  007  Leading zeros\n"
    );
}

#[test]
fn plan_never_starts_a_session_or_calls_gh() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    s.ticket("auth", "t1", "Only", "ready-for-agent", &[]);
    s.fake("claude", "exit 0");
    s.fake("gh", "exit 0");

    s.run(&["plan", "auth"]).ok();

    assert_eq!(s.calls("claude"), Vec::<String>::new());
    assert_eq!(s.calls("gh"), Vec::<String>::new());
}
