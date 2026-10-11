# Orchestrate

A tool that takes a spec on any tracker and works its tickets as unattended background sessions, landing gated work on one integration branch.

## Work on the tracker

**Tracker**:
The system where a repo's specs and tickets live, such as GitHub Issues, Jira, Linear, or markdown files in the repo.
_Avoid_: issue system, board

**Spec**:
The parent work item that describes a whole piece of work and owns its tickets.
_Avoid_: PRD, epic, parent issue

**Ticket**:
One unit of work under a spec, sized for a single session.
_Avoid_: sub-issue, task, child issue

**Blocked by**:
The relationship from a ticket to another ticket that must finish before it can start.
_Avoid_: depends on, prerequisite

**Ready**:
The mark on a ticket saying it may be worked by an unattended session; a ticket that is not ready never starts.
_Avoid_: approved, queued

**Finished**:
The state of a ticket whose gated work has landed on the integration branch, as the tracker records it.
_Avoid_: done, closed, merged

**Frontier**:
The open, ready tickets of a spec whose blockers have all finished, so they can start now.
_Avoid_: wave, ready queue

**Acceptance criteria**:
The checklist on a ticket that states what must be true for it to count as done.
_Avoid_: definition of done, checklist

## Running the work

**Run**:
One invocation of the orchestrator against one spec, with its own state and history.
_Avoid_: job, execution

**Session**:
A detached, top-level agent process working one ticket in its own worktree; it may start its own subagents.
_Avoid_: worker, subagent

**Report**:
What a session hands back when it exits: done or stuck, a verdict on each acceptance criterion, and notes; never trusted alone, since gates decide.
_Avoid_: result file, result contract

**Stuck**:
The outcome of a session that needs a human before its work can land: it reports it cannot go on, or reports any acceptance criterion unmet; distinct from a ticket being blocked by another ticket.
_Avoid_: blocked

**Paused**:
The state of a run that starts no new sessions because the tracker could not be read or written; running sessions carry on, and the run unpauses by itself once the tracker answers.
_Avoid_: held, suspended

**Integration branch**:
The single branch where a run lands every ticket's gated work, for a human to review.
_Avoid_: staging branch, feature branch

**Gate**:
A check that runs against a session's work and decides whether it may land on the integration branch.
_Avoid_: check, CI step

**Placeholder gate**:
A gate that always passes, standing in until the repo has real checks; work it lets through lands unchecked.
_Avoid_: dummy gate, no-op gate
