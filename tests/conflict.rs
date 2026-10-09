mod common;

use std::fs;
use std::path::PathBuf;

use common::Scratch;

/// Spec `auth` with independent tickets `a` and `b` that both write `shared.txt`, and
/// a gate recording the `shared.txt` it checks. `b` commits only after `a` has landed,
/// so its rebase conflicts. A conflict session (the worktree is mid-rebase) runs
/// `resolve`.
fn conflicting(resolve: &str) -> Scratch {
    let s = Scratch::new();
    let bin = s.bin.display().to_string();
    s.repo_config(&format!(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\n\
         [[gates]]\nname = \"check\"\nrun = 'cat shared.txt >> {bin}/gates'\n"
    ));
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.ticket("auth", "b", "Build API", "ready-for-agent", &[]);
    s.commit_all();
    s.fake(
        "claude",
        &format!(
            r#"slug=$(basename "$PWD")
cat > '{bin}/prompt.'"$slug"
if [ -d "$(git rev-parse --git-path rebase-merge)" ]; then
  echo "resolving $slug"
  {resolve}
  exit
fi
case "$slug" in
  a) until [ -e '{bin}/b.started' ]; do sleep 0.05; done;;
  b) touch '{bin}/b.started'
     until git log --format=%s int | grep -q 'Merge ticket a'; do sleep 0.05; done;;
esac
echo "$slug" > shared.txt
git add shared.txt && git commit -q -m "work $slug"
printf '{{"status":"done","acceptance_criteria":[],"notes":""}}' > .orchestrate-result.json"#
        ),
    );
    s
}

const RESOLVE: &str =
    "printf 'a\\nb\\n' > shared.txt; git add shared.txt; GIT_EDITOR=true git rebase --continue";

fn run(s: &Scratch) {
    s.run(&["run", "auth", "--branch", "int", "--max-parallel", "2"])
        .ok();
}

fn logs(s: &Scratch) -> PathBuf {
    let runs = s.repo_state(&s.state_root()).join("runs");
    runs.join(s.run_id()).join("logs")
}

fn worktree(s: &Scratch, slug: &str) -> PathBuf {
    let wts = s.repo_state(&s.state_root()).join("worktrees");
    wts.join(s.run_id()).join(slug)
}

#[test]
fn a_resolved_conflict_passes_the_gates_and_lands() {
    let s = conflicting(RESOLVE);

    run(&s);

    assert_eq!(s.git(&["show", "int:shared.txt"]), "a\nb\n");
    let log = fs::read_to_string(logs(&s).join("run.log")).unwrap();
    assert!(log.contains("ticket b finished: merged"), "{log}");
    // The gate checked the resolved file before `b` landed.
    let gates = fs::read_to_string(s.bin.join("gates")).unwrap();
    assert_eq!(gates, "a\na\nb\n");
    let conflict_log = fs::read_to_string(logs(&s).join("b.conflict.log")).unwrap();
    assert!(conflict_log.starts_with("resolving b\n"), "{conflict_log}");
    // Launched like any session: three sessions, the same `claude` args.
    let calls = s.calls("claude");
    assert_eq!(calls, vec!["-p --permission-mode bypassPermissions"; 3]);
    assert!(s.prompt("b").contains("conflict"), "{}", s.prompt("b"));
}

#[test]
fn a_failed_resolution_fails_the_ticket_with_cause_conflict_and_keeps_its_work() {
    let s = conflicting("exit 1");

    run(&s);

    let status = s.run(&["status"]).ok();
    let b = status.lines().find(|l| l.contains(" b ")).unwrap();
    assert!(b.contains("failed (conflict)"), "{status}");
    assert_eq!(
        fs::read_to_string(logs(&s).join("b.conflict.log")).unwrap(),
        "resolving b\n"
    );
    // Worktree and branch kept, with `b`'s own commit and no rebase left behind.
    let wt = worktree(&s, "b");
    assert_eq!(fs::read_to_string(wt.join("shared.txt")).unwrap(), "b\n");
    let branch = format!("orchestrate/{}/b", s.run_id());
    assert_eq!(s.git(&["show", &format!("{branch}:shared.txt")]), "b\n");
    let in_b = git_in(&wt, &["status", "--porcelain=v2", "--branch"]);
    assert!(!in_b.contains("rebase"), "{in_b}");
    // The integration branch has only `a`; its worktree went at run end.
    assert_eq!(s.git(&["show", "int:shared.txt"]), "a\n");
    assert!(!worktree(&s, "_integration").exists());
}

fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}
