mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::Scratch;

const COMMIT: &str =
    r#"echo "$slug" > "$slug.txt"; git add "$slug.txt"; git commit -q -m "work $slug""#;
const DONE: &str = r#"printf '{"status":"done","acceptance_criteria":[{"text":"it works","met":true,"evidence":"ok"}],"notes":""}' > .orchestrate-result.json"#;

/// A fake `claude` running `arms` (sh `case` arms on the worktree name); every other
/// ticket lands.
fn claude(s: &Scratch, arms: &str) {
    s.fake(
        "claude",
        &format!(
            "slug=$(basename \"$PWD\")\ncat > /dev/null\ncase \"$slug\" in\n{arms}\n*) {COMMIT}; {DONE};;\nesac"
        ),
    );
}

/// Spec `auth` with ticket `a` (two criteria) and the given gate command.
fn spec(s: &Scratch, gate: &str) {
    s.repo_config(&format!(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\
         retry_delay = \"10ms\"\n\n[[gates]]\nname = \"check\"\nrun = \"{gate}\"\n"
    ));
    s.spec("auth", "Login rework");
    s.write(
        "specs/auth/a.md",
        "# Add schema\n\nStatus: ready-for-agent\n\nBody.\n\n\
         ## Acceptance criteria\n\n- [ ] it works\n- [ ] it is fast\n",
    );
    s.commit_all();
}

fn ticket_file(s: &Scratch) -> String {
    s.git(&["show", "int:specs/auth/a.md"])
}

fn state(s: &Scratch) -> serde_json::Value {
    let path = s
        .repo_state(&s.state_root())
        .join("runs")
        .join(s.run_id())
        .join("state.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// The `## Orchestrate log` section and everything after it.
fn log(file: &str) -> &str {
    let at = file.find("## Orchestrate log").expect("no log section");
    &file[at..]
}

#[test]
fn landing_ticks_then_comments_then_marks_finished() {
    let s = Scratch::new();
    spec(&s, "true");
    s.claude_lands();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    let subjects = s.git(&["log", "--first-parent", "--format=%s", "-4", "int"]);
    assert_eq!(
        subjects.lines().collect::<Vec<_>>(),
        [
            "Mark a finished",
            "Comment on a",
            "Tick criteria of a",
            "Merge ticket a: Add schema"
        ]
    );
    let file = ticket_file(&s);
    assert!(file.contains("Status: done"), "{file}");
    assert!(
        file.contains("- [x] it works\n- [x] it is fast\n"),
        "{file}"
    );
    let sha = s.git(&["rev-parse", "int~3"]);
    let entry = log(&file);
    for part in ["`int`", &sha.trim()[..12], "check: passed", &s.run_id()] {
        assert!(entry.contains(part), "no {part} in {entry}");
    }
    assert!(file.trim_end().ends_with(entry.trim_end()), "log is last");
}

#[test]
fn criteria_edited_mid_session_are_left_alone_and_listed_as_mismatches() {
    let s = Scratch::new();
    spec(&s, "true");
    // A human rewords one criterion on the integration branch while the session runs.
    let edit = "f=../_integration/specs/auth/a.md; sed 's/it is fast/it is very fast/' $f > $f.new; \
                mv $f.new $f; git -C ../_integration commit -q -am edit";
    claude(&s, &format!("a) {edit}; {COMMIT}; {DONE};;"));

    s.run(&["run", "auth", "--branch", "int"]).ok();

    let file = ticket_file(&s);
    assert!(
        file.contains("- [x] it works\n- [ ] it is very fast\n"),
        "{file}"
    );
    assert!(file.contains("Status: done"), "{file}");
    assert!(log(&file).contains("it is fast"), "{file}");
    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "finished");
    assert_eq!(
        st["tickets"]["a"]["mismatches"],
        serde_json::json!(["it is fast"])
    );
}

#[test]
fn a_failed_ticket_gets_only_a_comment_without_gate_output() {
    let s = Scratch::new();
    spec(&s, "echo SECRET-OUTPUT; exit 3");
    s.claude_lands();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    let file = ticket_file(&s);
    let entry = log(&file);
    for part in ["failed", &s.run_id(), "gate check exited 3"] {
        assert!(entry.contains(part), "no {part} in {entry}");
    }
    assert!(!file.contains("SECRET-OUTPUT"), "{file}");
    assert!(file.contains("Status: ready-for-agent"), "{file}");
    assert!(
        file.contains("- [ ] it works\n- [ ] it is fast\n"),
        "{file}"
    );
}

#[test]
fn a_stuck_ticket_gets_a_comment_with_its_notes_and_no_local_paths() {
    let s = Scratch::new();
    spec(&s, "true");
    let stuck = r#"printf '{"status":"stuck","acceptance_criteria":[],"notes":"need a db in %s"}' "$PWD" > .orchestrate-result.json"#;
    claude(&s, &format!("a) {COMMIT}; {stuck};;"));

    s.run(&["run", "auth", "--branch", "int"]).ok();

    let file = ticket_file(&s);
    let entry = log(&file);
    for part in ["stuck", &s.run_id(), "need a db in"] {
        assert!(entry.contains(part), "no {part} in {entry}");
    }
    let root = s.home.parent().unwrap().to_string_lossy().into_owned();
    assert!(!file.contains(&root), "{file}");
    assert!(file.contains("Status: ready-for-agent"), "{file}");
}

#[test]
fn a_failing_tick_or_comment_is_recorded_and_never_fails_the_ticket_or_pauses() {
    let s = Scratch::new();
    spec(&s, "true");
    s.claude_lands();
    // Fail the first six commits in the integration worktree: three tick and three
    // comment attempts. Mark-finished then commits.
    let hook = s.repo.join(".git/hooks/pre-commit");
    let count = s.bin.join("hook.count");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\ncase \"$PWD\" in */_integration) ;; *) exit 0;; esac\n\
             n=$(($(cat '{c}' 2>/dev/null || echo 0) + 1))\necho $n > '{c}'\n\
             [ $n -gt 6 ] || {{ echo tracker down >&2; exit 1; }}\n",
            c = count.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(!out.contains("paused"), "{out}");
    let file = ticket_file(&s);
    assert!(file.contains("Status: done"), "{file}");
    assert!(
        file.contains("- [ ] it works"),
        "tick was rolled back: {file}"
    );
    assert!(!file.contains("## Orchestrate log"), "{file}");
    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "finished");
    let notes = st["tickets"]["a"]["writeback"].as_array().unwrap();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].as_str().unwrap().starts_with("tick failed: "));
    assert!(notes[1].as_str().unwrap().starts_with("comment failed: "));
    assert!(out.contains("ticket a: tick failed: "), "{out}");
    assert!(out.contains("ticket a: comment failed: "), "{out}");
}
