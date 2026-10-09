//! Command-template adapter: any tracker without a built-in adapter, reached through
//! the shell command templates in `[tracker.commands]` (ADR 0001). Each template runs
//! via `sh -c` with `{id}` replaced by the shell-quoted id; its stdout must meet the
//! adapter JSON schema below, or the operation fails.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use super::{Blocker, Spec, Ticket, Tracker, Unsupported, approvals};
use crate::{config, process};

pub struct Commands {
    pub templates: config::Commands,
    /// Where the templates run: the repo's top level or the integration worktree.
    pub dir: PathBuf,
    /// How long one template call may run.
    pub timeout: Duration,
}

// The adapter JSON schema. Unknown fields are allowed; ids are strings.
#[derive(Deserialize)]
struct SpecOut {
    title: String,
    body: String,
}

#[derive(Deserialize)]
struct TicketOut {
    id: String,
    title: String,
    body: String,
    finished: bool,
    ready: bool,
}

#[derive(Deserialize)]
struct BlockerOut {
    id: String,
    /// `null` when the blocker is not found on the tracker.
    finished: Option<bool>,
}

/// What `mark_finished` and `comment` print: any JSON object, such as `{}`.
type Answer = serde_json::Map<String, serde_json::Value>;

#[derive(Deserialize)]
struct TickOut {
    missing: Vec<String>,
}

impl Tracker for Commands {
    fn get_spec(&self, spec: &str) -> Result<Spec> {
        let out: SpecOut = self.call("get_spec", &self.templates.get_spec, spec, "")?;
        Ok(Spec {
            id: spec.into(),
            title: out.title,
            body: out.body,
        })
    }

    fn list_tickets(&self, spec: &str) -> Result<Vec<Ticket>> {
        let out: Vec<TicketOut> =
            self.call("list_tickets", &self.templates.list_tickets, spec, "")?;
        Ok(out
            .into_iter()
            .map(|t| Ticket {
                id: t.id,
                title: t.title,
                body: t.body,
                finished: t.finished,
                ready: t.ready,
            })
            .collect())
    }

    fn list_blockers(&self, ticket: &str) -> Result<Vec<Blocker>> {
        let out: Vec<BlockerOut> =
            self.call("list_blockers", &self.templates.list_blockers, ticket, "")?;
        Ok(out
            .into_iter()
            .map(|b| Blocker {
                id: b.id,
                finished: b.finished,
            })
            .collect())
    }

    fn mark_finished(&self, ticket: &str) -> Result<()> {
        let _: Answer = self.call("mark_finished", &self.templates.mark_finished, ticket, "")?;
        Ok(())
    }

    /// The Markdown goes to the template on stdin.
    fn comment(&self, ticket: &str, markdown: &str) -> Result<()> {
        let template = self.templates.comment.as_ref().ok_or(Unsupported)?;
        let _: Answer = self.call("comment", template, ticket, markdown)?;
        Ok(())
    }

    /// The criteria texts go to the template on stdin as a JSON array; the template
    /// re-reads the ticket, ticks the unticked criteria with those exact texts and
    /// prints `{"missing": [texts no criterion has]}`.
    fn tick(&self, ticket: &str, criteria: &[String]) -> Result<Vec<String>> {
        let template = self.templates.tick.as_ref().ok_or(Unsupported)?;
        let texts = serde_json::to_string(criteria)?;
        let out: TickOut = self.call("tick", template, ticket, &texts)?;
        Ok(out.missing)
    }
}

impl Commands {
    /// Run the `name` template for `id` with `stdin`, and parse its stdout.
    fn call<T: DeserializeOwned>(
        &self,
        name: &str,
        template: &str,
        id: &str,
        stdin: &str,
    ) -> Result<T> {
        let out = self
            .run(template, id, stdin)
            .with_context(|| format!("{name} template"))?;
        serde_json::from_slice(&out)
            .with_context(|| format!("{name} template printed output outside the schema"))
    }

    /// The template's stdout. Its process group is killed after `timeout`.
    fn run(&self, template: &str, id: &str, stdin: &str) -> Result<Vec<u8>> {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &render(template, id)])
            .current_dir(&self.dir)
            .stdin(Stdio::piped());
        let out = process::output(&mut cmd, stdin.as_bytes(), self.timeout)?;
        if !out.status.success() {
            let code = out.status.code().unwrap_or(-1);
            bail!(
                "exited {code}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out.stdout)
    }
}

