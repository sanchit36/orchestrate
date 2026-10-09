mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use common::Scratch;

const LAND: &str = r#"echo "$slug" > "$slug.txt"; git add "$slug.txt"; git commit -q -m "work $slug"
printf '{"status":"done","acceptance_criteria":[{"text":"it works","met":true,"evidence":"ok"}],"notes":""}' > .orchestrate-result.json"#;

/// Repo config with a short tracker backoff and a gate running `gate`.
fn config(s: &Scratch, gate: &str) {
    s.repo_config(&format!(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\
         retry_delay = \"10ms\"\nprobe_backoff = \"100ms\"\nprobe_backoff_max = \"300ms\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"{gate}\"\n"
    ));
}

/// Spec `auth` with independent ready tickets `ids`, committed.
fn independent(s: &Scratch, ids: &[&str]) {
    config(s, "true");
    s.spec("auth", "Login rework");
    for id in ids {
        s.ticket("auth", id, &format!("Ticket {id}"), "ready-for-agent", &[]);
    }
    s.commit_all();
}

/// A fake `claude` that runs `before` (sh, `$slug` and `$bin` set), then lands.
fn claude(s: &Scratch, before: &str) {
    s.fake(
        "claude",
        &format!(
            "slug=$(basename \"$PWD\")\nbin='{}'\ncat > /dev/null\n{before}\n{LAND}",
            s.bin.display()
        ),
    );
}

/// Sessions that wait (up to 10 s) for `want` sessions to be live, then record how many
/// are live in `bin/live-counts`. More than `want` shows up as a higher count, fewer as
/// a lower one.
fn claude_counts_live(s: &Scratch, want: usize) {
    claude(
        s,
        &format!(
            "mkdir \"$bin/live.$slug\"\nlive() {{ ls -d \"$bin\"/live.* | wc -l; }}\n\
             i=0; while [ $(live) -lt {want} ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i+1)); done\n\
             sleep 0.3\nlive >> \"$bin/live-counts\"\nrmdir \"$bin/live.$slug\""
        ),
    );
}

fn most_live(s: &Scratch) -> usize {
    fs::read_to_string(s.bin.join("live-counts"))
        .unwrap()
        .lines()
        .map(|l| l.trim().parse::<usize>().unwrap())
        .max()
        .unwrap()
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

fn run_log(s: &Scratch) -> Option<String> {
    let runs = s
        .state_root()
        .exists()
        .then(|| s.repo_state(&s.state_root()))?;
    let runs = runs.join("runs");
    let id = fs::read_dir(&runs).ok()?.next()?.ok()?.file_name();
    fs::read_to_string(runs.join(id).join("logs/run.log")).ok()
}

/// Wait until the run log contains `text`, failing after 30 s.
fn wait_for_log(s: &Scratch, text: &str) -> String {
    let start = Instant::now();
    loop {
        if let Some(log) = run_log(s).filter(|l| l.contains(text)) {
            return log;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "no `{text}`");
        sleep(Duration::from_millis(20));
    }
}

fn wait_for_file(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(start.elapsed() < Duration::from_secs(30), "no {path:?}");
        sleep(Duration::from_millis(20));
    }
}

fn integration_specs(s: &Scratch) -> std::path::PathBuf {
    let worktrees = s.repo_state(&s.state_root()).join("worktrees");
    let id = fs::read_dir(&worktrees).unwrap().next().unwrap().unwrap();
    id.path().join("_integration/specs/auth")
}

#[test]
fn auto_parallelism_is_the_graph_width_capped_by_half_the_cores_between_2_and_6() {
    let s = Scratch::new();
    independent(&s, &["a", "b", "c", "d"]);
    let cores = std::thread::available_parallelism().unwrap().get();
    let expected = 4.min(2.max(6.min(cores / 2)));
    claude_counts_live(&s, expected);

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(
        out.contains(&format!("max parallel: {expected}\n")),
        "{out}"
    );
    assert_eq!(most_live(&s), expected, "{out}");
    for id in ["a", "b", "c", "d"] {
        s.git(&["cat-file", "-e", &format!("int:{id}.txt")]);
    }
}

