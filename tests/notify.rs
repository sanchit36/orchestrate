//! Desktop notifications where none can be shown: the harness runs orchestrate with no
//! D-Bus session bus, like a headless Linux machine. macOS always has a desktop, so
//! there these tests would pop real notifications; they run on Linux only.
#![cfg(target_os = "linux")]

mod common;

use std::fs;

use common::Scratch;

/// Spec `auth`: `b` blocked by `a`, a passing gate, and the given machine config.
fn chain(machine: &str) -> Scratch {
    let s = Scratch::new();
    s.machine_config(machine);
    s.local_markdown_tracker_with_gate("true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.ticket("auth", "b", "Build API", "ready-for-agent", &["a"]);
    s.commit_all();
    s.claude_lands();
    s
}

fn run_log(s: &Scratch) -> String {
    let runs = s.repo_state(&s.state_root()).join("runs");
    fs::read_to_string(runs.join(s.run_id()).join("logs/run.log")).unwrap()
}

fn skip_lines(log: &str) -> usize {
    log.lines()
        .filter(|l| l.contains("notifications skipped"))
        .count()
}

#[test]
fn without_a_desktop_notifications_are_skipped_with_one_line_and_the_run_carries_on() {
    // `notifications` defaults to true.
    let s = chain("version = 1\n");

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(skip_lines(&run_log(&s)), 1, "{out}");
    assert!(
        out.contains("ended: 2 finished, 0 failed, 0 stuck"),
        "{out}"
    );
    s.git(&["cat-file", "-e", "int:b.txt"]);
}

#[test]
fn notifications_false_turns_them_all_off() {
    let s = chain("version = 1\nnotifications = false\n");

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(skip_lines(&run_log(&s)), 0, "{out}");
}
