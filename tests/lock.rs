mod common;

use std::fs;
use std::path::PathBuf;

use common::{Scratch, alive, wait_until};

/// A spec `auth` with one ready ticket and a fake `claude` that writes its pid to
/// `bin/claude.pid`, then blocks until `bin/go` exists and exits 1.
fn blocking() -> Scratch {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    let bin = s.bin.display();
    s.fake(
        "claude",
        &format!(
            "echo $$ > '{bin}/claude.pid'\nwhile [ ! -f '{bin}/go' ]; do sleep 0.05; done\nexit 1"
        ),
    );
    s
}

fn lock_file(s: &Scratch) -> PathBuf {
    s.repo_state(&s.state_root()).join("lock")
}

/// Wait until the fake session is running and return its pid.
fn session_pid(s: &Scratch) -> String {
    let pid = s.bin.join("claude.pid");
    wait_until("session started", || pid.exists());
    fs::read_to_string(pid).unwrap().trim().to_string()
}

fn state(s: &Scratch) -> serde_json::Value {
    let run = s.repo_state(&s.state_root()).join("runs").join(s.run_id());
    serde_json::from_str(&fs::read_to_string(run.join("state.json")).unwrap()).unwrap()
}

#[test]
fn a_live_run_holds_the_lock_and_a_second_run_refuses_naming_it() {
    let s = blocking();
    let mut first = s.spawn(&["run", "auth", "--branch", "int"]);
    session_pid(&s);

    let lock = fs::read_to_string(lock_file(&s)).unwrap();
    assert!(lock.contains(&format!("pid = {}", first.id())), "{lock}");
    assert!(
        lock.contains(&format!("run = \"{}\"", s.run_id())),
        "{lock}"
    );
    let err = s.run(&["run", "auth", "--branch", "other"]).fails();
    assert!(err.contains(&s.run_id()), "{err}");
    s.run(&["plan", "auth"]).ok();

    fs::write(s.bin.join("go"), "").unwrap();
    assert!(first.wait().unwrap().success());
    assert!(!lock_file(&s).exists());
    // Only the first run's session ever started.
    assert_eq!(s.calls("claude").len(), 1);
}

#[test]
fn a_lock_whose_pid_is_gone_is_taken_over_with_a_notice() {
    let s = blocking();
    fs::write(s.bin.join("go"), "").unwrap();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    // A first run creates the per-repo dir; then plant a stale lock in it.
    s.run(&["run", "auth", "--branch", "int"]).ok();
    let repo_dir = s.repo_state(&s.state_root());
    fs::write(
        repo_dir.join("lock"),
        format!("pid = {}\nrun = \"auth-old\"\n", gone.id()),
    )
    .unwrap();

    let out = s.run(&["run", "auth", "--branch", "int2"]);

    out.ok();
    assert!(out.stderr().contains("auth-old"), "{}", out.stderr());
    assert!(!repo_dir.join("lock").exists());
}

#[test]
fn stop_kills_a_foreground_runs_session_and_marks_the_run_stopped() {
    let s = blocking();
    let mut run = s.spawn(&["run", "auth", "--branch", "int"]);
    let session = session_pid(&s);

    let out = s.run(&["stop"]).ok();

    assert!(out.contains(&s.run_id()), "{out}");
    assert!(!run.wait().unwrap().success());
    wait_until("session killed", || !alive(&session));
    assert_eq!(state(&s)["status"], "stopped");
    assert!(!lock_file(&s).exists());
}

#[test]
fn stop_with_a_run_id_that_is_not_live_refuses() {
    let s = blocking();
    let mut run = s.spawn(&["run", "auth", "--branch", "int"]);
    session_pid(&s);

    let err = s.run(&["stop", "--run", "nope"]).fails();

    assert!(err.contains(&s.run_id()), "{err}");
    s.run(&["stop", "--run", &s.run_id()]).ok();
    run.wait().unwrap();
}

#[test]
fn stop_with_no_live_run_refuses() {
    let s = blocking();
    let err = s.run(&["stop"]).fails();
    assert!(err.contains("no live run"), "{err}");
}

#[test]
fn a_detached_run_prints_its_id_and_log_and_runs_in_its_own_session_until_stopped() {
    let s = blocking();

    let out = s.run(&["run", "auth", "--branch", "int", "--detach"]).ok();

    let id = s.run_id();
    let log = s
        .repo_state(&s.state_root())
        .join("runs")
        .join(&id)
        .join("logs/run.log");
    assert!(out.contains(&id), "{out}");
    assert!(out.contains(&log.display().to_string()), "{out}");
    let session = session_pid(&s);
    let lock = fs::read_to_string(lock_file(&s)).unwrap();
    let pid = lock.lines().next().unwrap().trim_start_matches("pid = ");
    // setsid: the orchestrator leads its own session and process group.
    let pgid = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", pid])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&pgid.stdout).trim(), pid);

    s.run(&["stop"]).ok();

    wait_until("orchestrator gone", || !alive(pid));
    wait_until("session killed", || !alive(&session));
    assert_eq!(state(&s)["status"], "stopped");
    let log = fs::read_to_string(log).unwrap();
    assert!(log.contains("stopped"), "{log}");
}
