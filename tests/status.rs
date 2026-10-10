mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::Scratch;

const COMMIT: &str =
    r#"echo "$slug" > "$slug.txt"; git add "$slug.txt"; git commit -q -m "work $slug""#;

/// A report with `it works` met and `it is fast` unmet.
fn report(status: &str, notes: &str) -> String {
    format!(
        r#"printf '{{"status":"{status}","acceptance_criteria":[{{"text":"it works","met":true,"evidence":"tests pass"}},{{"text":"it is fast","met":false,"evidence":"too slow"}}],"notes":"{notes}"}}' > .orchestrate-result.json"#
    )
}

/// A fake `claude` running `arms` (sh `case` arms on the worktree name) and landing
/// every other ticket.
fn claude(s: &Scratch, arms: &str) {
    s.fake(
        "claude",
        &format!(
            "slug=$(basename \"$PWD\")\ncat > /dev/null\ncase \"$slug\" in\n{arms}\n*) {};;\nesac",
            land()
        ),
    );
}

/// An sh line that commits and reports `done` with every criterion met.
fn land() -> String {
    format!(
        r#"{COMMIT}; printf '{{"status":"done","acceptance_criteria":[{{"text":"it works","met":true,"evidence":"tests pass"}}],"notes":"all good"}}' > .orchestrate-result.json"#
    )
}

/// Spec `auth` with `tickets` as `(id, title, blocked by)`, and gate `check` running `gate`.
fn spec(s: &Scratch, gate: &str, tickets: &[(&str, &str, &[&str])]) {
    s.local_markdown_tracker_with_gate(gate);
    s.spec("auth", "Login rework");
    for (id, title, blockers) in tickets {
        s.ticket("auth", id, title, "ready-for-agent", blockers);
    }
    s.commit_all();
}

#[test]
fn status_shows_the_run_header_counts_and_one_line_per_ticket() {
    let s = Scratch::new();
    spec(
        &s,
        "true",
        &[
            ("a", "Add schema", &[]),
            ("b", "Build API", &["a"]),
            ("c", "Write docs", &[]),
            ("d", "Deploy docs", &["c"]),
            ("e", "Tune cache", &[]),
            ("f", "Use cache", &["e"]),
        ],
    );
    claude(
        &s,
        &format!(
            "c) exit 3;;\ne) {COMMIT}; {};;",
            report("stuck", "need a key")
        ),
    );
    s.run(&["run", "auth", "--branch", "int"]).ok();

    let out = s.run(&["status"]).ok();

    let id = s.run_id();
    assert_eq!(
        out,
        format!(
            "Run {id}: spec auth, integration branch int, ended\n\
             2 finished, 1 failed, 1 stuck, 1 held, 1 skipped\n\n\
             \x20 a  Add schema  finished\n\
             \x20 b  Build API  finished\n\
             \x20 c  Write docs  failed (session exited 3)\n\
             \x20 d  Deploy docs  skipped (blocked by failed c)\n\
             \x20 e  Tune cache  stuck\n\
             \x20 f  Use cache  held (blocked by stuck e)\n"
        )
    );
}

#[test]
fn status_of_a_ticket_shows_criteria_notes_gates_worktree_and_logs() {
    let s = Scratch::new();
    spec(
        &s,
        "test ! -f bad.txt",
        &[("bad", "Break it", &[]), ("e", "Tune cache", &[])],
    );
    claude(
        &s,
        &format!("e) {COMMIT}; {};;", report("stuck", "need a key")),
    );
    s.run(&["run", "auth", "--branch", "int"]).ok();
    let id = s.run_id();
    let state = s.repo_state(&s.state_root());
    let wt = state.join("worktrees").join(&id);
    let logs = state.join("runs").join(&id).join("logs");

    let out = s.run(&["status", "e"]).ok();
    assert_eq!(
        out,
        format!(
            "Ticket e: Tune cache\nOutcome: stuck\n\
             Criteria:\n  [x] it works: tests pass\n  [ ] it is fast: too slow\n\
             Notes:\n  need a key\n\
             Worktree: {}\n\
             Logs:\n  {}\n",
            wt.join("e").display(),
            logs.join("e.session.log").display(),
        )
    );

    let out = s.run(&["status", "bad"]).ok();
    assert_eq!(
        out,
        format!(
            "Ticket bad: Break it\nOutcome: failed (gate check)\n\
             Reason: gate check exited 1\n\
             Criteria:\n  [x] it works: tests pass\n\
             Notes:\n  all good\n\
             Gates:\n  check: exited 1\n\
             Worktree: {}\n\
             Logs:\n  {}\n  {}\n",
            wt.join("bad").display(),
            logs.join("bad.gate-check.log").display(),
            logs.join("bad.session.log").display(),
        )
    );

    let err = s.run(&["status", "nope"]).fails();
    assert!(
        err.contains(&format!("no ticket nope in run {id}")),
        "{err}"
    );
}