/// `template` as `sh -c` runs it for `id`: `{id}` replaced by the single-quoted id.
pub fn render(template: &str, id: &str) -> String {
    template.replace("{id}", &quote(id))
}

/// `s` as one single-quoted shell word.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Refuses templates holding a literal secret, and templates whose [`approval_hash`]
/// is not in `approvals.toml`. Warns when the tracker guide changed since compiling.
pub fn check(templates: &config::Commands, top: &Path) -> Result<Vec<String>> {
    refuse_secrets(templates)?;
    let hash = approval_hash(templates);
    let path = approvals::path()?;
    if !approvals::approved(&hash)? {
        bail!(
            "the [tracker.commands] templates are not approved ({hash}); review them, then \
             approve them by adding `[[approved]]` with `hash = \"{hash}\"` to {}",
            path.display()
        );
    }
    let mut warnings = Vec::new();
    if let (Some(guide), Some(compiled)) = (&templates.guide, &templates.guide_hash) {
        let now = fs::read(top.join(guide)).ok().map(|b| sha256(&b));
        if now.as_ref() != Some(compiled) {
            warnings.push(format!(
                "tracker guide {} changed since the command templates were compiled from it; \
                 re-run `orchestrate setup` to recompile them",
                guide.display()
            ));
        }
    }
    Ok(warnings)
}

/// Refuses templates holding what looks like a literal secret.
pub fn refuse_secrets(templates: &config::Commands) -> Result<()> {
    for (name, template) in named(templates) {
        if literal_secret(template) {
            bail!(
                "the `{name}` template in [tracker.commands] holds what looks like a literal \
                 secret; reference an environment variable instead"
            );
        }
    }
    Ok(())
}

/// The templates that are set, by name, in a fixed order.
pub fn named(t: &config::Commands) -> Vec<(&'static str, &str)> {
    let required = [
        ("get_spec", &t.get_spec),
        ("list_tickets", &t.list_tickets),
        ("list_blockers", &t.list_blockers),
        ("mark_finished", &t.mark_finished),
    ];
    let optional = [("comment", &t.comment), ("tick", &t.tick)];
    let optional = optional
        .into_iter()
        .filter_map(|(n, t)| Some((n, t.as_deref()?)));
    required
        .into_iter()
        .map(|(n, t)| (n, t.as_str()))
        .chain(optional)
        .collect()
}

/// The approval key: a hash of the templates' content only (not the repo or path).
pub fn approval_hash(t: &config::Commands) -> String {
    let mut text = String::new();
    for (name, template) in named(t) {
        text.push_str(&format!("{name} {}\n{template}\n", template.len()));
    }
    sha256(text.as_bytes())
}

/// `sha256:<hex>`.
pub fn sha256(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// Well-known token prefixes; a word starting with one is a token.
const TOKEN_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "sk-",
    "AKIA",
    "ATATT",
];

/// Whether `template` seems to hold a credential rather than an environment variable:
/// a known token prefix, a long mixed letters-and-digits run, a password in a URL,
/// or a literal `Bearer`/`Basic` credential.
fn literal_secret(template: &str) -> bool {
    let word_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    for word in template.split(|c| !word_char(c)) {
        let before = &template[..word.as_ptr() as usize - template.as_ptr() as usize];
        let after_dollar = before.ends_with('$') || before.ends_with("${");
        if word.is_empty() || after_dollar {
            continue;
        }
        let prefixed = TOKEN_PREFIXES
            .iter()
            .any(|p| word.starts_with(p) && word.len() >= p.len() + 16);
        // Runs of letters and digits only, so UUIDs (12-char groups) pass.
        let random = word.split(|c: char| !c.is_ascii_alphanumeric()).any(|run| {
            run.len() >= 32
                && run.chars().any(|c| c.is_ascii_digit())
                && run.chars().any(|c| c.is_ascii_alphabetic())
        });
        if prefixed || random {
            return true;
        }
    }
    let url_password = template.split("://").skip(1).any(|rest| {
        let authority = rest.split(['/', ' ', '"', '\'']).next().unwrap_or("");
        authority
            .split_once('@')
            .and_then(|(user, _)| user.split_once(':'))
            .is_some_and(|(_, pass)| !pass.starts_with('$'))
    });
    let scheme = ["Bearer ", "Basic "].iter().any(|s| {
        template
            .split(s)
            .skip(1)
            .any(|rest| rest.starts_with(|c: char| c.is_ascii_alphanumeric()))
    });
    url_password || scheme
}

#[cfg(test)]
mod tests {
    use super::literal_secret;

