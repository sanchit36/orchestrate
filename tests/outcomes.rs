mod common;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::Scratch;

const COMMIT: &str =
    r#"echo "$slug" > "$slug.txt"; git add "$slug.txt"; git commit -q -m "work $slug""#;

fn report(status: &str, met: bool, notes: &str) -> String {
    format!(
        r#"printf '{{"status":"{status}","acceptance_criteria":[{{"text":"it works","met":true,"evidence":"ok"}},{{"text":"it is fast","met":{met},"evidence":"slow"}}],"notes":"{notes}"}}' > .orchestrate-result.json"#
    )
}

/// A fake `claude` running `arms` (sh `case` arms on the worktree name) and landing
/// every other ticket.
fn claude(s: &Scratch, arms: &str) {
    let land = format!("{COMMIT}; {}", report("done", true, ""));
    s.fake(
        "claude",
        &format!("slug=$(basename \"$PWD\")\ncat > /dev/null\ncase \"$slug\" in\n{arms}\n*) {land};;\nesac"),
    );
}

/// Spec `auth`: `b` blocked by `a`, unrelated `c`, a gate that records its calls.
fn spec(s: &Scratch, session: &str) {
    s.fake("gate", "");
    s.repo_config(&format!(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"gate\"\n\n[session]\n{session}"
    ));
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.ticket("auth", "b", "Build API", "ready-for-agent", &["a"]);
    s.ticket("auth", "c", "Write docs", "ready-for-agent", &[]);
    s.commit_all();
}

fn state(s: &Scratch) -> serde_json::Value {
    let path = s
        .repo_state(&s.state_root())
        .join("runs")
        .join(s.run_id())
        .join("state.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn ticket_wt(s: &Scratch, slug: &str) -> PathBuf {
    s.repo_state(&s.state_root())
        .join("worktrees")
        .join(s.run_id())
        .join(slug)
}

fn merged(s: &Scratch) -> String {
    s.git(&["log", "--first-parent", "--merges", "--format=%s", "int"])
}

#[test]
fn a_stuck_report_lands_nothing_keeps_the_worktree_holds_dependents_and_records_the_notes() {
    let s = Scratch::new();
    spec(&s, "");
    claude(
        &s,
        &format!("a) {COMMIT}; {};;", report("stuck", true, "need a db")),
    );

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(s.calls("gate").len(), 1, "only c is gated");
    assert!(!merged(&s).contains("Add schema"));
    assert!(ticket_wt(&s, "a").join("a.txt").is_file());
    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "stuck");
    assert_eq!(st["tickets"]["a"]["reason"], "need a db");
    assert_eq!(st["tickets"]["b"]["outcome"], "held");
    assert_eq!(st["tickets"]["b"]["reason"], "blocked by stuck a");
    assert_eq!(st["tickets"]["c"]["outcome"], "finished");
    assert!(out.contains("ticket a stuck: need a db\n"), "{out}");
    assert!(out.contains("ticket b held: blocked by stuck a\n"), "{out}");
    assert_eq!(s.calls("claude").len(), 2);
}

#[test]
fn a_done_report_with_an_unmet_criterion_is_stuck_with_the_unmet_criteria_recorded() {
    let s = Scratch::new();
    spec(&s, "");
    claude(&s, &format!("a) {COMMIT}; {};;", report("done", false, "")));

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "stuck");
    assert_eq!(
        st["tickets"]["a"]["unmet"],
        serde_json::json!(["it is fast"])
    );
    assert!(out.contains("ticket a stuck: unmet: it is fast\n"), "{out}");
    assert_eq!(s.calls("gate").len(), 1, "only c is gated");
}

#[test]
fn a_missing_or_malformed_report_no_new_commits_or_a_non_zero_exit_is_failed() {
    let done = report("done", true, "");
    for (arm, reason) in [
        (format!("a) {COMMIT};;"), "no report"),
        (
            format!("a) {COMMIT}; echo '{{' > .orchestrate-result.json;;"),
            "malformed report",
        ),
        (format!("a) {done};;"), "no new commits"),
        (format!("a) {COMMIT}; {done}; exit 3;;"), "session exited 3"),
    ] {
        let s = Scratch::new();
        spec(&s, "");
        claude(&s, &arm);

        let out = s.run(&["run", "auth", "--branch", "int"]).ok();

        let st = state(&s);
        assert_eq!(st["tickets"]["a"]["outcome"], "failed", "{arm}");
        assert!(
            st["tickets"]["a"]["reason"]
                .as_str()
                .unwrap()
                .starts_with(reason),
            "{arm}: {st}"
        );
        assert!(out.contains(&format!("ticket a failed: {reason}")), "{out}");
        assert_eq!(s.calls("gate").len(), 1, "{arm}: only c is gated");
    }
}

