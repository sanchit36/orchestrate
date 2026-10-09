mod common;
mod fake_gh;

use std::fs;

use common::Scratch;
use fake_gh::FakeGh;

const GUIDE: &str = "# Issue tracker: GitHub\n\nIssues live as GitHub issues. Use `gh`.\n";
const LABELS: &str = "# Triage Labels\n\n\
| Label in mattpocock/skills | Label in our tracker | Meaning |\n\
| -------------------------- | -------------------- | ------- |\n\
| `needs-triage`             | `triage`             | Evaluate |\n\
| `ready-for-agent`          | `agent-go`           | Ready for an agent |\n";

/// Spec issue #10 on `acme/app` with ready sub-issue #12, served by a fake `gh`.
fn github_spec(s: &Scratch) -> FakeGh {
    let gh = FakeGh {
        dir: s.home.join("issues"),
    };
    gh.issue(10, "Login rework", "The spec body.", false, &[]);
    gh.sub_issues(10, &[12]);
    let criteria = "## Acceptance criteria\n\n- [ ] it works\n";
    gh.issue(12, "Build API", criteria, false, &["agent-go"]);
    s.fake("gh", &gh.script());
    gh
}

#[test]
fn reads_the_tracker_guide_and_triage_labels_and_never_writes_them_or_the_tracker() {
    let s = Scratch::new();
    s.write("docs/agents/issue-tracker.md", GUIDE);
    s.write("docs/agents/triage-labels.md", LABELS);
    s.git(&["remote", "add", "origin", "git@github.com:acme/app.git"]);
    let gh = github_spec(&s);

    let out = s
        .run_with_input(&["setup"], "\n\n\n\ncheck: true\n\n\n\n10\n")
        .ok();

    assert!(
        out.contains("Tracker kind (github, local-markdown or commands) [github]"),
        "{out}"
    );
    assert!(out.contains("[acme/app]"), "{out}");
    assert!(out.contains("Ready marker [agent-go]"), "{out}");
    assert!(out.contains("Frontier:\n  12  Build API\n"), "{out}");
    assert!(
        out.contains("gh api -X PATCH repos/acme/app/issues/12"),
        "{out}"
    );
    let config = repo_config(&s);
    assert_eq!(config["tracker"]["kind"].as_str(), Some("github"));
    assert_eq!(config["tracker"]["location"].as_str(), Some("acme/app"));
    assert_eq!(config["tracker"]["ready"].as_str(), Some("agent-go"));
    let guide = fs::read_to_string(s.repo.join("docs/agents/issue-tracker.md")).unwrap();
    let labels = fs::read_to_string(s.repo.join("docs/agents/triage-labels.md")).unwrap();
    assert_eq!((guide.as_str(), labels.as_str()), (GUIDE, LABELS));
    assert!(!gh.closed(12), "verification never marks a ticket finished");
    let calls = s.calls("gh");
    assert!(!calls.is_empty(), "the fake gh recorded no call");
    let writes = |c: &&String| c.contains("-X PATCH") || c.contains("-X POST");
    assert!(!calls.iter().any(|c| writes(&c)), "{calls:?}");
}

#[test]
fn without_a_guide_the_kind_and_location_come_from_the_git_remote() {
    let s = Scratch::new();
    s.git(&["remote", "add", "origin", "https://github.com/acme/app.git"]);
    github_spec(&s);

    let out = s
        .run_with_input(&["setup"], "\n\nagent-go\n\ncheck: true\n\n\n\n10\n")
        .ok();

    assert!(
        out.contains("Tracker kind (github, local-markdown or commands) [github]"),
        "{out}"
    );
    assert_eq!(
        repo_config(&s)["tracker"]["location"].as_str(),
        Some("acme/app")
    );
    assert!(
        !s.repo.join("docs/agents").exists(),
        "setup never creates the guide"
    );
}

/// A local markdown spec `auth` under `specs/` with one ready ticket `02-api`.
fn local_spec(s: &Scratch) {
    s.spec("auth", "Login rework");
    s.ticket("auth", "01-schema", "Add schema", "done", &[]);
    s.ticket(
        "auth",
        "02-api",
        "Build API",
        "ready-for-agent",
        &["01-schema"],
    );
}

