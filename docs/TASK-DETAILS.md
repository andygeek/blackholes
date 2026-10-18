# Worktree Groups and current context

A **Worktree Group** (formerly a task) holds one or more repository worktrees
and branches, agent sessions and a shared context document. Its page is directly
editable: click the title or document and type. Text areas grow with their
contents and have no surrounding form boxes or separate Edit/Done mode.
Sessions, worktrees and previous notes remain available in collapsible sections.

Changes save automatically after a short pause (900 ms) or when leaving the page;
`Cmd+S` saves immediately. The header shows saved, saving or incomplete-field
status. You can keep typing during a save: acknowledgements preserve subsequent
edits and queue them using the returned revision. Drafts survive navigation in
the current app session. Concurrent agent updates stop autosave, retain the draft
and show the latest version for review instead of overwriting it.

Click the large icon above the title to choose an emoji or a bundled icon. You
can paste a custom Unicode emoji/symbol. The icon is shared with sidebar group
rows and project overviews and stored with the group metadata. Icon edits are
revision checked and survive subsequent session/worktree persistence.

## One document, multiple sources

The entire current description is visible and editable. Its first paragraph
still supplies the short summary returned by MCP (up to 320 characters). Text
supports Markdown as source and is never interpreted as user-supplied HTML.
Start with a short paragraph for a useful summary.

Agents must maintain the document as **current context**, replacing stale
content after meaningful changes and before finishing. Preserve the objective,
user requirements, constraints, essential decisions and unresolved work. Do not
append dated updates, action histories, changelogs or progress journals.
Generated CLAUDE.md / AGENTS.md, launch prompts, the routing skill and MCP server
instructions all carry this rule. This is agent guidance, not a background model
that automatically rewrites the document or a guarantee that a client obeys it.

A group supports up to 64 links. Each has `url`, optional `label`, and `kind`
(`pull_request` or `context`, default `context`). Use multiple PRs across any
repositories, plus ClickUp, Slack, documents or other context. URLs must be
complete HTTP(S) addresses without embedded credentials, up to 4,096 characters;
labels allow 200 characters. Duplicate URLs are normalized and stored once.
The editor shows only a web-styled type selector (**Pull Request GitHub** or
**Context**) and URL, plus open/remove actions. There is no name input. Existing
labels and the optional MCP `label` field remain compatible without being shown
as editable fields. Link opening uses the system browser. Saving a link does not fetch its contents,
synchronize PR status or authorize a remote mutation.

## MCP API

New clients discover these names; task-named calls remain accepted for older
clients, with their previous argument/result key names:

| Tool | Input / result |
| --- | --- |
| `list_worktree_groups` | Optional `projectId`; `{worktreeGroups:[...]}` |
| `search_worktree_groups` | `query`, optional `projectId` / `limit`; `{worktreeGroups:[...]}` matches title, document, links, legacy notes and repositories |
| `get_worktree_group` | `groupId`; `worktreeGroup`, `details`, `detailsRevision`, project and writable/source paths |
| `create_worktree_group` | `projectId`, `title`, optional `description`, `links`, branch/repository and agent-launch options |
| `update_worktree_group` | `groupId`, optional `title`, `icon`, `description`, `links`, `color`, `expectedRevision` |
| `start_worktree_group_agents` | `groups:[{groupId,prompt?,agent?,model?}]`, optional batch `agent` / `model` |
| `add_worktree_group_repositories` / `remove_worktree_group_repositories` | `groupId`, repository IDs and existing branch/file options |
| `open_worktree_group` / `notify_worktree_group_ready` | `groupId`, existing navigation/notification options |
| `close_worktree_group` / `close_worktree_groups` | `groupId` / `groupIds`, `confirm:true`, existing terminal/file/branch policies |

Read `get_worktree_group` before editing, then patch only fields that changed:

```json
{
  "groupId": "<group-id>",
  "expectedRevision": "<detailsRevision-from-get_worktree_group>",
  "description": "Export reports using the current filters. Implementation is ready; empty-state review remains.\n\nPreserve the existing CSV format. The API and web changes are in the linked PRs.",
  "links": [
    {"kind":"pull_request","label":"API","url":"https://github.com/OWNER/API/pull/123"},
    {"kind":"pull_request","label":"Web","url":"https://github.com/OWNER/WEB/pull/456"},
    {"kind":"context","label":"Requirements","url":"https://app.clickup.com/t/EXAMPLE"},
    {"kind":"context","label":"Discussion","url":"https://example.slack.com/archives/CHANNEL/p1234567890"}
  ]
}
```

