mod common;

use std::fs;
use std::path::PathBuf;

use common::{Scratch, wait_until};

const COMMIT: &str =
    r#"echo "$slug" > "$slug.txt"; git add "$slug.txt"; git commit -q -m "work $slug""#;

/// A fake `claude` that lands every ticket except `b` (stuck) and `c` (exits 3).
fn claude(s: &Scratch) {
    let report = |status: &str| {
        format!(
            r#"printf '{{"status":"{status}","acceptance_criteria":[{{"text":"it works","met":true,"evidence":"ok"}}],"notes":"n"}}' > .orchestrate-result.json"#
        )
    };
    let (done, stuck) = (report("done"), report("stuck"));
    s.fake(
        "claude",
        &format!(
            "slug=$(basename \"$PWD\")\ncat > /dev/null\ncase \"$slug\" in\n\
             b) {COMMIT}; {stuck};;\nc) {COMMIT}; exit 3;;\n*) {COMMIT}; {done};;\nesac"
        ),
    );
}

/// An ended run of `auth`: `a` landed, `b` stuck, `c` failed.
fn ended() -> Scratch {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.ticket("auth", "b", "Build API", "ready-for-agent", &[]);
    s.ticket("auth", "c", "Write docs", "ready-for-agent", &[]);
    s.commit_all();
    claude(&s);
    s.run(&["run", "auth", "--branch", "int"]).ok();
    s
}

fn repo_state(s: &Scratch) -> PathBuf {
    s.repo_state(&s.state_root())
}

fn worktrees(s: &Scratch, run: &str) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(repo_state(s).join("worktrees").join(run))
        .into_iter()
        .flatten()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn branches(s: &Scratch) -> String {
    s.git(&[
        "branch",
        "--list",
        "--format=%(refname:short)",
        "orchestrate/*",
    ])
}

#[test]
fn clean_keeps_and_lists_failed_and_stuck_evidence_and_never_touches_the_integration_branch() {
    let s = ended();
    let id = s.run_id();
    let int = s.git(&["rev-parse", "int"]);
    assert_eq!(worktrees(&s, &id), ["b", "c"]);

    let out = s.run(&["clean"]).ok();

    assert!(out.contains(&format!("run {id}")), "{out}");
    assert!(out.contains("kept b (stuck)"), "{out}");
    assert!(out.contains("kept c (failed)"), "{out}");
    assert!(out.contains("--force"), "{out}");
    assert_eq!(worktrees(&s, &id), ["b", "c"]);
    assert_eq!(
        branches(&s),
        format!("orchestrate/{id}/b\norchestrate/{id}/c\n")
    );
    assert_eq!(s.git(&["rev-parse", "int"]), int);
}

#[test]
fn clean_force_removes_every_worktree_and_branch_of_the_run_then_prunes() {
    let s = ended();
    let id = s.run_id();
    let int = s.git(&["rev-parse", "int"]);
    // A worktree whose dir vanished behind git's back is pruned.
    fs::remove_dir_all(repo_state(&s).join("worktrees").join(&id).join("c")).unwrap();

    s.run(&["clean", "--force", "--run", &id]).ok();

    assert!(worktrees(&s, &id).is_empty());
    assert_eq!(branches(&s), "");
    let list = s.git(&["worktree", "list", "--porcelain"]);
    assert_eq!(list.matches("worktree ").count(), 1, "{list}");
    assert_eq!(s.git(&["rev-parse", "int"]), int);
    // The run's state and logs stay.
    assert!(
        repo_state(&s)
            .join("runs")
            .join(&id)
            .join("state.json")
            .is_file()
    );
}

/// A spec `auth` with one ticket `a` and a fake `claude` that writes its pid to
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

