mod common;

use common::Scratch;

/// A spec `auth` with two chained ready tickets and a passing gate, committed, a fake
/// `claude` that lands every ticket, and a scratch remote holding `main`.
fn spec_with_remote() -> Scratch {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.ticket("auth", "b", "Build API", "ready-for-agent", &["a"]);
    s.commit_all();
    s.claude_lands();
    s.remote();
    s
}

/// Assert the run refused before anything was created or written.
fn assert_refused_untouched(s: &Scratch, err: &str, branch: &str, before: &str) {
    assert!(
        err.contains(&format!("`{branch}` is a protected branch")),
        "{err}"
    );
    assert!(s.calls("claude").is_empty());
    assert!(!s.state_root().exists());
    assert_eq!(s.git(&["rev-parse", "main"]), before);
}

#[test]
fn a_run_lands_without_pushing_anything_to_the_remote() {
    let s = spec_with_remote();
    let before = s.remote_refs();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(
        out.contains("push it with: git push -u origin int"),
        "{out}"
    );
    s.git(&["cat-file", "-e", "int:b.txt"]);
    assert_eq!(s.remote_refs(), before);
}

#[test]
fn an_existing_integration_branch_is_reused_with_its_commits() {
    let s = spec_with_remote();
    s.git(&["branch", "int"]);
    s.git(&["checkout", "-q", "int"]);
    s.write("started.txt", "mine\n");
    s.commit_all();
    s.git(&["checkout", "-q", "main"]);
    let started = s.git(&["rev-parse", "int"]);

    s.run(&["run", "auth", "--branch", "int"]).ok();

    s.git(&["merge-base", "--is-ancestor", started.trim(), "int"]);
    let merges = s.git(&[
        "rev-list",
        "--merges",
        "--count",
        &format!("{}..int", started.trim()),
    ]);
    assert_eq!(merges.trim(), "2");
}

#[test]
fn the_remotes_default_branch_is_protected_by_default() {
    let s = spec_with_remote();
    s.git(&["remote", "set-head", "origin", "--auto"]);
    let before = s.git(&["rev-parse", "main"]);

    let err = s.run(&["run", "auth", "--branch", "main"]).fails();

    assert_refused_untouched(&s, &err, "main", &before);
}

#[test]
fn the_default_protection_follows_the_remotes_head_not_the_local_branch_name() {
    let s = spec_with_remote();
    s.git(&["push", "-q", "origin", "main:trunk"]);
    s.remote_git(&["symbolic-ref", "HEAD", "refs/heads/trunk"]);
    let before = s.git(&["rev-parse", "main"]);
    let remote_before = s.remote_refs();

    let err = s.run(&["run", "auth", "--branch", "trunk"]).fails();

    assert_refused_untouched(&s, &err, "trunk", &before);
    assert!(!s.git(&["branch", "--list", "trunk"]).contains("trunk"));
    assert_eq!(s.remote_refs(), remote_before);
}

#[test]
fn configured_protected_branches_replace_the_default() {
    let s = spec_with_remote();
    s.repo_config(
        "version = 1\nprotected_branches = [\"release\"]\n\n[tracker]\nkind = \"local-markdown\"\n\
         location = \"specs\"\n\n[[gates]]\nname = \"check\"\nrun = \"true\"\n",
    );
    s.commit_all();
    let before = s.git(&["rev-parse", "main"]);

    let err = s.run(&["run", "auth", "--branch", "release"]).fails();

    assert_refused_untouched(&s, &err, "release", &before);
}