Omission preserves a field. `description:null` clears the document; `links:[]`
clears all links. A supplied description replaces the full document and a links
array replaces the full collection. Keep still-relevant information/links when
rewriting. A stale revision rejects the patch: reread and reconcile. Metadata
writes serialize in SQLite; session/worktree updates cannot overwrite newer
context. `icon` accepts a bundled icon ID (`layers`, `folder`, `code-2`, `square-terminal`,
`rocket`, `database`, `globe`, `list-todo`, `git-branch`) or `emoji:<symbol>` with up
to 32 Unicode characters. HTML, SVG and remote images are not icon inputs.
Titles allow 500 characters; documents allow 200,100 characters (enough
to preserve both previous 100,000-character fields). Whitespace-only optional
text becomes null.

## Compatibility and generated files

Database IDs, internal IPC names, storage directories and filenames remain
stable. No existing group, branch or session is recreated by this rename.
Existing acceptance criteria appear inside the single document until its next
rewrite. Existing single PR/external URLs appear in the link collection; saving
the new collection replaces those legacy slots, including when clearing it.
Legacy fields remain readable through compatibility APIs, which still accept
`acceptanceCriteria`, `pullRequestUrl` and `externalTaskUrl`.

The managed workspace retains `.blackholes-task-details.md`, `.blackholes-task.json`
(manifest version 3, including links and summary), and the project instruction
filename `.blackholes-task-CLAUDE.md`. These are compatibility paths; do not rename
them manually. Startup repairs generated group files. Only exact known default
instruction templates are upgraded automatically; custom instructions survive.
Every generated group header also includes the current-context maintenance rule.

Previous `.blackholes-note.md`, `.blackholes-project-note.md` and rich-note
sidecars remain available. Legacy note tools remain callable but task note
writers are no longer advertised to new clients. They do not update the current
context document. Reconnect external MCP clients after updating Blackholes to
refresh the tool list and embedded routing guidance.

## Manual review

With a matching updated app/MCP build:

1. Open an existing group. Type directly in the title/document, wait for Saved,
   navigate away and return. Type during an in-flight save and verify the newer
   text remains. Empty titles or unfinished URLs must stay as drafts.
2. Change its icon to an emoji and a bundled icon. Verify the page, sidebar and
   project overview agree, including after session updates and app relaunch.
3. Add several GitHub PR and context URLs using the custom web selector. Verify
   there is no name input, open links, remove all links, and confirm they stay empty.
4. While a draft is dirty, update the group through MCP with a revision. Verify
   automatic saves stop, your draft stays visible and the latest version can be
   reviewed/reloaded. Inspect generated context and a newly authorized agent's guidance.
5. Relaunch Blackholes when safe to do so. Agents must exclude dormant restored
   sessions until you activate them through Projects. Idle/working/attention
   sessions with live terminal processes appear; exited sessions disappear.
   Reordering active agents must retain dormant sessions' saved order.

Do not launch a second build against a live production data directory.

## Implementation files

- `frontend/src/workspace/TaskDetails.tsx`, `groupDetailsState.ts`, `GroupIconPicker.tsx`, `frontend/src/shared/GroupIcon.tsx`, `main.tsx`, `assets/workspace/workspace.css`: inline document editing, queued autosave/conflicts, emoji picker and simplified links.
- `frontend/src/navigation/main.tsx`, `CreateTaskForm.tsx`, `AppModal.tsx`, `WorkspaceSurface.tsx`, `src/ui/app.rs`: product terminology and navigation/edit actions.
- `src/model.rs`, `src/services/task_details.rs`, `tasks.rs`, `database.rs`: link storage/validation, legacy context preservation, revision checks and generated manifest.
- `src/bin/blackholes-mcp.rs`: canonical tools, compatibility adapters, document updates and server guidance.
- `src/services/notes.rs`, `task_agents.rs`, `integrations/skills/blackholes/SKILL.md`: maintained-context rules, template upgrades and agent prompts.
- `README.md`, `docs/USAGE.md`, `docs/MCP.md`, this document and generated frontend bundles: current user/API documentation and embedded assets.

Unit coverage was added for legacy fields, clearing links, URL validation,
revision conflicts, Unicode summaries, instruction migration, icon validation/revisions and MCP aliases.
These new tests were not executed during this UI change. The release build
compiles the production frontend and Rust binaries; runtime UI/agent behavior
still needs manual review. Existing task-closure and provider-compatibility
changes in this checkout are retained separately from this document redesign.
