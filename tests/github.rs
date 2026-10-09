mod common;
mod fake_gh;

use common::Scratch;
use fake_gh::FakeGh;

const CRITERIA: &str = "## Acceptance criteria\r\n\r\n- [ ] it works\r\n";

/// Spec issue #10 on `acme/app` with sub-issues #11 (closed), #12 (ready, natively blocked
/// by #11), #13 (ready, blocked by `#12` in its body) and #14 (not ready), served by a fake
/// `gh` on `PATH`. The repo config names a `check` gate running `true`.
fn github_spec() -> (Scratch, FakeGh) {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\n\n[tracker]\nkind = \"github\"\nlocation = \"acme/app\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"true\"\n",
    );
    s.commit_all();
    let gh = FakeGh {
        dir: s.home.join("issues"),
    };
    gh.issue(10, "Login rework", "The spec body.", false, &[]);
    gh.sub_issues(10, &[11, 12, 13, 14]);
    gh.issue(11, "Add schema", CRITERIA, true, &["ready-for-agent"]);
    gh.issue(12, "Build API", CRITERIA, false, &["ready-for-agent"]);
    gh.blocked_by(12, &[11]);
    let body = format!("Blocked by: #12\r\n\r\n{CRITERIA}");
    gh.issue(13, "Build UI", &body, false, &["ready-for-agent", "ui"]);
    gh.issue(14, "Write docs", CRITERIA, false, &["needs-triage"]);
    s.fake("gh", &gh.script());
    (s, gh)
}

#[test]
fn plan_reads_sub_issues_labels_and_both_blocker_forms() {
    let (s, _gh) = github_spec();

    let out = s.run(&["plan", "10"]).ok();

    assert_eq!(out, "Spec 10: Login rework\n\nFrontier:\n  12  Build API\n");
    let calls = s.calls("gh");
    assert!(calls.iter().all(|c| c.starts_with("api ")), "{calls:?}");
}

#[test]
fn a_run_lands_the_ready_tickets_and_closes_ticks_and_comments_them() {
    let (s, gh) = github_spec();
    s.claude_lands();

    s.run(&["run", "10", "--branch", "int"]).ok();

    let merges = s.git(&["log", "--first-parent", "--merges", "--format=%s", "int"]);
    assert_eq!(merges.lines().count(), 2, "{merges}");
    for n in [12, 13] {
        assert!(gh.closed(n), "#{n} closed");
        assert_eq!(gh.body(n).matches("- [x] it works\r\n").count(), 1);
        assert_eq!(gh.comments(n).len(), 1, "#{n} has its landing comment");
    }
    assert!(!gh.closed(14), "#14 is not ready");
    assert!(gh.comments(14).is_empty());
    // Only `gh api` is used, so `gh` older than 2.94 works.
    let calls = s.calls("gh");
    assert!(calls.iter().all(|c| c.starts_with("api ")), "{calls:?}");
}

#[test]
fn a_github_location_must_be_owner_slash_repo() {
    let s = Scratch::new();
    s.repo_config("version = 1\n\n[tracker]\nkind = \"github\"\nlocation = \"acme\"\n");

    let err = s.run(&["plan", "10"]).fails();

    assert!(err.contains("owner/repo"), "{err}");
}
