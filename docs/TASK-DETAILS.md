# Task details and MCP

A task title opens its details. **Edit task** opens the same page. Projects open
an overview of sessions, tasks, and repositories. The task `+` menu offers
**Blank Terminal**, **Claude**, and **Codex**; its `…` menu offers **Edit task**
and **Delete task**.

## Fields

| Field in the app | MCP input | Field in returned task JSON |
| --- | --- | --- |
| Title | `title` | `title` |
| Objective and description | `description` | `description` |
| Acceptance criteria | `acceptanceCriteria` | `acceptance_criteria` |
| Pull request | `pullRequestUrl` | `pull_request_url` |
| External task | `externalTaskUrl` | `external_task_url` |

Descriptions and criteria are plain text, including Markdown if useful. A task
has one optional PR link and one optional external task link. URLs must be
absolute HTTP(S) addresses without embedded credentials. They open in the
system browser. Blackholes does not fetch issue contents, authorize a provider,
or synchronize PR status from those URLs.

`create_task` accepts these fields in addition to its branch/repository and
agent launch inputs. Existing tasks default new fields to empty; their current
description remains the objective. Titles allow 500 characters, descriptions
and criteria 100,000 each, and links 4,096 each. Blank optional values are stored
as null; titles must be nonempty. Legacy task records remain readable.

## Reading and updating through MCP

`get_task` returns the task and its `detailsRevision`, project, repositories,
and previous note content. `get_current_context` includes the revision for task
contexts. `search_tasks` also matches acceptance criteria and links.

To update from a snapshot:

1. Read `get_task` with the task ID.
2. Reconcile the requested change with the returned values.
3. Call `update_task`, passing only changed fields and the returned revision.

```json
{
  "taskId": "<task-id>",
  "expectedRevision": "<detailsRevision-from-get_task>",
  "description": "Allow users to export the selected report.",
  "acceptanceCriteria": "- Export respects active filters.\n- Empty reports show a clear message.",
  "pullRequestUrl": "https://github.com/OWNER/REPOSITORY/pull/123",
  "externalTaskUrl": "https://app.clickup.com/t/TASK_ID"
}
```

Omitted fields remain unchanged. Null clears an optional field. A stale
`expectedRevision` rejects the update without applying it; reread and reconcile
instead of blindly resubmitting. Without a revision the patch applies to the
latest stored task. Updates return task JSON plus the new `detailsRevision`.
Metadata writes are serialized in SQLite and preserve task sessions and attached
repositories. Revisions cover metadata, so starting a terminal does not conflict
with a details edit.

The UI always sends a revision. Unsaved drafts remain in memory when navigating
to another view, and failed saves preserve their content. **Discard draft and
reload** replaces the draft with the latest received values. Drafts are not
restored after quitting the app.

## Generated context and previous notes

The task workspace contains `.blackholes-task-details.md` and a version 2
`.blackholes-task.json` manifest with the same fields. They are generated when
a task is created, repaired, or updated. Edit through the app or MCP; direct
changes to generated files can be overwritten. `get_task` is the current source
of truth. Startup refreshes managed task files, and default instructions direct
agents to task details before implementation.

Previous `.blackholes-note.md`, `.blackholes-project-note.md`, and rich-note
sidecars remain on disk. Nonempty task notes are available read-only under
**Previous notes**. Copy useful content into the structured fields when desired;
there is no automatic conversion that could overwrite an existing objective.
Legacy MCP note readers/writers remain available for older clients and do not
update the new fields. New workflows should use `update_task` and project
instructions. Only the exact old built-in task instruction template is migrated;
custom project/task instructions are preserved.

Restart external MCP clients after installing an updated build to refresh their
tool schemas and embedded routing guidance.
