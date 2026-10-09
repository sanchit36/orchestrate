//! GitHub tracker over `gh api` and gh's own login; orchestrate stores no credentials.
//! The spec is an issue (id = its number) and its native sub-issues are the tickets, in
//! sub-issue order. Closed is finished; ready is the configured label. Blockers are the
//! native dependencies plus the body forms (`#12`, `12`, issue URLs). Only REST calls
//! through `gh api` are made, so `gh` older than 2.94 (no `--parent` / `--blocked-by`)
//! works, and issue numbers are all those paths need: no database id is ever looked up.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Blocker, Spec, Ticket, Tracker};
use crate::{markdown, process};

pub struct GitHub {
    pub gh: PathBuf,
    /// How long one `gh` call may run.
    pub timeout: Duration,
    /// `owner/name`.
    pub repo: String,
    pub ready: String,
    pub criteria_headings: Vec<String>,
}

#[derive(Deserialize)]
struct Issue {
    number: u64,
    title: String,
    body: Option<String>,
    state: State,
    labels: Vec<Label>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum State {
    Open,
    Closed,
}

#[derive(Deserialize)]
struct Label {
    name: String,
}

/// The answer to posting a comment, parsed only to check it is one.
#[derive(Deserialize)]
struct Comment {
    #[expect(dead_code, reason = "parsed only to validate the answer")]
    id: u64,
}

impl GitHub {
    /// `gh api` on `repos/<repo>/issues/<path>`, parsed; `None` when GitHub answers 404.
    fn api<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        input: Option<Value>,
    ) -> Result<Option<T>> {
        let path = format!("repos/{}/issues/{path}", self.repo);
        let what = || format!("gh api -X {method} {path}");
        let mut cmd = Command::new(&self.gh);
        cmd.args(["api", "-X", method, &path]).stdin(Stdio::null());
        if input.is_some() {
            cmd.args(["--input", "-"]).stdin(Stdio::piped());
        }
        let input = input.map(|i| i.to_string()).unwrap_or_default();
        let out = process::output(&mut cmd, input.as_bytes(), self.timeout).with_context(what)?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("HTTP 404") {
                return Ok(None);
            }
            bail!("{}: {}", what(), err.trim());
        }
        let parsed = serde_json::from_slice(&out.stdout);
        parsed
            .map(Some)
            .with_context(|| format!("{}: unexpected answer", what()))
    }

    /// [`Self::api`] with a 404 as an error.
    fn need<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        input: Option<Value>,
    ) -> Result<T> {
        let found = self.api(method, path, input)?;
        found.with_context(|| format!("issue {path} not found in {}", self.repo))
    }

    fn ticket(&self, issue: Issue) -> Ticket {
        Ticket {
            id: issue.number.to_string(),
            title: issue.title,
            body: issue.body.unwrap_or_default(),
            finished: issue.state == State::Closed,
            ready: issue.labels.iter().any(|l| l.name == self.ready),
        }
    }
}

/// `id`, checked to be an issue number because it goes into the API path.
fn number(id: &str) -> Result<&str> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        bail!("`{id}` is not an issue number");
    }
    Ok(id)
}

/// The issue number a body reference names: `#12`, `12`, `owner/repo#12` or an issue URL.
/// shortcut: another repo's issue is looked up by number in this repo; fine until specs span repos.
fn reference(r: &str) -> Option<&str> {
    number(r.rsplit(['#', '/']).next()?).ok()
}

impl Tracker for GitHub {
    fn get_spec(&self, spec: &str) -> Result<Spec> {
        let issue: Issue = self.need("GET", number(spec)?, None)?;
        Ok(Spec {
            id: spec.to_string(),
            title: issue.title,
            body: issue.body.unwrap_or_default(),
        })
    }

    /// GitHub caps a parent at 100 sub-issues, so one page holds them all.
    fn list_tickets(&self, spec: &str) -> Result<Vec<Ticket>> {
        let path = format!("{}/sub_issues?per_page=100", number(spec)?);
        let issues: Vec<Issue> = self.need("GET", &path, None)?;
        Ok(issues.into_iter().map(|i| self.ticket(i)).collect())
    }

    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>> {
        let n = number(ticket)?;
        // shortcut: one page of 100 native blockers; page through if a ticket ever has more.
        let path = format!("{n}/dependencies/blocked_by?per_page=100");
        let native: Vec<Issue> = self.need("GET", &path, None)?;
        let closed = |i: Issue| Blocker {
            id: i.number.to_string(),
            finished: Some(i.state == State::Closed),
        };
        let mut blockers: Vec<Blocker> = native.into_iter().map(closed).collect();
        let issue: Issue = self.need("GET", n, None)?;
        for r in markdown::blocked_by(issue.body.as_deref().unwrap_or_default()) {
            let Some(id) = reference(&r) else {
                blockers.push(Blocker {
                    id: r,
                    finished: None,
                });
                continue;
            };
            if blockers.iter().any(|b| b.id == id) {
                continue;
            }
            blockers.push(match self.api::<Issue>("GET", id, None)? {
                Some(i) => closed(i),
                None => Blocker {
                    id: id.to_string(),
                    finished: None,
                },
            });
        }
        Ok(blockers)
    }

    fn mark_finished(&self, ticket: &str) -> Result<()> {
        let close = json!({ "state": "closed", "state_reason": "completed" });
        self.need::<Issue>("PATCH", number(ticket)?, Some(close))
            .map(drop)
    }