fn repo_config(s: &Scratch) -> toml::Table {
    let text = fs::read_to_string(s.repo.join(".orchestrate/config.toml")).unwrap();
    text.parse().unwrap()
}

#[test]
fn defaults_verify_a_local_markdown_spec_and_write_a_loadable_config() {
    let s = Scratch::new();
    local_spec(&s);

    // Every answer the default, then the spec to verify on.
    let out = s
        .run_with_input(&["setup"], "\n\n\n\ncheck: true\n\n\n\nauth\n")
        .ok();

    assert!(
        out.contains("Spec auth: Login rework\n\nFrontier:\n  02-api  Build API\n"),
        "{out}"
    );
    assert!(
        out.contains("`Status: done` in specs/auth/02-api.md"),
        "shows mark finished on a frontier ticket: {out}"
    );
    let ticket = fs::read_to_string(s.repo.join("specs/auth/02-api.md")).unwrap();
    assert!(
        ticket.contains("Status: ready-for-agent"),
        "not marked: {ticket}"
    );
    let config = repo_config(&s);
    assert_eq!(config["tracker"]["kind"].as_str(), Some("local-markdown"));
    assert_eq!(config["tracker"]["location"].as_str(), Some("specs"));
    let plan = s.run(&["plan", "auth"]).ok();
    assert!(plan.contains("02-api  Build API"), "{plan}");
}

