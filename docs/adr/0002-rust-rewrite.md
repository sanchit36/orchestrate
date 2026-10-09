# v2 is a fresh Rust codebase, not a port of v1's Python

v1 is about 2.4k lines of Python keyed on GitHub issue numbers and `gh` calls, and the tracker adapter boundary cuts through most of it.
We chose to write v2 from scratch in Rust in this repo, keeping v1 only as a behaviour reference and importing none of its code or git history.
Rust ships as one static binary, which suits an open-source tool installed on machines we don't control, and its types guard run state, resume and ticket states, where most of v1's fixes landed.

## Considered Options

- Python, porting v1 module by module: more potential contributors and code to reuse, but users need a Python toolchain and the integer ids and `gh` calls would be rewritten anyway.
- Go: a single binary and easy subprocess code, but a weaker type system for the state machine.

## Consequences

- v1's tests become a checklist of behaviours for the Rust tests to cover, not code to port.
- v1's history stays out of this repo, since it carries a work repo's config and local paths.
- The v2 spec states every carried behaviour itself and never points at v1's code or design doc.
