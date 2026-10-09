mod common;

use std::fs;
use std::path::PathBuf;

use common::{Scratch, wait_until};

/// A spec `auth` with one ready ticket `a` and a fake `claude` that writes its pid to
/// `bin/claude.pid`, then blocks.
fn blocking() -> Scratch {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    let bin = s.bin.display();
    s.fake(
        "claude",
        &format!("echo $$ > '{bin}/claude.pid'\nwhile :; do sleep 0.05; done"),
    );
    s
}

/// Start a run of `auth`, wait for its session, and stop it.
fn stopped(s: &Scratch) {
    let mut run = s.spawn(&["run", "auth", "--branch", "int"]);
    let pid = s.bin.join("claude.pid");
    wait_until("session started", || pid.exists());
    s.run(&["stop"]).ok();
    run.wait().unwrap();
}

fn run_dir(s: &Scratch) -> PathBuf {
    s.repo_state(&s.state_root()).join("runs").join(s.run_id())
}

fn worktrees(s: &Scratch) -> PathBuf {
    s.repo_state(&s.state_root())
        .join("worktrees")
        .join(s.run_id())
}

fn state(s: &Scratch) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(run_dir(s).join("state.json")).unwrap()).unwrap()
}

/// Rewrite `state.json` with `f`.
fn edit_state(s: &Scratch, f: impl FnOnce(&mut serde_json::Value)) {
    let mut st = state(s);
    f(&mut st);
    fs::write(run_dir(s).join("state.json"), st.to_string()).unwrap();
}

fn run_log(s: &Scratch) -> String {
    fs::read_to_string(run_dir(s).join("logs/run.log")).unwrap()
}

/// `git` in the run's integration worktree, checked out again if run end removed it.
fn integration_git(s: &Scratch, args: &[&str]) -> String {
    let wt = worktrees(s).join("_integration");
    if !wt.exists() {
        s.git(&["worktree", "add", "-q", &wt.display().to_string(), "int"]);
    }
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(wt)
        .env("HOME", &s.home)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// A finished run of `auth` with one landed ticket `a`, then set back to `stopped`.
fn landed() -> Scratch {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s.claude_lands();
    s.run(&["run", "auth", "--branch", "int"]).ok();
    edit_state(&s, |st| st["status"] = "stopped".into());
    s
}

#[test]
fn an_in_flight_ticket_fails_then_restarts_in_a_suffixed_worktree_keeping_the_old_one() {
    let s = blocking();
    stopped(&s);
    assert_eq!(state(&s)["tickets"]["a"]["outcome"], "running");
    s.claude_lands();

    let out = s.run(&["resume", "--detach"]).ok();

    assert!(out.contains(&s.run_id()), "{out}");
    wait_until("run ended", || state(&s)["status"] == "ended");
    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "finished", "{st}");
    // The old worktree is kept; the restart worked in `a-2` with logs to match.
    assert!(worktrees(&s).join("a").is_dir());
    assert!(s.prompt("a-2").contains("it works"));
    assert!(run_dir(&s).join("logs/a-2.session.log").exists());
    let log = run_log(&s);
    let failed = log
        .find("ticket a failed: its process is gone")
        .expect(&log);
    let restarted = log.rfind("ticket a started").unwrap();
    assert!(failed < restarted, "{log}");
    assert!(integration_git(&s, &["log", "--format=%s"]).contains("work a-2"));
}

#[test]
fn resume_refuses_while_the_run_is_live() {
    let s = blocking();
    let mut run = s.spawn(&["run", "auth", "--branch", "int"]);
    let pid = s.bin.join("claude.pid");
    wait_until("session started", || pid.exists());

    let err = s.run(&["resume"]).fails();

    assert!(err.contains(&s.run_id()), "{err}");
    s.run(&["stop"]).ok();
    run.wait().unwrap();
}

#[test]
fn a_ticket_whose_merge_is_missing_from_the_integration_branch_becomes_failed() {
    let s = landed();
    let merge = integration_git(&s, &["log", "--merges", "--format=%H", "-1"]);
    integration_git(
        &s,
        &["reset", "-q", "--hard", &format!("{}^1", merge.trim())],
    );

    s.run(&["resume"]).ok();

    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "failed", "{st}");
    let reason = st["tickets"]["a"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("missing from the integration branch"),
        "{reason}"
    );
    // Not worked again.
    assert_eq!(s.calls("claude").len(), 1);
}

#[test]
fn a_ticket_finished_on_the_tracker_since_the_run_started_counts_as_done() {
    let s = blocking();
    stopped(&s);
    let ticket = worktrees(&s).join("_integration/specs/auth/a.md");
    let body = fs::read_to_string(&ticket).unwrap();
    fs::write(&ticket, body.replace("ready-for-agent", "done")).unwrap();
    integration_git(&s, &["commit", "-q", "-am", "done elsewhere"]);

    s.run(&["resume"]).ok();

    assert_eq!(state(&s)["tickets"]["a"]["outcome"], "finished");
    assert_eq!(s.calls("claude").len(), 1);
}

#[test]
fn a_landed_ticket_not_yet_marked_finished_is_marked_before_anything_new_starts() {
    let s = landed();
    // Crash after the merge, before write-back: drop the tracker commits, add a new ticket.
    let merge = integration_git(&s, &["log", "--merges", "--format=%H", "-1"]);
    integration_git(&s, &["reset", "-q", "--hard", merge.trim()]);
    edit_state(&s, |st| {
        st["tickets"]["a"]["outcome"] = "integrating".into()
    });
    let specs = worktrees(&s).join("_integration/specs/auth");
    let b = fs::read_to_string(specs.join("a.md"))
        .unwrap()
        .replace("Add schema", "Add api");
    fs::write(specs.join("b.md"), b).unwrap();
    integration_git(&s, &["add", "-A"]);
    integration_git(&s, &["commit", "-q", "-m", "new ticket"]);

    s.run(&["resume"]).ok();

    let st = state(&s);
    assert_eq!(st["tickets"]["a"]["outcome"], "finished", "{st}");
    assert_eq!(st["tickets"]["b"]["outcome"], "finished", "{st}");
    let a = s.git(&["show", "int:specs/auth/a.md"]);
    assert!(a.contains("Status: done"), "{a}");
    let log = run_log(&s);
    let marked = log.rfind("ticket a finished").expect(&log);
    assert!(marked < log.find("ticket b started").unwrap(), "{log}");
}

#[test]
fn stop_resolves_run_like_every_other_command() {
    let s = landed();
    // A second, live run: `auth-2` is now an ambiguous prefix.
    let bin = s.bin.display();
    s.fake(
        "claude",
        &format!("echo $$ > '{bin}/claude.pid'\nwhile :; do sleep 0.05; done"),
    );
    let ticket = s.repo.join("specs/auth/c.md");
    fs::write(
        &ticket,
        "# Add c\n\nStatus: ready-for-agent\n\n## Acceptance criteria\n\n- [ ] it works\n",
    )
    .unwrap();
    s.commit_all();
    let mut run = s.spawn(&["run", "auth", "--branch", "int2"]);
    wait_until("session started", || s.bin.join("claude.pid").exists());

    let err = s.run(&["stop", "--run", "auth-2"]).fails();
    assert!(err.contains("matches several runs"), "{err}");

    s.run(&["stop", "--run", "auth"]).ok();
    run.wait().unwrap();
}
