mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::Scratch;

/// Spec `auth`: `b` blocked by `a`, a passing gate, short tracker backoff.
fn chain() -> Scratch {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\
         retry_delay = \"10ms\"\nprobe_backoff = \"100ms\"\nprobe_backoff_max = \"300ms\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"true\"\n",
    );
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.ticket("auth", "b", "Build API", "ready-for-agent", &["a"]);
    s.commit_all();
    s.claude_lands();
    s
}

/// A pre-commit hook that fails the first `n` commits made in the integration worktree,
/// which is where the local markdown tracker commits mark-finished.
fn failing_commits(s: &Scratch, n: u32) {
    let hook = s.repo.join(".git/hooks/pre-commit");
    let count = s.bin.join("hook.count");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\ncase \"$PWD\" in */_integration) ;; *) exit 0;; esac\n\
             n=$(($(cat '{c}' 2>/dev/null || echo 0) + 1))\necho $n > '{c}'\n\
             [ $n -gt {n} ] || {{ echo tracker down >&2; exit 1; }}\n",
            c = count.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_log(s: &Scratch) -> String {
    let runs = s.repo_state(&s.state_root()).join("runs");
    fs::read_to_string(runs.join(s.run_id()).join("logs/run.log")).unwrap()
}

/// The one run's `run.log` while the run may still be creating its dirs, if it exists yet.
fn run_log_so_far(s: &Scratch) -> Option<String> {
    let first = |dir: &std::path::Path| Some(fs::read_dir(dir).ok()?.next()?.ok()?.path());
    let run = first(&first(&s.state_root())?.join("runs"))?;
    fs::read_to_string(run.join("logs/run.log")).ok()
}

/// Event messages without their `YYYY-MM-DD HH:MM:SS ` prefix.
fn events(out: &str) -> Vec<&str> {
    out.lines().map(|l| &l[20..]).collect()
}

fn position(events: &[&str], prefix: &str) -> usize {
    events
        .iter()
        .position(|e| e.starts_with(prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` in {events:#?}"))
}

#[test]
fn a_failing_mark_finished_pauses_the_run_probes_with_backoff_and_recovers() {
    let s = chain();
    // 3 attempts, then probes after 100ms (fails), 200ms (fails), 300ms (fails), 300ms (ok).
    failing_commits(&s, 6);

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    let events = events(&out);
    let paused = position(&events, "run paused: mark a finished failed: ");
    assert!(events[paused].contains("tracker down"), "{out}");
    assert!(events[paused].ends_with("; next probe in 100ms"), "{out}");
    let probes: Vec<_> = events
        .iter()
        .filter(|e| e.starts_with("tracker probe failed: "))
        .map(|e| e.rsplit(' ').next().unwrap())
        .collect();
    assert_eq!(probes, ["200ms", "300ms", "300ms"], "{out}");
    let recovered = position(&events, "run recovered: mark a finished succeeded");
    let b_started = position(&events, "ticket b started");
    assert!(paused < recovered && recovered < b_started, "{out}");
    // The write landed and the run carried on.
    let a = s.git(&["show", "int:specs/auth/a.md"]);
    assert!(a.contains("Status: done"), "{a}");
    s.git(&["cat-file", "-e", "int:b.txt"]);
    assert_eq!(run_log(&s), out);
}

#[test]
fn a_tracker_that_stops_answering_pauses_new_starts_until_it_answers_again() {
    let s = chain();
    // Ticket a's session takes the spec away from the tracker the run reads.
    let tracker = "../_integration/specs/auth";
    s.fake(
        "claude",
        &format!(
            "slug=$(basename \"$PWD\")\n\
             [ \"$slug\" = a ] && mv {tracker}/spec.md {tracker}/spec.md.away\n\
             echo \"$slug\" > \"$slug.txt\"\n\
             git add \"$slug.txt\" && git commit -q -m \"work $slug\"\n\
             printf '{{\"status\":\"done\",\"acceptance_criteria\":[],\"notes\":\"\"}}' \
             > .orchestrate-result.json"
        ),
    );

    let out = std::thread::scope(|scope| {
        let run = scope.spawn(|| s.run(&["run", "auth", "--branch", "int"]));
        // The user notices the pause and brings the tracker back.
        let log = loop {
            if let Some(log) = run_log_so_far(&s).filter(|l| l.contains("run paused")) {
                break log;
            }
            assert!(!run.is_finished(), "the run ended without pausing");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(
            log.contains("run paused: get spec auth failed: spec `auth` not found"),
            "{log}"
        );
        assert_eq!(s.calls("claude").len(), 1, "a session started while paused");
        let run_dir = s.repo_state(&s.state_root()).join("worktrees");
        let specs = run_dir.join(s.run_id()).join("_integration/specs/auth");
        fs::rename(specs.join("spec.md.away"), specs.join("spec.md")).unwrap();
        run.join().unwrap().ok()
    });

    let events = events(&out);
    let recovered = position(&events, "run recovered: get spec auth succeeded");
    assert!(position(&events, "run paused") < recovered, "{out}");
    assert!(recovered < position(&events, "ticket b started"), "{out}");
    s.git(&["cat-file", "-e", "int:b.txt"]);
}