#[test]
fn the_graph_width_is_the_widest_level_of_open_tickets() {
    let s = Scratch::new();
    config(&s, "true");
    s.spec("auth", "Login rework");
    // Levels: {a}, {b, c}, {d}; e is finished and does not count.
    s.ticket("auth", "a", "A", "ready-for-agent", &[]);
    s.ticket("auth", "b", "B", "ready-for-agent", &["a"]);
    s.ticket("auth", "c", "C", "ready-for-agent", &["a"]);
    s.ticket("auth", "d", "D", "ready-for-agent", &["b"]);
    s.ticket("auth", "e", "E", "done", &[]);
    s.commit_all();
    s.claude_lands();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(out.contains("max parallel: 2\n"), "{out}");
}

#[test]
fn max_parallel_overrides_the_derived_number_from_the_flag_or_the_machine_config() {
    let s = Scratch::new();
    independent(&s, &["a", "b", "c", "d"]);
    claude_counts_live(&s, 1);
    s.machine_config("version = 1\nmax_parallel = 1\n");

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();
    assert!(out.contains("max parallel: 1\n"), "{out}");
    assert_eq!(most_live(&s), 1);

    let s = Scratch::new();
    independent(&s, &["a", "b", "c", "d", "e", "f"]);
    claude_counts_live(&s, 3);
    s.machine_config("version = 1\nmax_parallel = 1\n");

    let out = s
        .run(&["run", "auth", "--branch", "int", "--max-parallel", "3"])
        .ok();
    assert!(out.contains("max parallel: 3\n"), "{out}");
    assert_eq!(most_live(&s), 3);
}

#[test]
fn max_parallel_must_be_auto_or_a_positive_number() {
    let s = Scratch::new();
    independent(&s, &["a"]);
    let err = s
        .run(&["run", "auth", "--branch", "int", "--max-parallel", "0"])
        .fails();
    assert!(err.contains("expected `auto` or a number above 0"), "{err}");

    s.machine_config("version = 1\nmax_parallel = \"lots\"\n");
    let err = s.run(&["run", "auth", "--branch", "int"]).fails();
    assert!(err.contains("expected `auto` or a number above 0"), "{err}");
}

#[test]
fn sessions_run_concurrently_while_landings_happen_one_at_a_time() {
    let s = Scratch::new();
    // A gate that notices another gate running at the same time.
    s.fake(
        "gate",
        &format!(
            "bin='{}'\nmkdir \"$bin/gate.lock\" || echo overlap >> \"$bin/overlap\"\n\
             sleep 0.3\nrmdir \"$bin/gate.lock\"",
            s.bin.display()
        ),
    );
    config(&s, "gate");
    s.spec("auth", "Login rework");
    for id in ["a", "b", "c"] {
        s.ticket("auth", id, id, "ready-for-agent", &[]);
    }
    s.commit_all();
    claude_counts_live(&s, 3);

    let out = s
        .run(&["run", "auth", "--branch", "int", "--max-parallel", "3"])
        .ok();

    assert_eq!(most_live(&s), 3);
    assert_eq!(s.calls("gate").len(), 3);
    assert!(!s.bin.join("overlap").exists(), "gates overlapped:\n{out}");
    let merges = s.git(&["log", "--first-parent", "--merges", "--format=%s", "int"]);
    assert_eq!(merges.lines().count(), 3, "{merges}");
}

#[test]
fn simultaneously_startable_tickets_start_in_tracker_order() {
    let s = Scratch::new();
    independent(&s, &["x1", "x2", "x3"]);
    claude(&s, "sleep 0.5");

    let out = s
        .run(&["run", "auth", "--branch", "int", "--max-parallel", "2"])
        .ok();

    let events = events(&out);
    let mut first_two = [
        position(&events, "ticket x1 started"),
        position(&events, "ticket x2 started"),
    ];
    first_two.sort();
    let x3 = position(&events, "ticket x3 started");
    let first_finished =
        position(&events, "ticket x1 finished").min(position(&events, "ticket x2 finished"));
    assert!(first_two[1] < x3 && first_finished < x3, "{out}");
}

