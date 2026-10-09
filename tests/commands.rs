//! The command-template adapter: secret refusal, approval and tracker-guide drift.
mod common;

use std::fs;
use std::process::Command;

use common::Scratch;
use serde_json::json;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/command_tracker.py"
);
const GUIDE: &str = "docs/agents/issue-tracker.md";

/// Fixture tracker state outside the repo: spec `auth` with one ready ticket `A-1`.
fn tracker(s: &Scratch) {
    let ticket = json!({
        "id": "A-1", "title": "Add schema", "finished": false, "ready": true, "blockers": [],
        "body": "## Acceptance criteria\n\n- [ ] it works\n",
    });
    let spec = json!({"id": "auth", "title": "Login rework", "body": "The spec."});
    let state = json!({"spec": spec, "tickets": [ticket]});
    fs::write(s.home.join("tracker.json"), state.to_string()).unwrap();
}

/// The `[tracker.commands]` lines for the fixture tracker; `extra` is appended.
fn templates(s: &Scratch, extra: &str) -> String {
    let state = s.home.join("tracker.json");
    let op = |op: &str| format!("python3 '{SCRIPT}' '{}' {op} {{id}}", state.display());
    format!(
        "version = 1\n\n[[gates]]\nname = \"check\"\nrun = \"true\"\n\n\
         [tracker]\nkind = \"commands\"\nlocation = \"fixture\"\n\n[tracker.commands]\n\
         get_spec = \"{}\"\nlist_tickets = \"{}\"\nlist_blockers = \"{}\"\nmark_finished = \"{}\"\n{extra}",
        op("get-spec"),
        op("list-tickets"),
        op("list-blockers"),
        op("mark-finished"),
    )
}

/// The approval hash a refusal names, approved in the scratch home's approvals file.
fn approve(s: &Scratch, refusal: &str) {
    let at = refusal.find("sha256:").expect("refusal names the hash");
    let hex: String = refusal[at + 7..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    let hash = format!("sha256:{hex}");
    let path = s.home.join(".config/orchestrate/approvals.toml");
    let old = fs::read_to_string(&path).unwrap_or_default();
    fs::write(path, format!("{old}[[approved]]\nhash = \"{hash}\"\n")).unwrap();
}

#[test]
fn plan_refuses_templates_until_approved_and_again_after_a_change() {
    let s = Scratch::new();
    tracker(&s);
    s.repo_config(&templates(&s, ""));

    let refusal = s.run(&["plan", "auth"]).fails();
    assert!(refusal.contains("not approved"), "{refusal}");
    approve(&s, &refusal);
    let out = s.run(&["plan", "auth"]).ok();
    assert_eq!(
        out,
        "Spec auth: Login rework\n\nFrontier:\n  A-1  Add schema\n"
    );

    // Approval is keyed by content: an edited template is a new, unapproved one.
    s.repo_config(&templates(&s, "comment = \"cat >/dev/null; echo {}\"\n"));
    let refusal = s.run(&["plan", "auth"]).fails();
    assert!(refusal.contains("not approved"), "{refusal}");
}

#[test]
fn run_refuses_unapproved_templates_before_starting() {
    let s = Scratch::new();
    tracker(&s);
    s.repo_config(&templates(&s, ""));

    let refusal = s.run(&["run", "auth", "--branch", "int"]).fails();

    assert!(refusal.contains("not approved"), "{refusal}");
    assert!(!s.state_root().exists(), "no run state was created");
}

#[test]
fn templates_go_with_the_commands_kind_only() {
    let s = Scratch::new();
    s.repo_config("version = 1\n\n[tracker]\nkind = \"commands\"\nlocation = \"x\"\n");
    let err = s.run(&["plan", "auth"]).fails();
    assert!(err.contains("[tracker.commands]"), "{err}");

    let local = templates(&s, "").replace("\"commands\"", "\"local-markdown\"");
    s.repo_config(&local);
    let err = s.run(&["plan", "auth"]).fails();
    assert!(err.contains("[tracker.commands]"), "{err}");
}

#[test]
fn a_template_with_a_literal_secret_is_refused_without_echoing_it() {
    let s = Scratch::new();
    tracker(&s);
    let token = "ghp_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dE3fG5";
    let extra = format!("comment = \"curl -H 'Authorization: token {token}' x\"\n");
    s.repo_config(&templates(&s, &extra));

    let err = s.run(&["plan", "auth"]).fails();

    assert!(
        err.contains("comment") && err.contains("literal secret"),
        "{err}"
    );
    assert!(!err.contains(token), "{err}");
}

#[test]
fn templates_may_reference_environment_variables() {
    let s = Scratch::new();
    tracker(&s);
    let extra = "comment = \"curl -H \\\"Authorization: Bearer $TRACKER_TOKEN\\\" x\"\n";
    s.repo_config(&templates(&s, extra));

    let refusal = s.run(&["plan", "auth"]).fails();

    assert!(refusal.contains("not approved"), "{refusal}");
}

#[test]
fn resume_refuses_unapproved_templates_too() {
    let s = Scratch::new();
    tracker(&s);
    s.repo_config(&templates(&s, ""));

    let refusal = s.run(&["resume"]).fails();

    assert!(refusal.contains("not approved"), "{refusal}");
}

fn sha256(text: &str) -> String {
    let path = std::env::temp_dir().join(format!("guide-{}", std::process::id()));
    fs::write(&path, text).unwrap();
    let out = Command::new("shasum")
        .args(["-a", "256"])
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    let out = String::from_utf8(out.stdout).unwrap();
    format!("sha256:{}", out.split_whitespace().next().unwrap())
}

#[test]
fn a_changed_tracker_guide_warns_without_blocking() {
    let s = Scratch::new();
    tracker(&s);
    s.write(GUIDE, "Use the jira CLI.\n");
    let hash = sha256("Use the jira CLI.\n");
    let config = templates(
        &s,
        &format!("guide = \"{GUIDE}\"\nguide_hash = \"{hash}\"\n"),
    );
    s.repo_config(&config);
    approve(&s, &s.run(&["plan", "auth"]).fails());

    let unchanged = s.run(&["plan", "auth"]);
    assert_eq!(unchanged.code(), 0);
    assert_eq!(unchanged.stderr(), "");

    s.write(GUIDE, "Use the jira REST API.\n");
    let changed = s.run(&["plan", "auth"]);
    assert!(changed.ok().contains("A-1  Add schema"));
    let warning = changed.stderr();
    assert!(
        warning.contains(GUIDE) && warning.contains("changed since"),
        "{warning}"
    );
}

#[test]
fn a_run_without_comment_and_tick_templates_skips_and_notes_them() {
    let s = Scratch::new();
    tracker(&s);
    s.repo_config(&templates(&s, ""));
    approve(&s, &s.run(&["plan", "auth"]).fails());
    s.claude_lands();
    s.commit_all();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(out.contains("ticket A-1 finished"), "{out}");
    assert!(
        out.contains("ticket A-1: tick skipped: not supported by this tracker"),
        "{out}"
    );
    assert!(
        out.contains("ticket A-1: comment skipped: not supported by this tracker"),
        "{out}"
    );
    let state = fs::read_to_string(s.home.join("tracker.json")).unwrap();
    assert!(state.contains(r#""finished": true"#), "{state}");
}
