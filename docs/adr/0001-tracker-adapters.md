# Tracker access through deterministic adapters, not an agent

Orchestrate reaches a tracker through adapters that meet one fixed JSON schema for four operations: get the spec, list its tickets, list a ticket's blockers, mark a ticket finished.
Built-in adapters (GitHub, local markdown) are code; any other tracker is added as four shell command templates that an agent compiles from the repo's tracker guide at setup and the user approves once.
We rejected letting an agent read the tracker guide and perform operations at run time, even though it would cover every tracker for free, because the frontier is computed from these answers and one misread blocker or wrong close starts work on unfinished ground with nobody watching.

## Consequences

- Every adapter response is validated against the schema; on error or bad output the run retries briefly, then pauses and starts nothing new.
- Sessions never touch the tracker; the orchestrator is its only reader and writer.
- No agent touches the tracker at run time, not even for optional writing back: commenting and ticking criteria are optional deterministic adapter operations, skipped where an adapter lacks them.