#[test]
fn tickets_added_or_made_ready_mid_run_are_picked_up_and_unready_running_tickets_finish() {
    let s = Scratch::new();
    config(&s, "true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "A", "ready-for-agent", &[]);
    s.ticket("auth", "e", "E", "needs-triage", &[]);
    s.commit_all();
    // a's session stands in for the user editing the tracker the run reads.
    claude(
        &s,
        "t=../_integration/specs/auth\n\
         if [ \"$slug\" = a ]; then\n\
         printf '# A\\n\\nStatus: needs-triage\\n\\nbody\\n' > $t/a.md\n\
         printf '# E\\n\\nStatus: ready-for-agent\\n\\nbody\\n' > $t/e.md\n\
         printf '# D\\n\\nStatus: ready-for-agent\\n\\nbody\\n' > $t/d.md\n\
         (cd $t && git add . && git commit -q -m 'edit tracker')\n\
         fi",
    );

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    for id in ["a", "d", "e"] {
        s.git(&["cat-file", "-e", &format!("int:{id}.txt")]);
        let ticket = s.git(&["show", &format!("int:specs/auth/{id}.md")]);
        assert!(ticket.contains("Status: done"), "{ticket}");
    }
    assert!(out.contains("3 finished"), "{out}");
}

#[test]
fn a_cycle_appearing_mid_run_stops_new_starts_while_running_sessions_finish() {
    let s = Scratch::new();
    config(&s, "true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "A", "ready-for-agent", &[]);
    s.ticket("auth", "b", "B", "ready-for-agent", &[]);
    s.ticket("auth", "c", "C", "ready-for-agent", &["a"]);
    s.commit_all();
    // a's session makes c and a new d block each other; b is still running.
    claude(
        &s,
        "t=../_integration/specs/auth\n\
         case \"$slug\" in\n\
         a) printf '# C\\n\\nStatus: ready-for-agent\\nBlocked by: d\\n\\nbody\\n' > $t/c.md\n\
         printf '# D\\n\\nStatus: ready-for-agent\\nBlocked by: c\\n\\nbody\\n' > $t/d.md\n\
         (cd $t && git add . && git commit -q -m 'add cycle');;\n\
         b) while [ ! -e \"$bin/release\" ]; do sleep 0.05; done;;\n\
         esac",
    );

    let out = std::thread::scope(|scope| {
        let run = scope.spawn(|| s.run(&["run", "auth", "--branch", "int"]));
        wait_for_log(&s, "run paused: blocking cycle: c -> d -> c");
        fs::write(s.bin.join("release"), "").unwrap();
        wait_for_log(&s, "ticket b finished");
        sleep(Duration::from_millis(300));
        assert_eq!(s.calls("claude").len(), 2, "a session started in a cycle");
        assert!(!run.is_finished(), "a cycle ended the run");
        // The user breaks the cycle.
        let specs = integration_specs(&s);
        fs::write(
            specs.join("d.md"),
            "# D\n\nStatus: ready-for-agent\n\nbody\n",
        )
        .unwrap();
        run.join().unwrap().ok()
    });

    let events = events(&out);
    let paused = position(&events, "run paused: blocking cycle");
    let recovered = position(&events, "run recovered: blocking cycle resolved");
    assert!(paused < position(&events, "ticket b finished"), "{out}");
    assert!(recovered < position(&events, "ticket d started"), "{out}");
    assert!(position(&events, "ticket d finished") < position(&events, "ticket c started"));
}

/// A pre-commit hook that fails every commit made in the integration worktree, which is
/// where the local markdown tracker commits mark-finished.
fn tracker_down(s: &Scratch) -> std::path::PathBuf {
    let hook = s.repo.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        "#!/bin/sh\ncase \"$PWD\" in */_integration) echo tracker down >&2; exit 1;; esac\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    hook
}

#[test]
fn running_sessions_carry_on_while_the_tracker_is_paused() {
    let s = Scratch::new();
    independent(&s, &["a", "b"]);
    // b's session waits for the test, then finishes while the run is paused.
    claude(
        &s,
        "[ \"$slug\" = b ] && { while [ ! -e \"$bin/release\" ]; do sleep 0.05; done; \
         touch \"$bin/b.ran\"; }",
    );
    let hook = tracker_down(&s);

    std::thread::scope(|scope| {
        let run = scope.spawn(|| s.run(&["run", "auth", "--branch", "int"]));
        wait_for_log(&s, "run paused: mark a finished failed");
        fs::write(s.bin.join("release"), "").unwrap();
        wait_for_file(&s.bin.join("b.ran"));
        let log = run_log(&s).unwrap();
        assert!(!log.contains("run recovered"), "{log}");
        fs::remove_file(hook).unwrap();
        let out = run.join().unwrap().ok();
        let events = events(&out);
        let recovered = position(&events, "run recovered: mark a finished succeeded");
        assert!(recovered < position(&events, "ticket b finished"), "{out}");
    });
    s.git(&["cat-file", "-e", "int:b.txt"]);
}