    fn comment(&self, ticket: &str, markdown: &str) -> Result<()> {
        let path = format!("{}/comments", number(ticket)?);
        self.need::<Comment>("POST", &path, Some(json!({ "body": markdown })))
            .map(drop)
    }

    /// Re-reads the body and writes it back only when a criterion was ticked.
    fn tick(&self, ticket: &str, criteria: &[String]) -> Result<Vec<String>> {
        let n = number(ticket)?;
        let body = self.need::<Issue>("GET", n, None)?.body.unwrap_or_default();
        let (text, missing) = markdown::tick(&body, &self.criteria_headings, criteria);
        if text != body {
            self.need::<Issue>("PATCH", n, Some(json!({ "body": text })))?;
        }
        Ok(missing)
    }
}

#[cfg(test)]
#[path = "../../tests/fake_gh/mod.rs"]
mod fake_gh;

#[cfg(test)]
mod contract {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::GitHub;
    use crate::tracker::Tracker;
    use crate::tracker::contract::{self, HEADINGS, Harness, SPEC_TITLE, Status, TICKETS, UNKNOWN};

    use super::fake_gh::FakeGh;

    const SPEC: u64 = 10;

    struct Hub {
        _dir: tempfile::TempDir,
        fake: FakeGh,
        tracker: GitHub,
    }

    impl Hub {
        fn number(&self, name: &str) -> u64 {
            self.id(name).parse().unwrap()
        }

        /// Replaces the fake `gh` with `script`.
        fn install(&self, script: &str) {
            fs::write(&self.tracker.gh, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(&self.tracker.gh, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    impl contract::Harness for Hub {
        /// Known blockers are native dependencies; [`UNKNOWN`] is a body reference to an
        /// issue the fake 404s. Bodies use GitHub's CRLF line ends.
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let fake = FakeGh {
                dir: dir.path().join("issues"),
            };
            let tracker = GitHub {
                gh: dir.path().join("gh"),
                timeout: std::time::Duration::from_secs(60),
                repo: "acme/app".into(),
                ready: "ready-for-agent".into(),
                criteria_headings: HEADINGS.iter().map(|h| h.to_string()).collect(),
            };
            let h = Hub {
                _dir: dir,
                fake,
                tracker,
            };
            h.install(&h.fake.script());
            h.fake.issue(SPEC, SPEC_TITLE, "The spec body.", false, &[]);
            let subs: Vec<_> = TICKETS.iter().map(|t| h.number(t.name)).collect();
            h.fake.sub_issues(SPEC, &subs);
            for t in TICKETS {
                let unknown = t.blocked_by.iter().filter(|&&b| b == UNKNOWN);
                let refs: Vec<_> = unknown.map(|b| format!("#{}", h.id(b))).collect();
                let body = contract::body(t, &refs).replace('\n', "\r\n");
                let labels: &[&str] = match t.status {
                    Status::Ready => &["ready-for-agent"],
                    _ => &["needs-triage"],
                };
                let closed = t.status == Status::Finished;
                h.fake
                    .issue(h.number(t.name), t.title, &body, closed, labels);
                let native = t.blocked_by.iter().filter(|&&b| b != UNKNOWN);
                let native: Vec<_> = native.map(|b| h.number(b)).collect();
                h.fake.blocked_by(h.number(t.name), &native);
            }
            h
        }

        fn tracker(&self) -> &dyn Tracker {
            &self.tracker
        }

        fn spec_id(&self) -> String {
            SPEC.to_string()
        }

        fn id(&self, name: &str) -> String {
            match TICKETS.iter().position(|t| t.name == name) {
                Some(i) => (SPEC + 1 + i as u64).to_string(),
                None => "99".into(),
            }
        }

        fn edit_criterion(&self, name: &str, from: &str, to: &str) {
            let n = self.number(name);
            self.fake.set_body(n, &self.fake.body(n).replace(from, to));
        }

        fn has_comment(&self, name: &str, text: &str) -> bool {
            let comments = self.fake.comments(self.number(name));
            comments.iter().any(|c| c.contains(text))
        }

        fn fail(&self) {
            self.install("echo 'gh: Bad credentials (HTTP 401)' >&2; exit 1");
        }

        fn garble(&self) {
            self.install("echo not json");
        }
    }

    contract::suite!(Hub);

    #[test]
    fn tick_edits_only_matching_checkbox_lines() {
        let h = Hub::new();
        let n = h.number("02-form");
        let body = "Intro with - [ ] a stray box\r\n\r\n## Acceptance criteria\r\n\r\n\
                    - [ ] the form renders\r\n- [ ] bad passwords show an error\r\n\r\n\
                    ## Notes\n\n- [ ] the form renders\ntrailing  spaces  \t";
        h.fake.set_body(n, body);

        let missing = h.tracker.tick(&n.to_string(), &["the form renders".into()]);

        assert!(missing.unwrap().is_empty());
        let want = body.replacen(
            "- [ ] the form renders\r\n",
            "- [x] the form renders\r\n",
            1,
        );
        assert_eq!(h.fake.body(n), want);
    }

    #[test]
    fn ids_that_are_not_issue_numbers_are_refused_before_calling_gh() {
        let h = Hub::new();
        fs::remove_file(&h.tracker.gh).unwrap();
        let err = h.tracker.get_spec("../../x").err().unwrap();
        assert!(
            format!("{err:#}").contains("not an issue number"),
            "{err:#}"
        );
    }
}
