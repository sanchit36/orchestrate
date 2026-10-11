---
name: release
description: Cut a new orchestrate release - bump the crate version, open the release PR, then tag it so dist publishes to crates.io and GitHub Releases.
disable-model-invocation: true
---

# Release

A release is a version-bump PR merged to `main`, then a `vX.Y.Z` tag on the merge commit.
Pushing the tag is the release: `.github/workflows/release.yml` (cargo-dist) builds the binaries, publishes to crates.io and creates the GitHub release.
A crates.io publish is permanent, so every step before the tag must be green.

## Steps

1. **Sync and scope.**
   `git switch main && git pull --ff-only && git fetch --tags`, then `git log --oneline $(git describe --tags --abbrev=0)..HEAD`.
   Done when you have the list of PRs since the last tag.
   If the list is empty, stop: there is nothing to release.

2. **Pick the version.**
   Propose the next version from that list (patch for fixes and small additions, minor for anything breaking while we are on 0.x) and confirm it with the user before bumping.
   Done when the user has agreed to a version.

3. **Bump.**
   On a `release-X.Y.Z` branch, edit `version` in `Cargo.toml`, then `cargo update -p orchestrate --precise X.Y.Z` so `Cargo.lock` matches.
   Done when `git diff --stat` shows exactly `Cargo.toml` and `Cargo.lock`, one line each.

4. **Prove it locally.**
   Run `cargo test --locked`, commit as `Release X.Y.Z`, then run `cargo publish --dry-run --locked` (it refuses a dirty tree, so commit first).
   Done when every `test result` line is `ok` and the dry run ends with `aborting upload due to dry run`.
   `tests/release.rs` checks `--version` against the crate version, so it follows the bump on its own.

5. **Open the release PR.**
   Push the branch and open a PR titled `Release X.Y.Z`, using the body template below.
   Wait with `gh pr checks <n> --watch`, and confirm the latest CI run on `main` passed too.
   Done when every check is `pass`.

6. **Merge and tag.**
   Auto mode refuses to merge without review, so hand this to the user to run as a `!` command, with the real PR number and version filled in:
   ```
   gh pr merge <n> --merge --delete-branch && git switch main && git pull --ff-only && git tag vX.Y.Z && git push origin vX.Y.Z
   ```
   Tag the merge commit on `main`, never the release branch.
   Done when `git ls-remote --tags origin vX.Y.Z` shows the tag.

7. **Watch it ship.**
   `gh run list -w Release -e push -L 1` (the workflow also runs a plan-only pass on every PR), then `gh run watch <id>`.
   Done when the run succeeds, `gh release view vX.Y.Z` exists and is marked Latest, and `cargo search orchestrate --limit 1` shows `X.Y.Z`.
   If the run fails after the crates.io publish step, the version is burned: fix forward with the next patch, never re-tag.

## PR body template

````markdown
## Summary

```diff
 Cargo.toml
-version = "<old>"
+version = "X.Y.Z"
```

Ships <each PR since the last tag, by number, in one line each>.

## Evidence

- `cargo test --locked` passes.
- `cargo publish --dry-run --locked` passes at X.Y.Z.

## Merge Danger

**Door:** two-way

**Blast Radius:** none

The release itself happens on the `vX.Y.Z` tag after merge; publishing to crates.io is permanent.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
````