#[test]
fn a_failed_ticket_keeps_its_worktree_and_branch_skips_dependents_and_others_continue() {
    let s = Scratch::new();
    spec(&s, "");
    claude(&s, "a) exit 1;;");

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(ticket_wt(&s, "a").is_dir());
    let branch = format!("refs/heads/orchestrate/{}/a", s.run_id());
    s.git(&["rev-parse", "--verify", &branch]);
    let st = state(&s);
    assert_eq!(st["tickets"]["b"]["outcome"], "skipped");
    assert_eq!(st["tickets"]["b"]["reason"], "blocked by failed a");
    assert!(
        out.contains("ticket b skipped: blocked by failed a\n"),
        "{out}"
    );
    assert!(merged(&s).contains("Write docs"));
    assert!(out.contains("1 finished, 1 failed, 0 stuck"), "{out}");
}

#[test]
fn a_session_past_its_timeout_has_its_process_group_killed_and_times_out() {
    let s = Scratch::new();
    spec(&s, "timeout = \"3s\"\n");
    let pid = s.repo.parent().unwrap().join("grandchild.pid");
    claude(
        &s,
        &format!(
            "a) sleep 30 & echo $! > '{}'; {COMMIT}; wait;;",
            pid.display()
        ),
    );
    let started = Instant::now();

    // One at a time: under load a concurrent 1s session (c) could time out too.
    let out = s
        .run(&["run", "auth", "--branch", "int", "--max-parallel", "1"])
        .ok();

    assert!(started.elapsed() < Duration::from_secs(20));
    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "timeout");
    assert!(
        out.contains("ticket a timeout: no exit after 3s\n"),
        "{out}"
    );
    assert!(out.contains("1 finished, 1 failed"), "{out}");
    let pid = std::fs::read_to_string(pid).unwrap();
    assert!(!alive(pid.trim()), "grandchild {pid} survived");
    assert!(ticket_wt(&s, "a").is_dir());
}

#[test]
fn the_session_timeout_is_a_duration_string() {
    let s = Scratch::new();
    spec(&s, "timeout = 45\n");
    claude(&s, "");

    let err = s.run(&["run", "auth", "--branch", "int"]).fails();

    assert!(err.contains("timeout"), "{err}");
}

#[test]
fn an_interrupted_orchestrator_kills_its_sessions() {
    for signal in ["INT", "TERM", "HUP"] {
        let s = Scratch::new();
        spec(&s, "");
        let pid = s.repo.parent().unwrap().join("session.pid");
        claude(
            &s,
            &format!("a) sleep 30 & echo $! > '{}'; wait;;", pid.display()),
        );
        let path = format!("{}:{}", s.bin.display(), std::env::var("PATH").unwrap());
        let mut child = Command::new(env!("CARGO_BIN_EXE_orchestrate"))
            .args(["run", "auth", "--branch", "int"])
            .current_dir(&s.repo)
            .env("HOME", &s.home)
            .env("PATH", path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::fs::read_to_string(&pid).map_or(true, |p| p.trim().is_empty()) {
            assert!(Instant::now() < deadline, "session never started");
            std::thread::sleep(Duration::from_millis(50));
        }

        let sent = Command::new("kill")
            .args([&format!("-{signal}"), &child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
        let status = child.wait().unwrap();

        assert!(!status.success(), "{signal}");
        let pid = std::fs::read_to_string(&pid).unwrap();
        assert!(!alive(pid.trim()), "{signal}: session {pid} survived");
    }
}

fn alive(pid: &str) -> bool {
    // A killed process may linger briefly as a zombie until reparented and reaped.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let up = Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        if !up {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

#[test]
fn changes_the_session_left_uncommitted_fail_the_ticket_before_any_gate_or_conflict_session() {
    let done = report("done", true, "");
    for (arm, left) in [
        (format!("a) {COMMIT}; echo x > extra.txt; {done};;"), "extra.txt"),
        (format!("a) {COMMIT}; echo more >> a.txt; {done};;"), "a.txt"),
    ] {
        let s = Scratch::new();
        spec(&s, "");
        claude(&s, &arm);

        let out = s
            .run(&["run", "auth", "--branch", "int", "--max-parallel", "1"])
            .ok();

        let st = state(&s);
        assert_eq!(st["tickets"]["a"]["outcome"], "failed", "{arm}: {st}");
        let reason = format!("uncommitted changes in the worktree: {left}");
        assert_eq!(st["tickets"]["a"]["reason"], reason.as_str(), "{arm}");
        assert!(out.contains(&format!("ticket a failed: {reason}")), "{out}");
        assert_eq!(s.calls("gate").len(), 1, "{arm}: only c is gated");
        assert_eq!(s.calls("claude").len(), 2, "{arm}: no conflict session");
        assert!(!merged(&s).contains("Add schema"), "{arm}");
    }
}
