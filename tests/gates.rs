mod common;

use std::fs;

use common::Scratch;

/// A spec `auth` with one ready ticket `a`, the given `[[gates]]` TOML (see
/// `with_record`), all committed, and a fake `claude` that commits each of `files`.
fn ticket_with_gates(gates: &str, files: &[&str]) -> Scratch {
    let s = Scratch::new();
    let gates = with_record(&s, gates);
    s.repo_config(&format!(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\n{gates}"
    ));
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    let writes: String = files
        .iter()
        .map(|f| format!("mkdir -p \"$(dirname '{f}')\" && echo '{f}' > '{f}'\n"))
        .collect();
    s.fake(
        "claude",
        &format!(
            "{writes}git add -A && git commit -q -m work\n\
             printf '{{\"status\":\"done\",\"acceptance_criteria\":[{{\"text\":\"it works\",\"met\":true,\"evidence\":\"ok\"}}],\"notes\":\"\"}}' > .orchestrate-result.json"
        ),
    );
    s
}

/// Lines appended to `bin/gates` by gate commands (`echo x >> {GATES}`).
fn gate_lines(s: &Scratch) -> Vec<String> {
    fs::read_to_string(s.bin.join("gates"))
        .map(|t| t.lines().map(String::from).collect())
        .unwrap_or_default()
}

fn run_log(s: &Scratch) -> String {
    let logs = s.repo_state(&s.state_root()).join("runs").join(s.run_id());
    fs::read_to_string(logs.join("logs/run.log")).unwrap()
}

fn log_path(s: &Scratch, name: &str) -> std::path::PathBuf {
    let runs = s.repo_state(&s.state_root()).join("runs");
    runs.join(s.run_id()).join("logs").join(name)
}

/// `gates` with `{GATES}` replaced by the absolute `bin/gates` record file.
fn with_record(s: &Scratch, gates: &str) -> String {
    gates.replace("{GATES}", &s.bin.join("gates").display().to_string())
}

#[test]
fn gates_run_in_config_order_and_the_first_failure_fails_the_ticket() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"one\"\nrun = 'echo one >> {GATES}'\n\n\
         [[gates]]\nname = \"two\"\nrun = 'echo two >> {GATES}; exit 3'\n\n\
         [[gates]]\nname = \"three\"\nrun = 'echo three >> {GATES}'\n",
        &["a.txt"],
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(gate_lines(&s), ["one", "two"]);
    let log = run_log(&s);
    assert!(log.contains("ticket a failed: gate two exited 3"), "{log}");
}

#[test]
fn a_gate_runs_only_when_the_ticket_changed_files_matching_its_paths() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"docs\"\nrun = 'echo docs >> {GATES}'\npaths = [\"docs/**\"]\n\n\
         [[gates]]\nname = \"rust\"\nrun = 'echo rust >> {GATES}'\npaths = [\"*.md\", \"src/**/*.rs\"]\n\n\
         [[gates]]\nname = \"all\"\nrun = 'echo all >> {GATES}'\n",
        &["src/deep/lib.rs"],
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(gate_lines(&s), ["rust", "all"]);
    s.git(&["cat-file", "-e", "int:src/deep/lib.rs"]);
}

#[test]
fn a_gate_without_paths_checks_a_ticket_whose_commits_change_no_files() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"docs\"\nrun = 'echo docs >> {GATES}'\npaths = [\"docs/**\"]\n\n\
         [[gates]]\nname = \"all\"\nrun = 'echo all >> {GATES}'\n",
        &[],
    );
    s.fake(
        "claude",
        "git commit -q --allow-empty -m nothing\n\
         printf '{\"status\":\"done\",\"acceptance_criteria\":[{\"text\":\"it works\",\"met\":true}]}' \
         > .orchestrate-result.json",
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(gate_lines(&s), ["all"]);
    assert!(run_log(&s).contains("ticket a finished"));
}

