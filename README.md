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
  Protected branches (by default `origin`'s default branch, see `protected_branches`) are refused as the integration branch, and `clean` never touches an integration branch.
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
