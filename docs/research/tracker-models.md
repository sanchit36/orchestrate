# How trackers represent specs, tickets and blocking

Research for [issue #2](https://github.com/sanchit36/orchestrate-issues/issues/2), part of map [#1](https://github.com/sanchit36/orchestrate-issues/issues/1).
Researched on 2026-10-09 against primary sources only: official docs, API references, CLI manuals, and live API schemas.
Terms follow `GLOSSARY.md`: a **spec** owns **tickets**, a ticket is **blocked by** other tickets, and the **frontier** is the open tickets whose blockers have all finished.

For each tracker this note answers six questions:

1. Parent and children: what can hold a spec and its tickets.
2. Blocking edges: native or not, and whether the API reports open-blocker counts.
3. Labels, status and workflow state.
4. Assignees (the claim signal).
5. Checklists in the body (where acceptance criteria live).
6. What a script can read and write through the official CLI or API, and how it authenticates.

Where a fact comes from a live probe rather than a doc page, it is marked **observed**.

## GitHub Issues

### Parent and children

GitHub has native sub-issues.
A parent can have up to 100 sub-issues, nested up to eight levels, and you can pick sub-issues from other repositories ([adding sub-issues](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-sub-issues)).
The REST API requires that "the sub-issue must belong to the same repository owner as the parent issue" ([sub-issues REST](https://docs.github.com/en/rest/issues/sub-issues)).
REST exposes `GET .../issues/{n}/sub_issues`, `GET .../issues/{n}/parent`, add, remove and reprioritize; issue objects carry `sub_issues_summary` (`total`, `completed`, `percent_completed`) and `parent_issue_url` ([sub-issues REST](https://docs.github.com/en/rest/issues/sub-issues)).
The add endpoint takes `sub_issue_id`, the issue's numeric id, not its `#number` ([sub-issues REST](https://docs.github.com/en/rest/issues/sub-issues)).
The older "tasklist blocks" are retired, and GitHub points to sub-issues as the replacement ([about tasklists](https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/about-tasklists)).
Issue types (default task, bug, feature, up to 25) are configured at the organization level ([managing issue types](https://docs.github.com/en/issues/tracking-your-work-with-issues/configuring-issues/managing-issue-types-in-an-organization)), so a personal repo cannot mark a parent as a "spec" by type.

### Blocking edges

Native issue dependencies went GA on 2025-08-21, with up to 50 issues per relationship type (blocked by, blocking) and search filters `is:blocked`, `is:blocking`, `blocked-by:` and `blocking:` ([changelog](https://github.blog/changelog/2025-08-21-dependencies-on-issues/)).
REST has list, add and remove for `.../dependencies/blocked_by` and a list for `.../dependencies/blocking`; the add body is `issue_id`, again the numeric id ([issue dependencies REST](https://docs.github.com/en/rest/issues/issue-dependencies)).
Every issue object carries `issue_dependencies_summary` with `blocked_by`, `blocking`, `total_blocked_by` and `total_blocking`; the docs do not define the difference between the plain and `total_` counts ([issue dependencies REST](https://docs.github.com/en/rest/issues/issue-dependencies)).
**Observed** on this repo: issue #3 returned `blocked_by: 1, total_blocked_by: 1` while its only blocker #2 was open.
Immediately after #2 was closed it still returned `blocked_by: 1`; about a minute later it returned `blocked_by: 0, total_blocked_by: 1`.
So `blocked_by` counts open blockers and `total_blocked_by` counts all of them, but the summary updates asynchronously, so a script that closes a ticket and immediately recomputes the frontier can read a stale count.
Dependencies are also in GraphQL (`addBlockedBy`) and webhooks ([changelog](https://github.blog/changelog/2025-08-21-dependencies-on-issues/)).

### Labels, status and workflow state

An issue's state is only `open` or `closed`, with `state_reason` one of `completed`, `not_planned`, `duplicate`, `reopened` ([update an issue](https://docs.github.com/en/rest/issues/issues#update-an-issue)).
There is no native in-progress state on an issue; labels or a Projects field must stand in.
Issue fields (typed metadata such as Priority, Effort, Start date, Target date) went GA on 2026-07-02 for organizations; the announcement lists no default Status field and only mentions MCP as programmatic access ([issue fields GA](https://github.blog/changelog/2026-07-02-issue-fields-are-now-generally-available/)).

### Assignees

Assignees are a list; GitHub docs give a limit of 10 per issue ([assigning issues, GHES 3.16 docs](https://docs.github.com/en/enterprise-server@3.16/issues/tracking-your-work-with-issues/using-issues/assigning-issues-and-pull-requests-to-other-github-users)).
Only users with push access can change assignees, and otherwise the change is silently dropped ([update an issue](https://docs.github.com/en/rest/issues/issues#update-an-issue)).
`gh issue edit --add-assignee` accepts `@me` and `@copilot` ([gh issue edit](https://cli.github.com/manual/gh_issue_edit)).

### Checklists in the body

Checklists are plain GitHub Markdown task list items, `- [ ]` and `- [x]`, rendered as clickable checkboxes ([about tasklists](https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/about-tasklists)).
The API returns them only as body text; a script must parse and rewrite the Markdown to read or tick a criterion.

### CLI, API and auth

`gh` v2.94.0 added sub-issues, issue types and dependencies: `--parent`, `--blocked-by`, `--blocking` on create, and `--add-*` / `--remove-*` variants on edit ([changelog 2026-06-10](https://github.blog/changelog/2026-06-10-manage-sub-issues-types-and-dependencies-from-github-cli/), [gh issue edit](https://cli.github.com/manual/gh_issue_edit), [creating issue dependencies](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-issue-dependencies)).
`gh issue view --json` now exposes `blockedBy`, `blocking`, `parent`, `subIssues`, `subIssuesSummary` and `issueType` ([gh issue view](https://cli.github.com/manual/gh_issue_view)).
**Observed**: the `gh` installed on this machine is 2.92.0, which has none of these flags, so older installs must fall back to `gh api` against the REST endpoints above.
Auth: `gh auth login`, or `GH_TOKEN` / `GITHUB_TOKEN` (and `GH_ENTERPRISE_TOKEN` for GHES), which override stored credentials ([gh environment](https://cli.github.com/manual/gh_help_environment)).
Fine-grained tokens need the repository "Issues" permission: read for the GET endpoints, write for adding sub-issues and dependencies ([fine-grained PAT permissions](https://docs.github.com/en/rest/authentication/permissions-required-for-fine-grained-personal-access-tokens)).
Editing Projects from `gh` needs the extra `project` scope ([gh issue edit](https://cli.github.com/manual/gh_issue_edit)).

### Plan limits

Sub-issues and dependencies do not cite a plan restriction on the pages above; on GHES, issue types and sub-issues need 3.17+ and relationships 3.19+ per the `gh` 2.94.0 release notes ([cli v2.94.0](https://github.com/cli/cli/releases/tag/v2.94.0)).

## GitLab Issues

### Parent and children

GitLab is mid-migration to "work items": since 18.10, Plan > Issues and Plan > Epics are replaced by one Work items list, and `/issues/:iid` and `/epics/:iid` URLs redirect to `/work_items/:iid` ([work items](https://docs.gitlab.com/user/work_items/)).
On every tier an issue can have **tasks** as child items, in the same project; checklist items in the description can be converted to child tasks ([tasks](https://docs.gitlab.com/user/tasks/)).
An issue whose children are issues needs an **epic**, which is Premium and Ultimate only; nested epics are Ultimate ([epics](https://docs.gitlab.com/user/group/epics/)).
The REST `epic_id` field on issues is "Premium and Ultimate only" ([issues API](https://docs.gitlab.com/api/issues/)).
GraphQL exposes the hierarchy widget with `parent`, `hasParent`, `hasChildren`, `ancestors`, and `children` filterable by state ([GraphQL WorkItemWidgetHierarchy](https://docs.gitlab.com/api/graphql/reference/#workitemwidgethierarchy)).
Quick actions `/set_parent`, `/add_child` and `/remove_parent` exist for issues, epics and tasks ([quick actions](https://docs.gitlab.com/user/project/quick_actions/)).
So on Free, a spec that owns ticket-sized **issues** has no native parent; it is a label plus a `Part of #<n>` body line, or the tickets become tasks.

### Blocking edges

Linked issues (`relates to`) are on all tiers, but "blocks" and "is blocked by" are Premium and Ultimate ([linked issues](https://docs.gitlab.com/user/project/issues/related_issues/)).
Links can cross projects, and a blocked issue shows an icon that disappears when the blocker closes ([linked issues](https://docs.gitlab.com/user/project/issues/related_issues/)).
REST: `GET/POST/DELETE /projects/:id/issues/:issue_iid/links` with `link_type` one of `relates_to`, `blocks`, `is_blocked_by`; each listed link includes the linked issue's `state` ([issue links API](https://docs.gitlab.com/api/issue_links/)).
The REST issue object has a `blocking_issues_count` field, but the docs only show it in an example and do not define it ([issues API](https://docs.gitlab.com/api/issues/)).
GraphQL's linked-items widget has `blocked` ("Indicates the work item is blocked"), `blockedByCount` ("Count of items blocking the work item") and `blockingCount`; the docs do not say whether closed blockers are counted ([GraphQL WorkItemWidgetLinkedItems](https://docs.gitlab.com/api/graphql/reference/#workitemwidgetlinkeditems)).
Quick actions `/blocks` and `/blocked_by` exist ([quick actions](https://docs.gitlab.com/user/project/quick_actions/)).

### Labels, status and workflow state

REST state is `opened` or `closed`, changed with `state_event` `close` / `reopen`; labels are comma-separated names with `add_labels` / `remove_labels` ([issues API](https://docs.gitlab.com/api/issues/)).
Custom work item **status** is Premium and Ultimate, GA in 18.4; defaults are To do, In progress, Done, Won't do and Duplicate, in categories Triage, To do, In progress, Done and Canceled ([status](https://docs.gitlab.com/user/work_items/status/)).
Statuses in Done or Canceled close the item automatically; other categories keep it open ([status](https://docs.gitlab.com/user/work_items/status/)).
The GraphQL `status` field on the status widget is still marked "Experiment" ([GraphQL WorkItemWidgetStatus](https://docs.gitlab.com/api/graphql/reference/#workitemwidgetstatus)).
On Free, labels must stand in for an in-progress state.

### Assignees

On Free only one assignee per issue or task; multiple assignees are Premium and Ultimate ([quick actions](https://docs.gitlab.com/user/project/quick_actions/), [tasks](https://docs.gitlab.com/user/tasks/)).
REST sets them with `assignee_ids` ([issues API](https://docs.gitlab.com/api/issues/)).

### Checklists in the body

The description is Markdown with task list items, and REST reports `task_completion_status` (`count`, `completed_count`) on each issue ([issues API](https://docs.gitlab.com/api/issues/)).
That gives a script a native done/total count for acceptance criteria without parsing, though ticking one still means rewriting the description.

### CLI, API and auth

`glab issue` has `board`, `close`, `create`, `delete`, `list`, `note`, `reopen`, `subscribe`, `unsubscribe`, `update` and `view`, with no subcommand for links or parents ([glab issue](https://docs.gitlab.com/cli/issue/)).
`glab work-items` (`create`, `delete`, `list`, `update`) is marked "EXPERIMENTAL ... not ready for production use" ([glab work-items](https://docs.gitlab.com/cli/work-items/)).
So blocking and hierarchy need `glab api` calls, quick actions posted as notes, or direct REST / GraphQL.
Auth: `glab auth login` (OAuth or personal access token), or `GITLAB_TOKEN` (then `GITLAB_ACCESS_TOKEN`, `OAUTH_TOKEN`), with `GITLAB_HOST` for self-managed; env vars override the config file and are not scoped per host ([glab configuration](https://docs.gitlab.com/cli/configuration/)).
**Observed**: `glab` is not installed on this machine.

### Plan limits

The tier split is the decisive GitLab fact: on Free there are no native blocking links, no epics, no custom status and one assignee ([linked issues](https://docs.gitlab.com/user/project/issues/related_issues/), [epics](https://docs.gitlab.com/user/group/epics/), [status](https://docs.gitlab.com/user/work_items/status/), [tasks](https://docs.gitlab.com/user/tasks/)).

## Linear

### Parent and children

Native sub-issues: an issue becomes a parent once it has one, and sub-issues inherit the parent's team, priority and project ([parent and sub-issues](https://linear.app/docs/parent-and-sub-issues)).
Team settings can auto-close a parent when all sub-issues are done, and auto-close sub-issues when the parent is done ([parent and sub-issues](https://linear.app/docs/parent-and-sub-issues)).
A checklist in the description can be turned into sub-issues from the editor ([parent and sub-issues](https://linear.app/docs/parent-and-sub-issues)).
**Observed** in the live GraphQL schema: `Issue.parent`, `Issue.children`, `Issue.project`, and `IssueFilter.parent` / `children` (introspection of `https://api.linear.app/graphql`, 2026-10-09).
Linear also has Projects, which can hold a spec instead of a parent issue; issue `project` is nullable (same introspection).

### Blocking edges

Issue relations shipped in 2020 with "blocked by", "blocking" and "referencing" ([changelog](https://linear.app/changelog/2020-03-11-issue-relations)).
**Observed** in the schema: `IssueRelationType` is `blocks`, `duplicate`, `related`, `similar`; an `IssueRelation` has `issue`, `relatedIssue` and `type`; an issue exposes `relations` and `inverseRelations`, so "blocked by" is a `blocks` relation read from the inverse side.
`IssueRelationCreateInput` accepts either a UUID or a human identifier like `LIN-123` for both ends (same introspection).
There is no blocker-count field on `Issue`; `IssueFilter` has `hasBlockedByRelations` and `hasBlockingRelations`, and the schema does not say whether completed blockers still count (same introspection).
A script must therefore fetch each blocker's `state.type` to decide the frontier.

### Labels, status and workflow state

**Observed**: each team has its own `WorkflowState`s, each with a `type` of `triage`, `backlog`, `unstarted`, `started`, `completed`, `canceled` or `duplicate`, and every issue belongs to exactly one team (same introspection).
State names are free-form per team, so an integration must map on `type`, not on name.
Labels are native (`Issue.labels`, `labelIds`).

### Assignees

**Observed**: `Issue.assignee` is a single user, and there is a separate `Issue.delegate`, "the agent user that is delegated to work on this issue" (same introspection).

### Checklists in the body

**Observed**: `Issue.description` is "The issue's description in markdown format" (same introspection).
Checkboxes are Markdown in that string; there is no structured checklist field.

### CLI, API and auth

API: GraphQL at `https://api.linear.app/graphql`; personal API keys go in `Authorization: <API_KEY>` with no `Bearer`, OAuth2 tokens as `Authorization: Bearer <ACCESS_TOKEN>`; the TypeScript SDK wraps the schema ([GraphQL](https://linear.app/developers/graphql)).
Rate limits per user per hour: 2,500 requests for API keys and 5,000 for OAuth apps, plus complexity budgets and a 10,000-point cap per query ([rate limiting](https://linear.app/developers/rate-limiting)).
Linear's official MCP server is at `https://mcp.linear.app/mcp` (read-only variant `/mcp/readonly`), using OAuth 2.1 or a Bearer API key ([Linear MCP](https://linear.app/docs/mcp)).
No official Linear CLI was found in Linear's docs or changelog; scripts use GraphQL, the SDK or MCP.

### Plan limits

The Free plan caps a workspace at 250 issues and 2 teams ([pricing](https://linear.app/pricing)).
The pricing page lists "API and webhook access" as a core feature but does not show a per-plan breakdown ([pricing](https://linear.app/pricing)).

## Jira Cloud

### Parent and children

The default hierarchy is level 1 Epic, level 0 standard work items (Story, Task, Bug) and level -1 Subtask ([work type hierarchy](https://support.atlassian.com/jira-cloud-administration/docs/configure-the-issue-type-hierarchy/)).
Adding types at the Epic level or levels above it is "only available as part of Jira Cloud Premium and Enterprise" ([work type hierarchy](https://support.atlassian.com/jira-cloud-administration/docs/configure-the-issue-type-hierarchy/)).
The REST API uses one standard `parent` field for all levels; the `Epic Link` and `Parent Link` fields were deprecated in REST and webhooks ([Atlassian deprecation notice](https://community.developer.atlassian.com/t/deprecation-of-the-epic-link-parent-link-and-other-related-fields-in-rest-apis-and-webhooks/54048)).
So a spec is naturally an Epic and its tickets are Stories or Tasks; a spec whose tickets have their own children would hit the fixed levels.

### Blocking edges

Issue links are typed and admin-configurable, and the default "Blocks" type has outward description "blocks"; admins can rename, add, delete types, or deactivate linking for the whole site ([configure linking](https://support.atlassian.com/jira-cloud-administration/docs/configure-issue-linking/)).
"Jira issue links are bidirectional" and "the semantics of each direction is only interpretable at the user interface level and not at the API level" ([issue linking model](https://developer.atlassian.com/cloud/jira/platform/issue-linking-model/)).
REST: `POST /rest/api/3/issueLink` with `type`, `inwardIssue`, `outwardIssue`; the site must have linking enabled and the caller needs Link issues permission ([issue links REST](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-links/)).
There is no open-blocker count; JQL can search by link type, for example `workItemLink["is blocked by"]` ([JQL fields](https://support.atlassian.com/jira-software-cloud/docs/jql-fields/)), and status category (To Do, In Progress, Done) filters out finished blockers ([statusCategory in JQL](https://support.atlassian.com/jira/kb/how-to-search-using-statuscategory-statuscategorychangeddate-function-with-jql/)).
An integration has to be told which link type and direction means "blocked by" on that site.

### Labels, status and workflow state

Status is per-workflow and every status belongs to one of three categories, To Do, In Progress, Done ([statusCategory in JQL](https://support.atlassian.com/jira/kb/how-to-search-using-statuscategory-statuscategorychangeddate-function-with-jql/)).
Status changes go through workflow transitions, listed and executed as separate operations in the Issues API group ([issues REST](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/)).
Search moved to `GET/POST /rest/api/3/search/jql`; the old `/rest/api/3/search` endpoints are deprecated and being removed ([issue search REST](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-search/)).

### Assignees

A single assignee, set by `PUT /rest/api/3/issue/{issueIdOrKey}/assignee` with an `accountId`, or by `acli jira workitem assign` ([issues REST](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/), [acli workitem](https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem/)).

### Checklists in the body

REST v3 descriptions are Atlassian Document Format, a JSON node tree ([ADF structure](https://developer.atlassian.com/cloud/jira/platform/apis/document/structure/)).
ADF has native `taskList` / `taskItem` nodes, each item with a `localId` and `state` of `TODO` or `DONE` ([ADF JSON schema](https://unpkg.com/@atlaskit/adf-schema@57.7.5/dist/json-schema/v1/full.json), linked from [go.atlassian.com/adf-json-schema](http://go.atlassian.com/adf-json-schema)).
So acceptance criteria are structured, but a script reads and writes JSON, not Markdown.

### CLI, API and auth

The official Atlassian CLI has `acli jira workitem` with `assign`, `comment-create`, `create`, `create-bulk`, `edit`, `link`, `search`, `transition`, `view` and more ([acli workitem](https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem/)).
`acli jira workitem link create --out KEY-123 --in KEY-456 --type Blocks` creates a link ([acli link create](https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem-link-create/)), and `view --json` returns JSON ([acli view](https://developer.atlassian.com/cloud/acli/reference/commands/jira-workitem-view/)).
`acli jira auth login` takes an API token on stdin with `--site` and `--email`, or `--web` for browser OAuth ([acli getting started](https://developer.atlassian.com/cloud/acli/guides/how-to-get-started/)).
REST basic auth is `email:api_token` base64-encoded, recommended for "simple scripts and manual calls", with OAuth 2.0 (3LO) recommended for apps ([basic auth](https://developer.atlassian.com/cloud/jira/platform/basic-auth-for-rest-apis/)).
Link creation needs the `write:jira-work` classic scope or granular `write:issue-link:jira` ([issue links REST](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-links/)).

### Plan limits

Free is limited to 10 users ([Jira Cloud plans](https://support.atlassian.com/jira-cloud-administration/docs/explore-jira-cloud-plans/)); hierarchy above Epic is Premium and Enterprise ([work type hierarchy](https://support.atlassian.com/jira-cloud-administration/docs/configure-the-issue-type-hierarchy/)).

## Local markdown (`.scratch/`)

Source: the `setup-matt-pocock-skills` tracker doc (`issue-tracker-local.md`) and the `to-tickets` skill, read from `~/.claude/skills/` on 2026-10-09.

### Parent and children

The spec is `.scratch/<feature-slug>/spec.md` and tickets are one file each at `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01` (`issue-tracker-local.md`).
Parenthood is the directory; there is no ID or link.
Wayfinder uses `map.md` instead of `spec.md` in the same layout (`issue-tracker-local.md`).

### Blocking edges

A `Blocked by: NN, NN` line near the top; a ticket is unblocked when every listed file is `resolved` (`issue-tracker-local.md`).
The `to-tickets` template instead writes `**Blocked by:**` in bold and allows "the numbers/titles" or "None (can start immediately)", so the line format is not one fixed grammar (`to-tickets/SKILL.md`).
No counts; a reader parses every file.

### Labels, status and workflow state

A `Status:` line near the top (`issue-tracker-local.md`).
The values differ by producer: `to-tickets` writes `**Status:** ready-for-agent`, wayfinder uses `claimed` / `resolved` (`to-tickets/SKILL.md`, `issue-tracker-local.md`).
There is no defined "closed" or "done" value for ordinary tickets in these two files.

### Assignees

None. Claiming is `Status: claimed` (`issue-tracker-local.md`).

### Checklists in the body

GFM `- [ ]` lines under the ticket body (`to-tickets/SKILL.md`).

### CLI, API and auth

None: the filesystem is the API, git is the history, and there is no auth.
Comments append under `## Comments` (`issue-tracker-local.md`).
Writes are visible to sessions in other worktrees only after they are committed and merged or read from the main checkout, which is a property of git rather than of the convention.

## Comparison

| | GitHub Issues | GitLab Issues | Linear | Jira Cloud | Local markdown |
|---|---|---|---|---|---|
| Spec to tickets | Native sub-issues, 100 per parent, 8 levels | Issue to tasks on all tiers; epic to issues Premium+ | Native sub-issues, or a Project | Epic to Story/Task via `parent` | Directory |
| Blocked-by edge | Native, 50 per type | Native but Premium+; Free has none | Native `blocks` relation | Typed link, site-configurable name and direction | `Blocked by:` line |
| Open-blocker count from API | `issue_dependencies_summary.blocked_by` | GraphQL `blockedByCount`, semantics undocumented | No; filter `hasBlockedByRelations` only | No; JQL plus status category | No |
| Status | open/closed + `state_reason`; labels for anything else | opened/closed + labels; custom status Premium+ | Per-team states with fixed `type` | Per-workflow statuses with 3 categories, via transitions | `Status:` line, values vary by producer |
| Assignee | Up to 10 | 1 on Free, many on Premium+ | 1, plus agent `delegate` | 1 | None; `Status: claimed` |
| Body checklist | GFM, parse text | GFM, plus `task_completion_status` counts | Markdown, parse text | ADF `taskItem` with `TODO`/`DONE` | GFM, parse text |
| Official CLI covers parent and blocking | `gh` 2.94.0+ | No (`glab api` or quick actions) | No CLI | `acli` links yes, parent via edit | n/a |
| Auth | `gh auth` / `GH_TOKEN` | `glab auth` / `GITLAB_TOKEN` | API key or OAuth header | Email + API token, or OAuth 3LO | None |
| Gating plan limit | GHES 3.19 for relationships | Free tier lacks blocking, epics, status | Free: 250 issues, 2 teams | Free: 10 users; levels above Epic Premium+ | None |

## Implications for an orchestrator

- Every tracker can express "spec owns tickets" and "ticket is blocked by ticket", but only GitHub (and GitLab Premium, if `blockedByCount` excludes closed blockers) hands back a ready-made open-blocker count; elsewhere the frontier is computed by fetching each blocker's state.
- GitHub's open-blocker count lags a close by some seconds (observed above), so it is eventually consistent, not a read-after-write gate.
- "Finished" means a different thing per tracker: `closed` on GitHub and GitLab, `state.type` in `completed` or `canceled` on Linear, status category Done on Jira, and an undefined `Status:` value locally.
- Blocking is not uniformly native: GitLab Free and local markdown need a `Blocked by:` body line, and the local format already differs between `issue-tracker-local.md` and `to-tickets`.
- Jira's blocking link has no API-level meaning; the link type name and direction must be configured per site.
- Claiming by assignee works on four trackers, but local markdown has no assignee, and Linear distinguishes a human `assignee` from an agent `delegate`.
- Acceptance criteria are Markdown checkboxes everywhere except Jira, where they are ADF JSON nodes; GitLab alone returns a done/total count for them.
- The official CLIs cover different slices: `gh` 2.94.0+ covers hierarchy and blocking, `acli` covers links and transitions, `glab` covers neither natively, and Linear has no CLI, so some trackers need raw API calls whatever approach is chosen.
- IDs differ per tracker: GitHub REST wants numeric database ids for sub-issue and dependency writes while `gh` takes `#numbers`; Linear accepts `LIN-123` or a UUID; GitLab uses project-scoped `iid`; Jira uses keys like `KEY-123`.
- Plan tiers change the shape of the data on GitLab and Jira, so a single integration per tracker must handle both the native and the body-convention fallback.
