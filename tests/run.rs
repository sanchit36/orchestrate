mod common;

use common::Scratch;

/// A spec `auth` with ticket `b` blocked by ticket `a`, committed, and a passing gate.
fn chain() -> Scratch {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.ticket("auth", "b", "Build API", "ready-for-agent", &["a"]);
    s.commit_all();
    s.claude_lands();
    s
}

#[test]
fn a_two_ticket_chain_lands_both_tickets_in_order_as_no_ff_merges() {
    let s = chain();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    let merges = s.git(&["log", "--first-parent", "--merges", "--format=%s", "int"]);
    let merges: Vec<_> = merges.lines().rev().collect();
    assert_eq!(merges.len(), 2, "{merges:?}");
    assert!(merges[0].contains("Add schema"), "{merges:?}");
    assert!(merges[1].contains("Build API"), "{merges:?}");
    // b was built on top of a's landed work.
    s.git(&["cat-file", "-e", "int:a.txt"]);
    s.git(&["cat-file", "-e", "int:b.txt"]);
    assert_eq!(s.calls("claude").len(), 2);
}

#[test]
fn run_refuses_with_no_gate_configured() {
    let s = Scratch::new();
    s.local_markdown_tracker();
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s.claude_lands();

    let err = s.run(&["run", "auth", "--branch", "int"]).fails();

    assert!(err.contains("gate"), "{err}");
    assert!(s.calls("claude").is_empty());
    assert!(!s.state_root().exists());
}

#[test]
fn the_integration_branch_is_created_from_head_with_its_worktree_under_the_run() {
    let s = chain();
    let head = s.git(&["rev-parse", "HEAD"]);

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(s.git(&["merge-base", "int", "main"]), head);
    let state = s.repo_state(&s.state_root());
    let wt = state
        .join("worktrees")
        .join(s.run_id())
        .join("_integration");
    // Removed at run end; the branch keeps every merge.
    assert!(!wt.exists());
    let list = s.git(&["worktree", "list", "--porcelain"]);
    assert!(!list.contains(&wt.display().to_string()), "{list}");
    assert_eq!(s.git(&["rev-list", "--merges", "--count", "int"]), "2\n");
}

#[test]
fn a_ticket_works_in_its_own_worktree_and_branch_under_the_state_dir() {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("false");
    s.spec("auth", "Login rework");
    s.ticket(
        "auth",
        "01 Add Schema!",
        "Add schema",
        "ready-for-agent",
        &[],
    );
    s.commit_all();
    s.claude_lands();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    // The gate failed, so the worktree and branch are kept for inspection.
    let run_id = s.run_id();
    let wt = s
        .repo_state(&s.state_root())
        .join("worktrees")
        .join(&run_id)
        .join("01-add-schema");
    assert!(wt.join("01-add-schema.txt").is_file());
    let branch = format!("orchestrate/{run_id}/01-add-schema");
    s.git(&["rev-parse", "--verify", &format!("refs/heads/{branch}")]);
    // The report sits at the worktree root and is git-excluded.
    assert!(wt.join(".orchestrate-result.json").is_file());
    assert_eq!(
        s.git(&["-C", wt.to_str().unwrap(), "status", "--porcelain"]),
        ""
    );
    // Nothing in or beside the working tree; nothing checked out in it.
    assert_eq!(s.git(&["status", "--porcelain"]), "");
    assert_eq!(s.git(&["symbolic-ref", "HEAD"]).trim(), "refs/heads/main");
    let mut beside: Vec<_> = std::fs::read_dir(s.repo.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    beside.sort();
    assert_eq!(beside, ["bin", "home", "repo"]);
}

#[test]
fn the_run_id_is_the_spec_slug_and_local_time_and_state_keeps_the_real_spec_id() {
    let s = Scratch::new();
    s.local_markdown_tracker_with_gate("true");
    s.spec("Auth  Rework_v2!", "Login rework");
    s.ticket(
        "Auth  Rework_v2!",
        "a",
        "Add schema",
        "ready-for-agent",
        &[],
    );
    s.commit_all();
    s.claude_lands();

    s.run(&["run", "Auth  Rework_v2!", "--branch", "int"]).ok();

    let id = s.run_id();
    let stamp = id.strip_prefix("auth-rework-v2-").expect(&id);
    let (date, time) = stamp.split_once('-').expect(stamp);
    assert_eq!((date.len(), time.len()), (8, 6), "{id}");
    assert!(
        stamp.replace('-', "").chars().all(|c| c.is_ascii_digit()),
        "{id}"
    );
    let state = s.repo_state(&s.state_root()).join("runs").join(&id);
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["version"], 1);
    assert_eq!(state["spec"], "Auth  Rework_v2!");
}