#[test]
fn a_landed_ticket_shows_its_passed_gates_and_its_removed_worktree() {
    let s = Scratch::new();
    spec(&s, "true", &[("a", "Add schema", &[])]);
    s.claude_lands();
    s.run(&["run", "auth", "--branch", "int"]).ok();

    let out = s.run(&["status", "a"]).ok();

    assert!(out.contains("Outcome: finished\n"), "{out}");
    assert!(out.contains("Gates:\n  check: exited 0\n"), "{out}");
    assert!(out.contains("/a (removed)\n"), "{out}");
}

#[test]
fn a_timeout_or_prepare_failure_shows_as_failed_with_that_cause() {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\nprepare = \"test ! -f prep-fails\"\n\n\
         [tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"true\"\n\n[session]\ntimeout = \"1s\"\n",
    );
    s.spec("auth", "Login rework");
    s.ticket("auth", "slow", "Be slow", "ready-for-agent", &[]);
    s.commit_all();
    claude(&s, "slow) sleep 10;;");
    s.run(&["run", "auth", "--branch", "int"]).ok();

    let out = s.run(&["status"]).ok();
    assert!(out.contains("\n1 failed\n"), "{out}");
    assert!(out.contains("  slow  Be slow  failed (timeout)\n"), "{out}");

    s.write("prep-fails", "");
    s.spec("other", "Other");
    s.ticket("other", "p", "Prepare", "ready-for-agent", &[]);
    s.commit_all();
    s.run(&["run", "other", "--branch", "int2"]).ok();

    let out = s.run(&["status", "--run", "other"]).ok();
    assert!(out.contains("  p  Prepare  failed (prepare)\n"), "{out}");
}

/// Start `orchestrate run auth --branch int` in the background.
fn spawn_run(s: &Scratch) -> Child {
    let path = format!("{}:{}", s.bin.display(), std::env::var("PATH").unwrap());
    Command::new(env!("CARGO_BIN_EXE_orchestrate"))
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
        .unwrap()
}

