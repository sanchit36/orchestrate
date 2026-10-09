//! A fake `gh` over fixture issues on disk, shared by the GitHub adapter's contract
//! tests (`src/tracker/github.rs`) and the black-box tests. Each issue is a directory
//! `<dir>/<number>/` holding `title.json` and `body.json` (JSON strings), `state`,
//! `labels.json`, and optional `subs` / `blocked_by` (issue numbers) and `comments`
//! (one posted JSON payload per line). The fake applies writes to these files.
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

const SCRIPT: &str = include_str!("gh.sh");

pub struct FakeGh {
    pub dir: PathBuf,
}

impl FakeGh {
    /// The fake's shell script body (no `#!` line), reading fixtures from `dir`.
    pub fn script(&self) -> String {
        format!("D='{}'\n{SCRIPT}", self.dir.display())
    }

    pub fn issue(&self, n: u64, title: &str, body: &str, closed: bool, labels: &[&str]) {
        let labels: Vec<_> = labels.iter().map(|l| json!({ "name": l })).collect();
        let state = if closed { "closed" } else { "open" };
        self.put(n, "title.json", &json!(title).to_string());
        self.put(n, "body.json", &json!(body).to_string());
        self.put(n, "state", state);
        self.put(n, "labels.json", &json!(labels).to_string());
    }

    pub fn sub_issues(&self, parent: u64, children: &[u64]) {
        self.put(parent, "subs", &numbers(children));
    }

    /// Native dependencies: `n` is blocked by `blockers`.
    pub fn blocked_by(&self, n: u64, blockers: &[u64]) {
        self.put(n, "blocked_by", &numbers(blockers));
    }

    pub fn body(&self, n: u64) -> String {
        let text = fs::read_to_string(self.file(n, "body.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    pub fn set_body(&self, n: u64, body: &str) {
        self.put(n, "body.json", &json!(body).to_string());
    }

    pub fn closed(&self, n: u64) -> bool {
        fs::read_to_string(self.file(n, "state")).unwrap().trim() == "closed"
    }

    /// The bodies of the comments posted on `n`.
    pub fn comments(&self, n: u64) -> Vec<String> {
        let text = fs::read_to_string(self.file(n, "comments")).unwrap_or_default();
        let body = |l: &str| {
            let posted: Value = serde_json::from_str(l).unwrap();
            posted["body"].as_str().unwrap().to_string()
        };
        text.lines().map(body).collect()
    }

    fn file(&self, n: u64, name: &str) -> PathBuf {
        self.dir.join(n.to_string()).join(name)
    }

    fn put(&self, n: u64, name: &str, content: &str) {
        fs::create_dir_all(self.dir.join(n.to_string())).unwrap();
        fs::write(self.file(n, name), content).unwrap();
    }
}

fn numbers(ns: &[u64]) -> String {
    ns.iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}
