# orchestrate

Orchestrate takes a spec from your tracker and works its tickets as unattended Claude Code sessions, landing the gated work on one integration branch for you to review.

You write a spec and break it into tickets with acceptance criteria and blockers.
Orchestrate starts a session for every ready ticket whose blockers have finished, each in its own git worktree.
When a session ends, your gates (tests, lint, anything with an exit code) run on its work rebased onto the integration branch.
Work that passes is merged, the ticket is marked finished on the tracker, and the tickets it unblocked start next.
Work that fails or that the session reports as stuck stays in its worktree for you to look at.

## Requirements

- macOS or Linux.
- `git`.
- The [Claude Code](https://docs.anthropic.com/en/docs/claude-code) CLI, `claude`, logged in.
- The [GitHub CLI](https://cli.github.com), `gh`, logged in, for the GitHub tracker only.

## Install

With the shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/sanchit36/orchestrate/releases/latest/download/orchestrate-installer.sh | sh
```

With Homebrew:

```sh
brew install sanchit36/tap/orchestrate
```

With Cargo:

```sh
cargo install orchestrate
```

Prebuilt binaries for each release are on the [releases page](https://github.com/sanchit36/orchestrate/releases).

## Quickstart

Run everything from inside your repo.

1. Configure the repo.

   ```sh
   orchestrate setup
   ```

   Setup detects your tracker, proposes gates from your build files, and asks you to confirm each answer (Enter takes the default in brackets).
   It keeps asking for a gate until you have at least one.
   It checks the tracker on one real spec, then writes `.orchestrate/config.toml`.
   Commit that file.

2. See what would start.

   ```sh
   orchestrate plan 15
   ```

   `plan` reads spec `15` and shows its frontier: the open, ready tickets whose blockers have all finished.
   It also warns about tickets without acceptance criteria and holds back tickets it cannot trust, such as one blocked by a ticket that does not exist.

3. Run it.

   ```sh
   orchestrate run 15 --branch spec-15
   ```

   The run creates `spec-15` from `HEAD` (or reuses it) and works the spec until nothing more can start.
   It prints one line per event and ends with a summary and the `git push -u origin spec-15` command; orchestrate never pushes.
   Add `--detach` to keep it running after you close the terminal, and `--max-parallel <n>` to cap the sessions at once.

4. Watch it.

   ```sh
   orchestrate status          # where the run is
   orchestrate status 02-form  # one ticket: criteria, notes, gates, worktree and logs
   orchestrate runs            # this repo's runs
   ```

   `orchestrate stop` stops the live run, and `orchestrate resume` continues a stopped or crashed one.

5. Clean up.

   ```sh
   orchestrate clean
   ```

   Landed tickets' worktrees are removed as they land.
   `clean` removes what the run left behind, but keeps failed and stuck tickets' worktrees as evidence unless you pass `--force`.
   See `orchestrate clean --help` for `--all`, `--purge` and `--orphans`.

Every command has `--help`.

## Writing a spec

A spec is one piece of work, and its tickets are the steps, each sized for one session.
Give every ticket an `## Acceptance criteria` checklist: the session is asked to meet each item and report on it.
A ticket starts only when it carries the ready marker (by default `ready-for-agent`) and every ticket it is blocked by has finished.

On GitHub, the spec is an issue and its tickets are its native sub-issues.
The ready marker is a label, a closed sub-issue is finished, and blockers are the native "blocked by" dependencies or a `Blocked by: #12` line in the body.

With local markdown, the spec is a folder holding a `spec.md`, and every other `.md` file in it is a ticket:

```markdown
# Build the login form

Status: ready-for-agent

Blocked by: 01-schema

## Acceptance criteria

- [ ] the form renders
- [ ] bad passwords show an error
```

See [Trackers](docs/trackers.md) for every rule and for other trackers.

## How a run works

1. The run takes the repo's lock, creates or reuses the integration branch, and reads the spec and its tickets from the tracker.
2. It starts a session for each ticket on the frontier, up to `--max-parallel` at once.
   Each session is `claude -p` in a fresh worktree on its own branch, cut from the integration branch, with a prompt holding the spec, the ticket and its acceptance criteria.
3. The session commits its work and writes a report: done or stuck, and a verdict with evidence for each criterion.
4. A ticket reported done with new commits is rebased onto the integration branch.
   If the rebase conflicts, a separate session resolves it.
5. Every gate runs on the rebased work.
   If all pass, the ticket is merged into the integration branch, its criteria are ticked, a comment is posted, and it is marked finished on the tracker.
6. The tickets it unblocked join the frontier and start next.
   This repeats until nothing more can start.
7. The run prints a summary and the `git push` command for the integration branch.

Only orchestrate talks to the tracker, never a session.
If the tracker cannot be read or written, the run pauses: running sessions carry on, no new ones start, and it resumes by itself once the tracker answers.

### Ticket outcomes

| Outcome | Meaning | What happens next |
| --- | --- | --- |
| finished | Its work passed every gate and merged. | Its worktree is removed, and the tickets it blocked can start. |
| failed | Its worktree could not be prepared, or the session exited non-zero, made no commits, left changes uncommitted, failed a gate, or could not be rebased. | Its worktree is kept, a comment is posted, and the tickets it blocks are skipped. |
| timeout | The session ran past `[session] timeout` (default 45 minutes). | Treated like failed. |
| stuck | The session reported it cannot go on, or reported a criterion unmet. | Its worktree is kept, a comment is posted, and the tickets it blocks are held. |
| held | Blocked by a stuck ticket. | Not started in this run. |
| skipped | Blocked by a failed ticket. | Not started in this run. |

To retry a failed or stuck ticket, fix what it needs (the ticket text, a gate, or the code on the integration branch) and start the spec again with `run`: tickets already finished on the tracker are not worked again.

## Commands

Run every command from inside your repo.
Commands that take `--run` accept a run id, a unique prefix of one, or a spec id.

### `orchestrate setup`

Configures the repo and writes `.orchestrate/config.toml`.
It asks for the tracker kind and location, the ready marker, the acceptance criteria headings, the gates, any seed files and the session permission mode.
Each question shows a default in brackets that Enter accepts.
On a machine's first setup it also asks how many sessions to run at once and whether to show desktop notifications, and writes the machine config.
Last, it asks for one spec id and checks the tracker on it, printing the spec's frontier; nothing is written if the check fails.

Answers are read from stdin, so setup can also be scripted by piping one answer per line, in the order the questions are asked; an empty line takes the default.

### `orchestrate plan <spec>`

Shows what `run` would start now, without starting anything.
It lists the frontier and warns about tickets without acceptance criteria.
It also lists the tickets it holds back, such as one blocked by a ticket that does not exist, and why.

### `orchestrate run <spec> --branch <branch>`

Works the spec until nothing more can start, landing each ticket on `<branch>`.

| Option | Meaning |
| --- | --- |
| `--branch <branch>` | The integration branch, created from `HEAD` or reused if it exists. Required. A protected branch is refused. |
| `--max-parallel <n>` | Sessions at once: `auto` (from the width of the ticket graph and the CPU cores) or a number. Defaults to the machine config's `max_parallel`. |
| `--detach` | Keep running in the background after the terminal closes. Follow it with `status` and the run log. |

It prints one timestamped line per event, the same lines it appends to the run log.
It refuses to start without a gate, or while another run of the same repo is live.

### `orchestrate status [ticket]`

Without a ticket, shows the run: its spec, integration branch and state, a count per outcome, and every ticket with its outcome.
With a ticket id, shows that ticket's outcome and reason, each criterion with the session's verdict and evidence, the session's notes, each gate's result, its worktree and its log files.
It reads run state only and never touches the tracker.
`--run` picks the run; the default is the most recent.

### `orchestrate runs`

Lists this repo's runs, most recent first, with each run's spec and state.

### `orchestrate stop`

Stops the live run: kills its sessions and marks the run stopped, so `resume` can continue it later.
`--run` picks the run; the default is the live one.

### `orchestrate resume`

Continues a stopped or crashed run with the same integration branch.
A run that ended normally cannot be resumed; start a new one with `run`.
It checks each ticket against the integration branch and the tracker, keeps what already landed, and restarts the tickets that were left in flight.
`--run` picks the run; the default is the most recent.
`--detach` works as for `run`.

### `orchestrate clean`

Removes the worktrees and branches a run left behind.
It never touches an integration branch.

| Option | Meaning |
| --- | --- |
| `--run <run>` | The run to clean; the default is the most recent. |
| `--force` | Also remove failed and stuck tickets' worktrees and branches, which are otherwise kept as evidence. |
| `--all` | Clean every ended run of this repo. |
| `--purge` | Also delete the run's state and logs. Implies `--force`. |
| `--orphans` | Only remove the state of repos that are gone or moved, printing each. |

## Where runs keep their files

Each repo gets its own folder under the state directory (by default `~/.local/state/orchestrate`, see `state_dir`):

```text
<repo>-<hash>/
  runs/<run-id>/
    state.json                  the run's state, read by status and resume
    logs/
      run.log                   every event line
      <ticket>.session.log      the session's output
      <ticket>.gate-<name>.log  each gate's output
      <ticket>.prepare.log      the prepare command's output, if any
      <ticket>.conflict.log     the conflict-resolution session's output, if any
  worktrees/<run-id>/
    _integration/               the integration branch's worktree, where tickets are merged
    <ticket>/                   a ticket's worktree, until it lands or is cleaned
```

Your checkout is never changed: the run works only in these worktrees and on the integration branch.

## Safety model

Sessions run unattended, so orchestrate decides what they may do and what lands.

- **Sessions work in their own worktrees.**
  Each session runs `claude -p` with `--permission-mode bypassPermissions` (the default, set by `[session] permission_mode`), so it never stops to ask before running a command or editing a file.
  It starts in a fresh worktree on its own branch under orchestrate's state directory, never in your checkout, which orchestrate never changes.
  This is not an operating system sandbox: a session runs commands as you, so only run specs whose tickets you would let an agent work unattended.
- **Gates decide what lands.**
  A session's own report (done or stuck, a verdict on each criterion) is never trusted alone.
  Work lands only when every gate passes on it rebased onto the integration branch, and `run` refuses to start without a gate.
  Gates check exactly the commit that merges: a session that leaves changes uncommitted in its worktree fails its ticket.
- **Work lands on one integration branch.**
  Tickets are merged into the branch you name with `--branch`, for you to review and push.
  Protected branches (by default `origin`'s default branch, else your checked-out branch; see `protected_branches`) are refused as the integration branch, and `clean` never touches an integration branch.
- **Only ready tickets start.**
  A ticket starts only when its tracker carries the ready marker and its blockers have finished.
- **The tracker is reached deterministically.**
  Sessions never touch the tracker, and no agent does at run time.
  Built-in adapters are code; command templates for other trackers are approved once per machine before they run, and orchestrate stores no credentials.
- **One run per repo.**
  A lock keeps a second run from starting while one is live.

## Documentation

- [Configuration](docs/configuration.md): every key of the repo and machine config files.
- [Trackers](docs/trackers.md): the built-in adapters, the adapter JSON schema and the command-template contract.
- [Glossary](GLOSSARY.md): the words orchestrate uses.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