#[test]
fn slugs_are_capped_at_40_characters() {
    let s = Scratch::new();
    let spec = "a".repeat(39) + "-bcd";
    s.local_markdown_tracker_with_gate("true");
    s.spec(&spec, "Long");
    s.ticket(&spec, "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s.claude_lands();

    s.run(&["run", &spec, "--branch", "int"]).ok();

    // 39 a's then `-` hits the cap; the trailing `-` is trimmed.
    let prefix = format!("{}-", "a".repeat(39));
    let stamp = s.run_id().strip_prefix(&prefix).unwrap().to_string();
    assert_eq!(stamp.len(), "YYYYMMDD-HHMMSS".len(), "{stamp}");
}

#[test]
fn the_session_gets_the_built_in_prompt_on_stdin_with_bypassed_permissions() {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"true\"\n\n\
         [session]\ninstructions = \"docs/orchestrate.md\"\nskills = [\"tdd\", \"diagnose\"]\n",
    );
    s.write("docs/orchestrate.md", "Always run the linter.\n");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s.claude_lands();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(
        s.calls("claude"),
        ["-p --permission-mode bypassPermissions"]
    );
    let prompt = s.prompt("a");
    assert!(prompt.contains("The spec body."), "{prompt}");
    assert!(prompt.contains("## Acceptance criteria"), "{prompt}");
    assert!(
        prompt.contains(&format!("orchestrate/{}/a", s.run_id())),
        "{prompt}"
    );
    assert!(prompt.contains(".orchestrate-result.json"), "{prompt}");
    assert!(prompt.contains("\"status\": \"done | stuck\""), "{prompt}");
    assert!(prompt.contains("AGENTS.md / CLAUDE.md"), "{prompt}");
    assert!(prompt.contains("tdd, diagnose"), "{prompt}");
    assert!(
        prompt.trim_end().ends_with("Always run the linter."),
        "{prompt}"
    );
}

#[test]
fn the_permission_mode_is_a_repo_setting() {
    let s = Scratch::new();
    s.repo_config(
        "version = 1\n\n[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"true\"\n\n\
         [session]\npermission_mode = \"acceptEdits\"\n",
    );
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s.claude_lands();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(s.calls("claude"), ["-p --permission-mode acceptEdits"]);
}

#[test]
fn status_done_is_committed_on_the_integration_branch_not_in_the_users_checkout() {
    let s = chain();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    for t in ["a", "b"] {
        let landed = s.git(&["show", &format!("int:specs/auth/{t}.md")]);
        assert!(landed.contains("Status: done\n"), "{landed}");
        let mine = std::fs::read_to_string(s.repo.join(format!("specs/auth/{t}.md"))).unwrap();
        assert!(mine.contains("Status: ready-for-agent\n"), "{mine}");
    }
    assert_eq!(s.git(&["status", "--porcelain"]), "");
    let log = s.git(&["log", "--format=%s", "-1", "int"]);
    assert!(log.contains("b"), "{log}");
}

#[test]
fn landed_ticket_worktrees_branches_and_the_integration_worktree_are_removed() {
    let s = chain();

    s.run(&["run", "auth", "--branch", "int"]).ok();

    assert_eq!(s.git(&["branch", "--list", "orchestrate/*"]), "");
    let worktrees = s
        .repo_state(&s.state_root())
        .join("worktrees")
        .join(s.run_id());
    let left: Vec<_> = std::fs::read_dir(worktrees)
        .into_iter()
        .flatten()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn the_run_end_counts_only_tickets_the_run_finished() {
    let s = chain();
    s.ticket("auth", "e", "Done before", "done", &[]);
    s.commit_all();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(
        out.contains("ended: 2 finished, 0 failed, 0 stuck"),
        "{out}"
    );
}

#[test]
fn a_run_whose_integration_worktree_cannot_be_removed_still_ends_with_its_summary() {
    let s = Scratch::new();
    // A locked worktree refuses `git worktree remove --force`.
    s.local_markdown_tracker_with_gate("git worktree lock ../_integration || true");
    s.spec("auth", "Login rework");
    s.ticket("auth", "a", "Add schema", "ready-for-agent", &[]);
    s.commit_all();
    s.claude_lands();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    assert!(
        out.contains("could not remove the integration worktree"),
        "{out}"
    );
    assert!(out.contains("ended: 1 finished"), "{out}");
    assert!(out.contains("git push -u origin int"), "{out}");
}

#[test]
fn the_foreground_and_run_log_hold_the_same_timestamped_event_lines() {
    let s = chain();

    let out = s.run(&["run", "auth", "--branch", "int"]).ok();

    let logs = s
        .repo_state(&s.state_root())
        .join("runs")
        .join(s.run_id())
        .join("logs");
    assert_eq!(std::fs::read_to_string(logs.join("run.log")).unwrap(), out);
    for line in out.lines() {
        let (date, rest) = line.split_once(' ').unwrap();
        assert_eq!(date.len(), "YYYY-MM-DD".len(), "{line}");
        assert_eq!(rest.as_bytes()[8], b' ', "{line}");
    }
    assert!(out.contains("ticket a finished"), "{out}");
    assert!(out.contains("ticket b finished"), "{out}");
    assert!(out.contains("git push -u origin int"), "{out}");
    let session = std::fs::read_to_string(logs.join("a.session.log")).unwrap();
    assert_eq!(session, "session output\n");
}