#[test]
fn proposes_detected_gates_to_keep_edit_or_drop_and_takes_hand_entered_ones() {
    let s = Scratch::new();
    local_spec(&s);
    s.write(
        "package.json",
        r#"{"scripts": {"typecheck": "tsc", "lint": "eslint .", "test": "vitest", "build": "vite"}}"#,
    );
    s.write("Makefile", "build:\n\ttrue\ntest:\n\ttrue\n");
    s.write("Cargo.toml", "");
    s.write("go.mod", "");
    s.write("pytest.ini", "");

    // Keep typecheck, drop lint, edit test, keep the rest, add a smoke gate.
    let answers =
        "\n\n\n\n\n-\nnpm test -- --run\n\n\n\n\nsmoke: docker compose up --wait\n\n\n\nauth\n";
    let out = s.run_with_input(&["setup"], answers).ok();

    assert!(
        out.contains("Gate `npm-typecheck` (- drops it) [npm run typecheck]"),
        "{out}"
    );
    let config = repo_config(&s);
    let gates: Vec<(&str, &str)> = config["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["name"].as_str().unwrap(), g["run"].as_str().unwrap()))
        .collect();
    assert_eq!(
        gates,
        [
            ("npm-typecheck", "npm run typecheck"),
            ("npm-test", "npm test -- --run"),
            ("make-test", "make test"),
            ("cargo-test", "cargo test"),
            ("go-test", "go test ./..."),
            ("pytest", "pytest"),
            ("smoke", "docker compose up --wait"),
        ]
    );
}

#[test]
fn proposes_ignored_local_files_as_seed_files_and_states_the_permission_default() {
    let s = Scratch::new();
    local_spec(&s);
    s.write(
        ".gitignore",
        ".env\n.env.local\n.claude/settings.local.json\n",
    );
    for f in [
        ".env",
        ".env.local",
        ".env.example",
        ".claude/settings.local.json",
    ] {
        s.write(f, "x");
    }

    let out = s
        .run_with_input(&["setup"], "\n\n\n\ncheck: true\n\n\n\nauth\n")
        .ok();

    assert!(
        out.contains("[.env, .env.local, .claude/settings.local.json]"),
        "{out}"
    );
    assert!(out.contains("bypassPermissions"), "{out}");
    assert!(out.contains("repo setting"), "{out}");
    let config = repo_config(&s);
    let seeds = config["seed_files"].as_array().unwrap();
    assert_eq!(seeds.len(), 3, "{seeds:?}");
    assert_eq!(
        config["session"]["permission_mode"].as_str(),
        Some("bypassPermissions")
    );
}

#[test]
fn asks_machine_preferences_only_when_the_machine_config_is_missing() {
    let s = Scratch::new();
    local_spec(&s);
    let machine = s.home.join(".config/orchestrate/config.toml");
    fs::remove_file(&machine).unwrap();

    let out = s
        .run_with_input(&["setup"], "\n\n\n\ncheck: true\n\n\n\n3\nno\nauth\n")
        .ok();

    assert!(out.contains("Desktop notifications"), "{out}");
    let written = fs::read_to_string(&machine).unwrap();
    assert!(written.contains("max_parallel = 3"), "{written}");
    assert!(written.contains("notifications = false"), "{written}");

    let out = s.run_with_input(&["setup"], "\n\n\n\n\n\n\n\nauth\n").ok();
    assert!(!out.contains("Desktop notifications"), "{out}");
}

#[test]
fn a_failed_verification_writes_nothing() {
    let s = Scratch::new();
    local_spec(&s);

    let err = s
        .run_with_input(&["setup"], "\n\n\n\ncheck: true\n\n\n\nnope\n")
        .fails();

    assert!(err.contains("verification failed"), "{err}");
    assert!(!s.repo.join(".orchestrate").exists());
}

#[test]
fn skipping_verification_warns_and_saves() {
    let s = Scratch::new();

    let run = s.run_with_input(&["setup"], "\n\n\n\ncheck: true\n\n\n\nskip\n");

    run.ok();
    let err = run.stderr();
    assert!(err.contains("warning: verification skipped"), "{err}");
    let config = repo_config(&s);
    assert_eq!(config["tracker"]["kind"].as_str(), Some("local-markdown"));
}

#[test]
fn running_out_of_answers_fails_without_writing() {
    let s = Scratch::new();

    let err = s.run_with_input(&["setup"], "\n\n").fails();

    assert!(err.contains("no answer for"), "{err}");
    assert!(!s.repo.join(".orchestrate").exists());
}

#[test]
fn setup_asks_until_there_is_a_gate_and_never_saves_without_one() {
    let s = Scratch::new();
    local_spec(&s);
    s.write("Cargo.toml", "");

    // Drop the detected gate, then finish with none: setup asks again.
    let err = s.run_with_input(&["setup"], "\n\n\n\n-\n\n").fails();
    assert!(err.contains("no answer for"), "{err}");
    assert!(!s.repo.join(".orchestrate").exists());

    let out = s
        .run_with_input(&["setup"], "\n\n\n\n-\n\ncheck: true\n\n\n\nauth\n")
        .ok();

    assert!(out.contains("At least one gate is required"), "{out}");
    assert_eq!(repo_config(&s)["gates"][0]["run"].as_str(), Some("true"));
}

#[test]
fn re_running_shows_current_values_as_defaults_and_keeps_other_settings() {
    let s = Scratch::new();
    local_spec(&s);
    s.repo_config(
        "version = 1\nprepare = \"make deps\"\ncriteria_headings = [\"Done when\"]\n\n\
         [tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\nready = \"go\"\n\
         retry_delay = \"5s\"\n\n\
         [[gates]]\nname = \"check\"\nrun = \"make check\"\ntimeout = \"5m\"\n\n\
         [session]\npermission_mode = \"acceptEdits\"\ntimeout = \"1h\"\n",
    );
    s.ticket("auth", "02-api", "Build API", "go", &[]);
    s.write("Cargo.toml", "");

    // Change only the gate's command.
    let out = s
        .run_with_input(&["setup"], "\n\n\n\nmake ci\n\n\n\nauth\n")
        .ok();

    let shown = [
        "Ready marker [go]",
        "[Done when]",
        "Gate `check` (- drops it) [make check]",
        "[acceptEdits]",
    ];
    for shown in shown {
        assert!(out.contains(shown), "{shown}: {out}");
    }
    assert!(
        !out.contains("cargo-test"),
        "current gates replace detection"
    );
    let config = repo_config(&s);
    assert_eq!(config["prepare"].as_str(), Some("make deps"));
    assert_eq!(config["tracker"]["retry_delay"].as_str(), Some("5s"));
    assert_eq!(config["gates"][0]["run"].as_str(), Some("make ci"));
    assert_eq!(config["gates"][0]["timeout"].as_str(), Some("5m"));
    assert_eq!(config["session"]["timeout"].as_str(), Some("1h"));
    let mode = config["session"]["permission_mode"].as_str();
    assert_eq!(mode, Some("acceptEdits"));
}
