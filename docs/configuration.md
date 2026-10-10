# Configuration

Orchestrate reads two TOML files.

- The **repo file**, `.orchestrate/config.toml` at the repo's top level, holds what the repo needs: tracker, gates, session settings.
  It is required, and you commit it.
- The **machine file**, `~/.config/orchestrate/config.toml`, holds your own preferences.
  It is optional.

`orchestrate setup` writes both (the machine file only when it does not exist yet).
You can also edit them by hand.

Both files start with `version = 1`.
Each key is valid in exactly one file: a key from the other file is an error that names the file it belongs in, and any other unknown key is an error too.
Keys inside tables (`[tracker]`, `[session]`, `[[gates]]`, `[tracker.commands]`) are checked the same way.

Durations are strings such as `"90s"`, `"20m"`, `"1h"` or the ISO 8601 form `"PT20M"`.

## Repo file

### `tracker`

Where the spec and its tickets live.
See [trackers.md](trackers.md) for what each kind reads and writes.

| Key | Default | Meaning |
| --- | --- | --- |
| `kind` | required | `"github"`, `"local-markdown"` or `"commands"`. |
| `location` | required | For `github`, the `owner/repo` the issues live in. For `local-markdown`, the folder of spec folders, relative to the top level. For `commands`, the project, board or URL the templates reach (informational). |
| `ready` | `"ready-for-agent"` | The ready marker: a label on GitHub, the `Status:` value for local markdown. A ticket without it never starts. |
| `retry_delay` | `"2s"` | The wait between the quick retries of a failed tracker operation (three tries in all). |
| `probe_backoff` | `"60s"` | When the tracker keeps failing the run pauses; this is the first wait between probes. It doubles after each failed probe. |
| `probe_backoff_max` | `"5m"` | The longest wait between probes of a paused run. |
| `timeout` | `"60s"` | How long one `gh` call or command template may run before its process group is killed and the operation fails. |
| `commands` | none | The `[tracker.commands]` templates. Required with `kind = "commands"` and refused with any other kind. |

### `tracker.commands`