#[test]
fn clean_refuses_a_live_run() {
    let s = blocking();
    let mut run = s.spawn(&["run", "auth", "--branch", "int"]);
    wait_until("session started", || s.bin.join("claude.pid").exists());
    let id = s.run_id();

    let err = s.run(&["clean", "--force"]).fails();

    assert!(err.contains(&id), "{err}");
    assert!(err.contains("live"), "{err}");
    assert_eq!(worktrees(&s, &id), ["_integration", "a"]);
    s.run(&["stop"]).ok();
    run.wait().unwrap();
}

#[test]
fn superseded_resume_attempts_are_removed_even_without_force() {
    let s = blocking();
    let mut run = s.spawn(&["run", "auth", "--branch", "int"]);
    wait_until("session started", || s.bin.join("claude.pid").exists());
    s.run(&["stop"]).ok();
    run.wait().unwrap();
    // The restart in `a-2` gets stuck; `a` is the superseded attempt.
    s.fake(
        "claude",
        &format!(
            "cat > /dev/null; slug=$(basename \"$PWD\"); {COMMIT}; printf '{}' > .orchestrate-result.json",
            r#"{"status":"stuck","acceptance_criteria":[],"notes":"n"}"#
        ),
    );
    s.run(&["resume"]).ok();
    let id = s.run_id();
    assert_eq!(worktrees(&s, &id), ["a", "a-2"]);

    let out = s.run(&["clean"]).ok();

    assert!(out.contains("kept a (stuck)"), "{out}");
    assert_eq!(worktrees(&s, &id), ["a-2"]);
    assert_eq!(branches(&s), format!("orchestrate/{id}/a-2\n"));
}

#[test]
fn clean_all_covers_every_ended_run_and_purge_deletes_the_run_dirs() {
    let s = ended();
    let first = s.run_id();
    // A second, stopped run of another spec: not ended, so `--all` leaves it.
    s.spec("other", "Other");
    s.ticket("other", "x", "Do x", "ready-for-agent", &[]);
    s.commit_all();
    let bin = s.bin.display();
    s.fake(
        "claude",
        &format!("echo $$ > '{bin}/claude.pid'\nwhile :; do sleep 0.05; done"),
    );
    let mut run = s.spawn(&["run", "other", "--branch", "int2"]);
    wait_until("session started", || s.bin.join("claude.pid").exists());
    s.run(&["stop"]).ok();
    run.wait().unwrap();
    let runs = repo_state(&s).join("runs");
    let second = fs::read_dir(&runs)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|id| *id != first)
        .unwrap();

    s.run(&["clean", "--all", "--purge"]).ok();

    assert!(!runs.join(&first).exists());
    assert!(!repo_state(&s).join("worktrees").join(&first).exists());
    assert!(!branches(&s).contains(&first));
    assert!(runs.join(&second).join("state.json").is_file());
    assert!(worktrees(&s, &second).contains(&"x".to_string()));
    assert!(branches(&s).contains(&second));
}

#[test]
fn clean_orphans_removes_state_of_repos_that_are_gone_or_moved() {
    let s = ended();
    let mine = repo_state(&s);
    let root = s.state_root();
    let gone = root.join("gone-0123abcd");
    fs::create_dir_all(&gone).unwrap();
    fs::write(gone.join("repo.toml"), "path = \"/no/such/repo\"\n").unwrap();
    // The recorded path is a git repo, but another one than the dir was made for.
    let moved = root.join("repo-0123abcd");
    fs::create_dir_all(&moved).unwrap();
    let path = toml::Value::String(s.repo.display().to_string());
    fs::write(moved.join("repo.toml"), format!("path = {path}\n")).unwrap();

    let out = s.run(&["clean", "--orphans"]).ok();

    assert!(out.contains(&gone.display().to_string()), "{out}");
    assert!(out.contains(&moved.display().to_string()), "{out}");
    assert!(!out.contains(&mine.display().to_string()), "{out}");
    assert!(!gone.exists() && !moved.exists());
    assert!(mine.join("runs").is_dir());
}
