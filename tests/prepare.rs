mod common;

use common::Scratch;

/// A committed spec `auth` with one ready ticket `a`, a passing gate, and `extra`
/// appended to the repo config.
fn one_ticket(extra: &str) -> Scratch {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    let config = std::fs::read_to_string(s.repo.join(".orchestrate/config.toml")).unwrap();
    s.repo_config(&format!("{extra}\n{config}"));
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s
}

/// A fake `claude` that saves the output of `look`, run in its worktree, to `bin/seen`.
fn claude_looks(s: &Scratch, look: &str) {
    s.fake(
        "claude",
        &format!("({look}) > '{}/seen' 2>&1", s.bin.display()),
    );
}

fn seen(s: &Scratch) -> String {
    std::fs::read_to_string(s.bin.join("seen")).unwrap()
}

#[test]
fn seed_files_are_copied_from_the_primary_checkout_into_the_worktree() {
    let s = one_ticket("seed_files = [\".env\", \"config/local.toml\"]");
    // Untracked local files, only in the user's checkout.
    s.write(".env", "SECRET=1\n");
    s.write("config/local.toml", "x = 2\n");
    claude_looks(&s, "cat .env config/local.toml");

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(seen(&s), "SECRET=1\nx = 2\n");
}

#[test]
fn a_missing_seed_file_fails_the_ticket_as_a_seeding_failure_before_any_session() {
    let s = one_ticket("seed_files = [\".env\"]");
    claude_looks(&s, "true");

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(
        out.contains("ticket a failed: seeding failed: .env not found"),
        "{out}"
    );
    assert!(s.calls("claude").is_empty());
}

#[test]
fn prepare_runs_in_the_worktree_after_seeding_and_before_the_session() {
    let s = one_ticket("seed_files = [\".env\"]\nprepare = \"cat .env > prepared\"");
    s.write(".env", "SECRET=1\n");
    claude_looks(&s, "cat prepared");

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(seen(&s), "SECRET=1\n");
}

#[test]
fn a_prepare_failure_is_its_own_cause_with_output_in_the_prepare_log() {
    let s = one_ticket("prepare = \"echo installing; exit 3\"");
    claude_looks(&s, "true");

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(out.contains("ticket a failed: prepare exited 3"), "{out}");
    assert!(s.calls("claude").is_empty());
    let logs = s
        .repo_state(&s.state_root())
        .join("runs")
        .join(s.run_id())
        .join("logs");
    let log = std::fs::read_to_string(logs.join("a.prepare.log")).unwrap();
    assert_eq!(log, "installing\n");
}

#[test]
fn a_prepare_past_the_session_timeout_is_killed_and_fails_the_ticket() {
    let s = one_ticket("prepare = \"sleep 30\"");
    let config = std::fs::read_to_string(s.repo.join(".orchestrate/config.toml")).unwrap();
    s.repo_config(&format!("{config}\n[session]\ntimeout = \"1s\"\n"));
    s.commit_all();
    claude_looks(&s, "true");
    let start = std::time::Instant::now();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
    assert!(
        out.contains("ticket a failed: prepare timed out after 1s"),
        "{out}"
    );
    assert!(s.calls("claude").is_empty());
    let status = s.run(&["status"]).ok();
    assert!(status.contains("failed (prepare)"), "{status}");
}

#[test]
fn submodules_are_initialised_when_the_repo_has_them() {
    let s = one_ticket("");
    let lib = s.repo.parent().unwrap().join("lib");
    let lib = lib.to_str().unwrap();
    s.git(&["init", "-q", "-b", "main", lib]);
    s.git(&["-C", lib, "commit", "-q", "--allow-empty", "-m", "lib"]);
    // Local-path submodules need file transport; real ones are usually remote.
    s.git(&["config", "--global", "protocol.file.allow", "always"]);
    s.git(&["submodule", "add", "-q", lib, "vendor/lib"]);
    s.git(&["commit", "-q", "-m", "add lib"]);
    claude_looks(&s, "git -C vendor/lib log --format=%s");

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(seen(&s), "lib\n");
}

#[test]
fn the_prompt_carries_no_dependency_advice() {
    let s = one_ticket("prepare = \"touch deps-installed\"");
    s.claude_lands();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    let prompt = s.prompt("a");
    for word in ["deps-installed", "prepare", "install", "npm", "submodule"] {
        assert!(!prompt.contains(word), "{word} in {prompt}");
    }
}

#[test]
fn seeded_and_prepared_files_left_untracked_do_not_stop_the_ticket_landing() {
    let s = one_ticket("seed_files = [\".env\"]\nprepare = \"touch deps-installed\"");
    s.write(".env", "SECRET=1\n");
    s.claude_lands();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(out.contains("ticket a finished"), "{out}");
}