/// Poll `status` until its output contains `want`; return that output.
fn status_until(s: &Scratch, want: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let run = s.run(&["status"]);
        if run.code() == 0 && run.stdout().contains(want) {
            return run.stdout();
        }
        let (out, err) = (run.stdout(), run.stderr());
        assert!(Instant::now() < deadline, "no `{want}` in:\n{out}{err}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// An sh line that blocks until `bin/<file>` exists.
fn wait_for(s: &Scratch, file: &str) -> String {
    let path = s.bin.join(file);
    format!("while [ ! -f '{}' ]; do sleep 0.05; done", path.display())
}

#[test]
fn a_live_run_shows_running_with_elapsed_time_then_integrating() {
    let s = Scratch::new();
    spec(&s, &wait_for(&s, "go-gate"), &[("a", "Add schema", &[])]);
    claude(
        &s,
        &format!("a) {}; {};;", wait_for(&s, "go-session"), land()),
    );
    let mut child = spawn_run(&s);

    let out = status_until(&s, "  a  Add schema  running (");
    assert!(out.contains(", running\n1 running\n"), "{out}");
    let line = out.lines().find(|l| l.starts_with("  a  ")).unwrap();
    assert!(line.ends_with("s)"), "{line}");

    fs::write(s.bin.join("go-session"), "").unwrap();
    let out = status_until(&s, "  a  Add schema  integrating\n");
    assert!(out.contains("\n1 integrating\n"), "{out}");

    fs::write(s.bin.join("go-gate"), "").unwrap();
    assert!(child.wait().unwrap().success());
    status_until(&s, "  a  Add schema  finished\n");
}

#[test]
fn a_paused_run_shows_paused_with_its_reason_until_it_recovers() {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\
         retry_delay = \"10ms\"\nprobe_backoff = \"50ms\"\nprobe_backoff_max = \"50ms\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"true\"\n",
    );
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s.claude_lands();
    // Commits in the integration worktree, so mark-finished, fail while `bin/down` exists.
    let down = s.bin.join("down");
    fs::write(&down, "").unwrap();
    let hook = s.repo.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\ncase \"$PWD\" in */_integration) ;; *) exit 0;; esac\n\
             [ ! -f '{}' ] || {{ echo tracker down >&2; exit 1; }}\n",
            down.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = spawn_run(&s);

    let out = status_until(&s, ", paused (mark a finished failed: ");
    assert!(out.contains("tracker down"), "{out}");

    fs::remove_file(&down).unwrap();
    assert!(child.wait().unwrap().success());
    let out = status_until(&s, ", ended\n");
    assert!(out.contains("  a  Add schema  finished\n"), "{out}");
}

/// The run id `status` shows.
fn shown_run(s: &Scratch, args: &[&str]) -> String {
    let out = s.run(args).ok();
    let header = out.strip_prefix("Run ").unwrap();
    header.split(':').next().unwrap().to_string()
}

#[test]
fn runs_lists_the_repos_runs_and_run_resolves_an_id_prefix_or_spec_id() {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    for spec in ["S.1", "other"] {
        s.spec(spec, "A spec");
        s.ticket(spec, "a", "Add schema", "ready-for-agent", &[]);
    }
    s.commit_all();
    s.claude_lands();
    let ids: Vec<String> = [("S.1", "i1"), ("S.1", "i2"), ("other", "i3")]
        .iter()
        .map(|(spec, branch)| {
            if *branch == "i2" {
                // Run ids carry the start time to the second.
                std::thread::sleep(Duration::from_millis(1100));
            }
            let out = s.run(&["run", spec, "--branch", branch]).ok();
            out.split(' ').nth(3).unwrap().to_string()
        })
        .collect();
    let [first, second, third] = [&ids[0], &ids[1], &ids[2]];
    assert!(
        first.starts_with("s-1-") && third.starts_with("other-"),
        "{ids:?}"
    );

    assert_eq!(
        s.run(&["runs"]).ok(),
        format!(
            "{third}  spec other  ended\n{second}  spec S.1  ended\n{first}  spec S.1  ended\n"
        )
    );
    assert_eq!(shown_run(&s, &["status"]), *third);
    assert_eq!(shown_run(&s, &["status", "--run", first]), *first);
    assert_eq!(shown_run(&s, &["status", "--run", "oth"]), *third);
    assert_eq!(shown_run(&s, &["status", "--run", "S.1"]), *second);
    s.run(&["status", "a", "--run", "S.1"]).ok();

    let err = s.run(&["status", "--run", "s-1"]).fails();
    let want = format!("s-1 matches several runs: {second}, {first}");
    assert!(err.contains(&want), "{err}");
    let err = s.run(&["status", "--run", "zzz"]).fails();
    assert!(err.contains("no run matches zzz"), "{err}");
}

#[test]
fn status_and_runs_are_read_only() {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");

    assert_eq!(s.run(&["runs"]).ok(), "no runs in this repo\n");
    let err = s.run(&["status"]).fails();
    assert!(err.contains("no runs in this repo"), "{err}");
    assert!(!s.state_root().exists());

    spec(&s, "true", &[("a", "Add schema", &[])]);
    s.claude_lands();
    s.run(&["run", "auth", "--branch", "int"]).ok();
    let runs = s.repo_state(&s.state_root()).join("runs");
    let state = runs.join(s.run_id()).join("state.json");
    let before = fs::read_to_string(&state).unwrap();
    s.run(&["status"]).ok();
    s.run(&["status", "a"]).ok();
    s.run(&["runs"]).ok();
    assert_eq!(fs::read_to_string(&state).unwrap(), before);
    assert_eq!(fs::read_dir(&runs).unwrap().count(), 1);
}