The shell command templates of a tracker without a built-in adapter.
`setup` compiles them for you; the contract each one meets is in [trackers.md](trackers.md#command-templates).

| Key | Default | Meaning |
| --- | --- | --- |
| `get_spec` | required | Prints the spec. |
| `list_tickets` | required | Prints the spec's tickets. |
| `list_blockers` | required | Prints a ticket's blockers. |
| `mark_finished` | required | Records that a ticket landed. |
| `comment` | none | Posts a Markdown comment on a ticket. Without it, nothing is posted. |
| `tick` | none | Ticks met acceptance criteria. Without it, nothing is ticked. |
| `guide` | none | Set by `setup`: the tracker guide the templates were compiled from, relative to the top level. |
| `guide_hash` | none | Set by `setup`: the guide's `sha256:<hex>` at compile time. When the guide changes, `plan` and `run` warn you to recompile. |

### `criteria_headings`

Default `["Acceptance criteria", "Acceptance"]`.

The headings, at any level and ignoring case, whose `- [ ]` / `- [x]` items are a ticket's acceptance criteria.
The first matching heading in a ticket body wins.

### `gates`

Default: none.
`run` refuses to start without at least one gate.

Each `[[gates]]` entry is a shell command run with `sh -c` in the ticket's worktree after it is rebased onto the integration branch.
Gates run in the order listed; exit 0 passes, and the first failure stops the ticket from landing.

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | Names the gate in status and logs. |
| `run` | required | The command. |
| `paths` | `[]` (all files) | Globs against the ticket's changed files; the gate is skipped when none match. `*` stays within one path segment, `**` crosses segments. |
| `dirs` | `[]` | Folder globs (`*` per segment, no `**`). The gate runs once in each matching folder holding a changed file. |
| `cwd` | top level | A fixed working directory, relative to the top level. Not allowed together with `dirs`. |
| `timeout` | `"20m"` | How long one gate run may take before its process group is killed and the gate fails. |
| `teardown` | none | A command run after the gate in the same directory, even after a failure or timeout. Its exit status does not decide the gate. |

A ticket that changes no file matching a gate's `paths` skips that gate.
A gate without `paths` and without `dirs` always runs, even on a ticket whose commits change no file.
Nothing lands unchecked: a ticket on which every gate was skipped fails with `no gate matched its change`.

### `session`

How each ticket's Claude Code session is launched and prompted.

| Key | Default | Meaning |
| --- | --- | --- |
| `permission_mode` | `"bypassPermissions"` | Passed to `claude -p --permission-mode`. The default means sessions never stop to ask before running commands or editing files in their worktree. |
| `instructions` | none | A repo file, relative to the top level, appended to the built-in prompt under `# Repo instructions`. |
| `skills` | `[]` | Skill names the prompt points the session at. |
| `timeout` | `"45m"` | The wall-clock limit of a session; past it, the session's process group is killed. |

### `seed_files`

Default: `[]`.

Local files, relative to the top level, copied from your checkout into each fresh worktree (for example an untracked `.env`).
A listed file that does not exist fails that ticket's preparation.

### `prepare`

Default: none.

A shell command run in each fresh worktree before its session starts, such as `npm ci`.
Its output goes to the ticket's `<ticket>.prepare.log` in the run logs, and a non-zero exit fails the ticket.
It shares the session's `timeout`: past it, its process group is killed and the ticket fails.
Git submodules are initialised before it runs.

### `protected_branches`

Default: the remote `origin`'s default branch; without one (no `origin`, or an `origin` with no default branch), the branch checked out in your checkout when the run starts.

Branches a run never writes.
`run --branch` refuses a protected branch as its integration branch.

### Example

```toml
version = 1
criteria_headings = ["Acceptance criteria", "Acceptance"]
seed_files = [".env"]
prepare = "npm ci"
protected_branches = ["main", "release"]

[tracker]
kind = "github"
location = "acme/shop"
ready = "ready-for-agent"
retry_delay = "2s"
probe_backoff = "60s"
probe_backoff_max = "5m"
timeout = "60s"

[[gates]]
name = "test"
run = "npm test"
timeout = "20m"

[[gates]]
name = "lint"
run = "cargo clippy -- -D warnings"
paths = ["services/**/*.rs"]
dirs = ["services/*"]
teardown = "cargo clean -p scratch || true"

[session]
permission_mode = "bypassPermissions"
instructions = "docs/agents/session.md"
skills = ["tdd"]
timeout = "45m"
```

With a tracker that has no built-in adapter, the `[tracker]` table carries the templates:

```toml
version = 1

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

[[gates]]
name = "test"
run = "make test"
```

## Machine file

| Key | Default | Meaning |
| --- | --- | --- |
| `max_parallel` | `"auto"` | The default for `run --max-parallel`: `"auto"` (from the width of the ticket graph and the CPU cores) or a number above 0. |
| `notifications` | `true` | Desktop notifications for the events of a run. On macOS they come from the app you started the run in, such as Terminal or VS Code, and show only once you allow that app's notifications; macOS asks the first time and gives no error if you decline. |
| `state_dir` | `$XDG_STATE_HOME/orchestrate`, else `~/.local/state/orchestrate` | Where runs keep their state, logs and worktrees. An absolute path, or one starting with `~/` for your home folder. |

### Example

```toml
version = 1
max_parallel = 4
notifications = false
state_dir = "/Volumes/scratch/orchestrate"
```

## Other files

Orchestrate also keeps, without you editing them:

- `~/.config/orchestrate/approvals.toml`: the command templates you approved on this machine (see [trackers.md](trackers.md#approval)).
- One line, `/.orchestrate-result.json`, in your repo's `.git/info/exclude`, added once so a session's report is never committed.
  Git has no exclude file per worktree, so this is the only place orchestrate writes inside your repo.
- Under the state dir, one folder per repo (`<repo-folder>-<hash>`) holding `runs/<run-id>/state.json`, the logs in `runs/<run-id>/logs/` (`run.log` and one file per session, gate and preparation) and the worktrees in `worktrees/<run-id>/`.
