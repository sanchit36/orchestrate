mod common;

use common::Scratch;

/// A repo with a one-ticket local markdown spec `s1` and no config yet.
fn scratch() -> Scratch {
    let s = Scratch::new();
    s.spec("s1", "Spec one");
    s.ticket("s1", "t1", "Ticket one", "ready-for-agent", &[]);
    s
}

const TRACKER: &str = "[tracker]\nkind = \"local-markdown\"\nlocation = \"specs\"\n";

#[test]
fn plan_without_repo_config_refuses_and_points_to_setup() {
    let s = scratch();

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains("orchestrate setup"), "{err}");
}

#[test]
fn repo_config_without_version_names_the_file() {
    let s = scratch();
    s.repo_config(TRACKER);

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains(".orchestrate/config.toml"), "{err}");
    assert!(err.contains("version"), "{err}");
}

#[test]
fn repo_config_with_unknown_version_names_the_file() {
    let s = scratch();
    s.repo_config(&format!("version = 2\n{TRACKER}"));

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains(".orchestrate/config.toml"), "{err}");
    assert!(err.contains("version"), "{err}");
}

#[test]
fn machine_config_without_version_names_the_file() {
    let s = scratch();
    s.local_markdown_tracker();
    s.machine_config("notifications = false\n");

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains(".config/orchestrate/config.toml"), "{err}");
    assert!(err.contains("version"), "{err}");
}

#[test]
fn machine_config_with_unknown_version_names_the_file() {
    let s = scratch();
    s.local_markdown_tracker();
    s.machine_config("version = 9\n");

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains(".config/orchestrate/config.toml"), "{err}");
    assert!(err.contains("version"), "{err}");
}

#[test]
fn valid_machine_config_is_accepted() {
    let s = scratch();
    s.local_markdown_tracker();
    s.machine_config("version = 1\nnotifications = false\n");

    s.run(&["plan", "s1"]).ok();
}

#[test]
fn machine_key_in_repo_config_names_key_and_file() {
    let s = scratch();
    s.repo_config(&format!("version = 1\nnotifications = false\n{TRACKER}"));

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains("`notifications`"), "{err}");
    assert!(err.contains(".orchestrate/config.toml"), "{err}");
}

#[test]
fn repo_key_in_machine_config_names_key_and_file() {
    let s = scratch();
    s.local_markdown_tracker();
    s.machine_config(&format!("version = 1\n{TRACKER}"));

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains("`tracker`"), "{err}");
    assert!(err.contains(".config/orchestrate/config.toml"), "{err}");
}

#[test]
fn unknown_key_names_key_and_file() {
    let s = scratch();
    s.repo_config(&format!("version = 1\nbogus = 1\n{TRACKER}"));

    let err = s.run(&["plan", "s1"]).fails();

    assert!(err.contains("`bogus`"), "{err}");
    assert!(err.contains(".orchestrate/config.toml"), "{err}");
}

#[test]
fn config_is_found_at_the_git_top_level_from_a_subdirectory() {
    let s = scratch();
    s.local_markdown_tracker();
    std::fs::create_dir_all(s.repo.join("src/deep")).unwrap();

    let out = s.run_in(&s.repo.join("src/deep"), &["plan", "s1"]).ok();

    assert!(out.contains("t1  Ticket one"), "{out}");
}

#[test]
fn config_below_the_git_top_level_is_not_found() {
    let s = scratch();
    s.write(
        "sub/.orchestrate/config.toml",
        &format!("version = 1\n{TRACKER}"),
    );

    let err = s.run_in(&s.repo.join("sub"), &["plan", "s1"]).fails();

    assert!(err.contains("orchestrate setup"), "{err}");
}