    #[test]
    fn literal_secrets_are_found_and_environment_variables_pass() {
        for secret in [
            "curl -H 'Authorization: Bearer abc.def' x",
            "curl -u x https://me:hunter2@jira.example.com/rest",
            "jira --token ATATT3xFfGF0aBcDeFgHiJkLmNoP issue view {id}",
            "curl -H 'X-Key: 9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c' x",
        ] {
            assert!(literal_secret(secret), "{secret}");
        }
        for fine in [
            "curl -H \"Authorization: Bearer $JIRA_TOKEN\" https://$USER:$PASS@x/rest",
            "curl -H \"Authorization: Basic ${JIRA_BASIC}\" x",
            "curl https://api.atlassian.com/ex/jira/1324a887-45db-1bf4-1e99-ef0ff456d421/rest/api/3",
            "python3 /tmp/.tmpAbC123/command_tracker.py list-tickets {id}",
            "echo $ATATT3xFfGF0aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
        ] {
            assert!(!literal_secret(fine), "{fine}");
        }
    }
}

#[cfg(test)]
mod contract {
    use std::fs;
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::Commands;
    use crate::config;
    use crate::tracker::Tracker;
    use crate::tracker::contract::{self, Harness, SPEC_TITLE, Status, TICKETS};

    /// The fixture tracker's state lives in a JSON file the fixture script edits.
    struct Fixture {
        _dir: tempfile::TempDir,
        state: PathBuf,
        tracker: Commands,
    }

    impl Fixture {
        fn edit(&self, f: impl FnOnce(&mut Value)) {
            let mut state: Value = serde_json::from_slice(&fs::read(&self.state).unwrap()).unwrap();
            f(&mut state);
            fs::write(&self.state, state.to_string()).unwrap();
        }

        fn ticket(&self, name: &str) -> Value {
            let state: Value = serde_json::from_slice(&fs::read(&self.state).unwrap()).unwrap();
            let id = self.id(name);
            let tickets = state["tickets"].as_array().unwrap();
            tickets.iter().find(|t| t["id"] == id).unwrap().clone()
        }
    }

    impl Harness for Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let state = dir.path().join("state.json");
            let script = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/command_tracker.py"
            );
            let op = |op: &str| format!("python3 '{script}' '{}' {op} {{id}}", state.display());
            let templates = config::Commands {
                get_spec: op("get-spec"),
                list_tickets: op("list-tickets"),
                list_blockers: op("list-blockers"),
                mark_finished: op("mark-finished"),
                comment: Some(op("comment")),
                tick: Some(op("tick")),
                guide: None,
                guide_hash: None,
            };
            let tracker = Commands {
                templates,
                dir: dir.path().to_path_buf(),
                timeout: std::time::Duration::from_secs(60),
            };
            let h = Fixture {
                _dir: dir,
                state,
                tracker,
            };
            let tickets: Vec<_> = TICKETS
                .iter()
                .map(|t| {
                    json!({
                        "id": h.id(t.name),
                        "title": t.title,
                        "body": contract::body(t, &[]),
                        "finished": t.status == Status::Finished,
                        "ready": t.status == Status::Ready,
                        "blockers": t.blocked_by.iter().map(|b| h.id(b)).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let spec = json!({"id": h.spec_id(), "title": SPEC_TITLE, "body": "The spec."});
            fs::write(
                &h.state,
                json!({"spec": spec, "tickets": tickets}).to_string(),
            )
            .unwrap();
            h
        }

        fn tracker(&self) -> &dyn Tracker {
            &self.tracker
        }

        // A quote and a space, so rendering must shell-quote the id.
        fn spec_id(&self) -> String {
            "it's login".into()
        }

        fn id(&self, name: &str) -> String {
            format!("LOGIN {name}")
        }

        fn edit_criterion(&self, name: &str, from: &str, to: &str) {
            let id = self.id(name);
            self.edit(|s| {
                for t in s["tickets"].as_array_mut().unwrap() {
                    if t["id"] == id {
                        t["body"] = t["body"].as_str().unwrap().replace(from, to).into();
                    }
                }
            });
        }

        fn has_comment(&self, name: &str, text: &str) -> bool {
            let comments = self.ticket(name)["comments"].clone();
            comments
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c.as_str().unwrap().contains(text)))
        }

        fn fail(&self) {
            self.edit(|s| s["mode"] = "fail".into());
        }

        fn garble(&self) {
            self.edit(|s| s["mode"] = "garble".into());
        }
    }

    contract::suite!(Fixture);
}
