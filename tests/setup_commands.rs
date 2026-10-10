//! Setup for a tracker without a built-in adapter: a headless agent (the fake `claude`)
//! compiles the command templates, the user approves them, and setup verifies them.
mod common;

use std::fs;

use common::Scratch;
use serde_json::json;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/command_tracker.py"
);
const GUIDE: &str = "# Issue tracker: Jira\n\nIssues live in Jira project ACME. Use `jira`.\n";

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

/// A fake `claude` that saves its prompt to `bin/compile.prompt` and answers with
/// templates for the fixture tracker (plus `extra` JSON fields), wrapped in prose.
fn compiler(s: &Scratch, extra: &str) {
    let state = s.home.join("tracker.json");
    let op = |op: &str| format!("python3 '{SCRIPT}' '{}' {op} {{id}}", state.display());
    let templates = json!({
        "get_spec": op("get-spec"),
        "list_tickets": op("list-tickets"),
        "list_blockers": op("list-blockers"),
        "mark_finished": op("mark-finished"),
        "comment": op("comment"),
    });
    let mut text = templates.to_string();
    text.pop();
    let text = format!("{text}{extra}}}");
    let script = format!(
        "cat > '{bin}/compile.prompt'\necho 'Here are the templates:'\necho '```json'\n\
         cat <<'EOF'\n{text}\nEOF\necho '```'",
        bin = s.bin.display()
    );
    s.fake("claude", &script);
}

fn repo_config(s: &Scratch) -> toml::Table {
    let text = fs::read_to_string(s.repo.join(".orchestrate/config.toml")).unwrap();
    text.parse().unwrap()
}

fn approvals(s: &Scratch) -> String {
    fs::read_to_string(s.home.join(".config/orchestrate/approvals.toml")).unwrap_or_default()
}

/// Kind, location, ready, headings, approval, gates, seeds, mode, spec.
const ANSWERS: &str = "\nACME\n\n\nyes\ncheck: true\n\n\n\nauth\n";

#[test]
fn compiles_templates_from_the_guide_shows_them_records_approval_and_verifies() {
    let s = Scratch::new();
    s.write("docs/agents/issue-tracker.md", GUIDE);
    tracker(&s);
    compiler(&s, "");

    let out = s.run_with_input(&["setup"], ANSWERS).ok();

    assert!(out.contains("[commands]"), "{out}");
    let prompt = fs::read_to_string(s.bin.join("compile.prompt")).unwrap();
    assert!(prompt.contains("Jira project ACME"), "{prompt}");
    assert!(prompt.contains("list_blockers"), "{prompt}");
    for name in [
        "get_spec",
        "list_tickets",
        "list_blockers",
        "mark_finished",
        "comment",
    ] {
        assert!(
            out.contains(&format!("  {name}: python3 ")),
            "{name}: {out}"
        );
    }
    assert!(out.contains("Frontier:\n  A-1  Add schema\n"), "{out}");
    assert!(out.contains("mark-finished 'A-1'"), "{out}");
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(s.home.join("tracker.json")).unwrap()).unwrap();
    assert_eq!(state["tickets"][0]["finished"], false, "never marked");

    let config = repo_config(&s);
    let tracker = &config["tracker"];
    assert_eq!(tracker["kind"].as_str(), Some("commands"));
    let commands = tracker["commands"].as_table().unwrap();
    assert!(
        commands["mark_finished"]
            .as_str()
            .unwrap()
            .contains("mark-finished")
    );
    assert!(commands.contains_key("comment") && !commands.contains_key("tick"));
    assert_eq!(
        commands["guide"].as_str(),
        Some("docs/agents/issue-tracker.md")
    );
    // sha256 of GUIDE, computed independently with `shasum -a 256`.
    assert_eq!(
        commands["guide_hash"].as_str(),
        Some("sha256:bf913a166b4cf820f87b2fc28e5bc454637d23404022d6f0599e1c9a4a498b12")
    );
    assert_eq!(approvals(&s).matches("[[approved]]").count(), 1);
    let plan = s.run(&["plan", "auth"]).ok();
    assert!(
        plan.contains("A-1  Add schema"),
        "approved, so plan works: {plan}"
    );
}

#[test]
fn without_a_guide_the_agent_compiles_from_the_users_one_paragraph_description() {
    let s = Scratch::new();
    tracker(&s);
    compiler(&s, "");

    let answers = "commands\nACME\n\n\nLinear, reached with the `linear` CLI.\nyes\ncheck: true\n\n\n\nauth\n";
    let out = s.run_with_input(&["setup"], answers).ok();

    assert!(
        out.contains("Describe your tracker in one paragraph"),
        "{out}"
    );
    let prompt = fs::read_to_string(s.bin.join("compile.prompt")).unwrap();
    assert!(
        prompt.contains("Linear, reached with the `linear` CLI."),
        "{prompt}"
    );
    let commands = repo_config(&s)["tracker"]["commands"].clone();
    assert!(commands.get("guide").is_none(), "{commands}");
    assert!(commands.get("guide_hash").is_none(), "{commands}");
}

#[test]
fn declining_the_templates_writes_nothing() {
    let s = Scratch::new();
    s.write("docs/agents/issue-tracker.md", GUIDE);
    tracker(&s);
    compiler(&s, "");

    let err = s.run_with_input(&["setup"], "\nACME\n\n\nno\n").fails();

    assert!(err.contains("not approved"), "{err}");
    assert!(!s.repo.join(".orchestrate").exists());
    assert_eq!(approvals(&s), "");
}

#[test]
fn a_compiled_template_with_a_literal_secret_is_refused_before_approval() {
    let s = Scratch::new();
    s.write("docs/agents/issue-tracker.md", GUIDE);
    tracker(&s);
    compiler(
        &s,
        r#","tick":"curl -H 'Authorization: Bearer ghp_abcdefghijklmnopqrstuvwxyz0123' x""#,
    );

    let run = s.run_with_input(&["setup"], ANSWERS);

    let err = run.fails();
    assert!(err.contains("`tick` template"), "{err}");
    assert!(err.contains("literal secret"), "{err}");
    assert!(!run.stdout().contains("Approve"), "{}", run.stdout());
    assert!(!s.repo.join(".orchestrate").exists());
}

#[test]
fn re_running_keeps_approved_templates_without_compiling_or_asking_again() {
    let s = Scratch::new();
    s.write("docs/agents/issue-tracker.md", GUIDE);
    tracker(&s);
    compiler(&s, "");
    s.run_with_input(&["setup"], ANSWERS).ok();
    let first = repo_config(&s)["tracker"]["commands"].clone();

    // Kind, location, ready, headings, keep the templates, keep the gate, no more gates,
    // seeds, mode, spec.
    let out = s
        .run_with_input(&["setup"], "\n\n\n\n\n\n\n\n\nauth\n")
        .ok();

    assert!(
        out.contains("Recompile the command templates (yes or no) [no]"),
        "{out}"
    );
    assert!(!out.contains("Approve"), "{out}");
    assert_eq!(s.calls("claude").len(), 1, "compiled once");
    assert_eq!(repo_config(&s)["tracker"]["commands"], first);
    assert_eq!(approvals(&s).matches("[[approved]]").count(), 1);
}
