# Trackers

A tracker is where a repo's specs and tickets live.
Orchestrate reaches it through an adapter: code for the built-in kinds, shell command templates for anything else.
Only orchestrate reads and writes the tracker; sessions never touch it, and no agent does at run time.

Pick the adapter with `[tracker] kind` in the repo config (see [configuration.md](configuration.md#tracker)).

## What orchestrate needs from a tracker

Every adapter answers the same operations.

| Operation | Required | What it does |
| --- | --- | --- |
| `get_spec` | yes | The spec's title and body. |
| `list_tickets` | yes | The spec's tickets in tracker order, each with an id, title, body, and whether it is finished and ready. |
| `list_blockers` | yes | The tickets blocking a ticket, each with whether it is finished, or "not found". |
| `mark_finished` | yes | Record that a ticket's work landed on the integration branch. |
| `comment` | no | Post a Markdown comment on a ticket. |
| `tick` | no | Tick a ticket's met acceptance criteria. |

Ids are opaque strings: orchestrate never parses them, only hands them back to the adapter.

When a ticket lands, orchestrate ticks its met criteria, comments, then marks it finished.
When a ticket fails or is stuck, it only comments.
Comments and ticks are cosmetic: a failure is recorded in the run log, and an adapter without them skips them.
Nothing posted carries gate output or local paths.

When an operation fails or answers outside the schema, it is retried (see `retry_delay`).
If it still fails, the run pauses: it starts no new sessions, running sessions carry on, and it probes the tracker until it answers again.

### Ticket bodies

A ticket body is Markdown, read the same way for every adapter.

- **Acceptance criteria** are the `- [ ]` / `- [x]` items directly under a heading named in `criteria_headings` (default `Acceptance criteria` or `Acceptance`, any level, any case).
  A ticket without them is planned with a warning.
- **Blockers** in the body are a `Blocked by:` line with comma-separated references, or the list items under a `## Blocked by` heading, or both.
  A Markdown link counts as its target.
  The built-in adapters add these to the tracker's native links; a command template should do the same.

A ticket with an empty body, or with a blocker that is not found on the tracker, is held back together with every ticket it blocks.

## GitHub

```toml
[tracker]
kind = "github"
location = "owner/repo"
```

It runs `gh api` with your existing `gh` login; orchestrate stores no credentials.
Any `gh` version with `gh api` works.

- The spec is an issue; its id is the issue number.
- The tickets are the spec's native sub-issues, in sub-issue order (GitHub allows up to 100).
- A closed ticket is finished.
  A ticket with the `ready` label is ready.
- Blockers are the native "blocked by" dependencies plus the body forms, written as `#12`, `12`, `owner/repo#12` or an issue URL.
  A reference to another repo is looked up by its number in `location`.
- `mark_finished` closes the issue as completed, `comment` posts an issue comment, and `tick` edits the issue body.

## Local markdown

```toml
[tracker]
kind = "local-markdown"
location = "specs"
```

The tracker is a folder in the repo:

```text
specs/
  login/
    spec.md          the spec "login"; its title is the first `# ` line
    01-schema.md     ticket "01-schema"
    02-form.md       ticket "02-form"
```

- Each folder under `location` holding a `spec.md` is a spec; its id is the folder name.
- Every other `.md` file in that folder is a ticket; its id is the file name without `.md`, and tracker order is file name order.
  Ticket ids must be unique across all specs.
- The ticket's title is its first `# ` line, else its id.
- The first `Status:` line decides the state: `Status: done` is finished, and `Status: <ready>` (by default `Status: ready-for-agent`) is ready.
- Blockers are the body forms only, as ticket ids or links to ticket files such as `[schema](01-schema.md)`.

A ticket file looks like this:

```markdown
# Build the login form

Status: ready-for-agent

Blocked by: 01-schema

## Acceptance criteria

- [ ] the form renders
- [ ] bad passwords show an error
```

During a run, writes go to the copy of the folder in the integration worktree and are committed there, so they reach your branch with the run's work, never your checkout.
`mark_finished` sets `Status: done`, `comment` appends a timestamped entry under an `## Orchestrate log` heading at the end of the file, and `tick` ticks criteria in place.

## Command templates

Any other tracker (Jira, Linear, an internal tool) is reached through shell command templates in `[tracker.commands]`.
`orchestrate setup` writes them: a headless Claude Code session compiles them from your tracker guide (`docs/agents/issue-tracker.md`) or, without one, from a short description you type, and you review them.

```toml
[tracker]
kind = "commands"
location = "https://acme.atlassian.net/browse/SHOP"

[tracker.commands]
get_spec = "jira-json spec {id}"
list_tickets = "jira-json tickets {id}"
list_blockers = "jira-json blockers {id}"
mark_finished = "jira-json finish {id}"
comment = "jira-json comment {id}"
tick = "jira-json tick {id}"
```

### Contract

- Each template runs as `sh -c '<template>'`.
  `{id}` is replaced by the spec or ticket id, single-quoted for the shell; write `{id}`, not `'{id}'`.
- It runs in the repo's top level for `plan` and `setup`, and in the run's integration worktree during a run.
- `comment` gets the Markdown comment on stdin.
  `tick` gets a JSON array of criterion texts on stdin.
  Other templates get empty stdin.
- It must exit 0 and print one JSON value on stdout, as in the schema below, within 60 seconds.
  On a non-zero exit (its stderr is shown), a timeout (its whole process group is killed) or output outside the schema, the operation fails.
- Never put a credential in a template: reference an environment variable, such as `$JIRA_TOKEN`.
  Orchestrate refuses templates holding what looks like a literal token, password or `Bearer` credential.

### JSON schema

Fields not listed are ignored.
Ids are JSON strings, even when the tracker uses numbers.

`get_spec {id}` prints the spec:

```json
{ "title": "Login", "body": "Markdown body" }
```

`list_tickets {id}` prints the spec's tickets in tracker order:

```json
[
  {
    "id": "SHOP-12",
    "title": "Build the login form",
    "body": "Markdown body with its acceptance criteria",
    "finished": false,
    "ready": true
  }
]
```

`list_blockers {id}` prints a ticket's blockers, with `finished` set to `null` for a blocker the tracker does not have:

```json
[
  { "id": "SHOP-11", "finished": true },
  { "id": "SHOP-99", "finished": null }
]
```

`mark_finished {id}` and `comment {id}` print any JSON object, such as `{}`.

`tick {id}` re-reads the ticket, ticks each unticked criterion whose text is exactly one of the texts on stdin, and prints the texts no criterion has any more (edited since they were read).
An already ticked criterion counts as found.

```json
{ "missing": ["bad passwords show an error"] }
```

### Approval

Templates run commands on your machine, so each machine approves them once before `plan` or `run` uses them.
`setup` shows them and asks; approving appends their hash to `~/.config/orchestrate/approvals.toml`:

```toml
[[approved]]
hash = "sha256:..."
```

The hash covers the templates' text only, so editing any template needs a new approval, and a teammate pulling the same repo config approves it once on their machine.
`plan` and `run` refuse unapproved templates and print the hash to approve.

When the tracker guide changes after the templates were compiled from it, `plan` and `run` warn you to re-run `orchestrate setup` to recompile them.