#[test]
fn a_ticket_no_gate_checked_fails_instead_of_landing_unchecked() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"docs\"\nrun = 'echo docs >> {GATES}'\npaths = [\"docs/**\"]\n",
        &["src/lib.rs"],
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(gate_lines(&s).is_empty());
    let log = run_log(&s);
    assert!(
        log.contains("ticket a failed: no gate matched its change"),
        "{log}"
    );
    assert!(
        s.git(&["ls-tree", "-r", "--name-only", "int"])
            .lines()
            .all(|f| f != "src/lib.rs")
    );
}

/// The working directories gates recorded with `pwd`, relative to the ticket worktree.
fn gate_dirs(s: &Scratch) -> Vec<String> {
    gate_lines(s)
        .iter()
        .map(|l| match l.rsplit_once("/a/") {
            Some((_, rel)) => rel.to_string(),
            None => ".".into(),
        })
        .collect()
}

#[test]
fn a_gate_with_dirs_runs_once_in_each_listed_folder_the_change_touched() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"pkg\"\nrun = 'pwd >> {GATES}'\ndirs = [\"pkgs/*\", \"tools\"]\n",
        &[
            "pkgs/x/a.txt",
            "pkgs/x/deep/b.txt",
            "pkgs/y/c.txt",
            "pkgs/top.txt",
            "other/d.txt",
        ],
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(gate_dirs(&s), ["pkgs/x", "pkgs/y"]);
}

#[test]
fn a_gate_without_dirs_runs_once_at_the_repo_root_or_its_fixed_cwd() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"root\"\nrun = 'pwd >> {GATES}'\n\n\
         [[gates]]\nname = \"web\"\nrun = 'pwd >> {GATES}'\ncwd = \"web/app\"\n",
        &["web/app/a.txt", "web/b.txt"],
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(gate_dirs(&s), [".", "web/app"]);
}

#[test]
fn a_gate_with_both_dirs_and_cwd_or_a_bad_glob_is_a_config_error() {
    for (gate, want) in [
        (
            "dirs = [\"a\"]\ncwd = \"b\"",
            "gate check: set `dirs` or `cwd`, not both",
        ),
        ("paths = [\"a[\"]", "bad glob `a[`"),
        ("dirs = [\"{\"]", "bad glob `{`"),
    ] {
        let s = ticket_with_gates(
            &format!("[[gates]]\nname = \"check\"\nrun = 'true'\n{gate}\n"),
            &[],
        );

        let err = s.run(&["plan", "auth"]).fails();

        assert!(err.contains(want), "{err}");
    }
}

#[test]
fn a_gate_past_its_timeout_fails_and_still_runs_its_teardown() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"slow\"\nrun = '(sleep 2; echo late >> {GATES}) & sleep 30'\n\
         timeout = \"1s\"\nteardown = 'echo torn >> {GATES}'\n",
        &["a.txt"],
    );
    let start = std::time::Instant::now();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(start.elapsed().as_secs() < 15, "{:?}", start.elapsed());
    // The gate's whole process group was killed, so its background job never writes.
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(gate_lines(&s), ["torn"]);
    let log = run_log(&s);
    assert!(
        log.contains("ticket a failed: gate slow timed out after 1s"),
        "{log}"
    );
}

#[test]
fn teardown_runs_after_a_passing_and_a_failing_gate() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"one\"\nrun = 'echo one >> {GATES}'\nteardown = 'echo down-one >> {GATES}'\n\n\
         [[gates]]\nname = \"two\"\nrun = 'echo two >> {GATES}; exit 1'\nteardown = 'echo down-two >> {GATES}'\n",
        &["a.txt"],
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(gate_lines(&s), ["one", "down-one", "two", "down-two"]);
}

#[test]
fn each_gate_logs_its_full_stdout_and_stderr_and_its_teardown() {
    let s = ticket_with_gates(
        "[[gates]]\nname = \"check\"\nrun = 'echo to-out; echo to-err >&2'\nteardown = 'echo cleaned'\n",
        &["a.txt"],
    );

    s.run(&["run", "auth", "--branch", "int"]).ok();

    let log = fs::read_to_string(log_path(&s, "a.gate-check.log")).unwrap();
    assert_eq!(log, "to-out\nto-err\ncleaned\n");
}
