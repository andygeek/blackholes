---
name: blackholes
description: Use whenever the user mentions Blackholes as the local project and Worktree Group manager, or asks to manage its projects, groups (formerly tasks), repositories, branches, worktrees, context, terminals or notifications. Prefer the Blackholes MCP. Never create a Worktree Group unless explicitly requested.
---

<!-- managed-by: blackholes-ai-integrations -->

# Blackholes MCP

Use the MCP server named `blackholes` for Blackholes orchestration. Locate its tools before filesystem discovery or desktop control. Never use Computer Use for an operation exposed by the MCP. When changing Blackholes source code directly at the user's request, work in the selected checkout without creating a group.

## Projects and Worktree Groups

- A Worktree Group contains one or more repository worktrees and branches, sessions and a current context document. The legacy name was task. Use Worktree Group terminology and the new tools; old task-named tools are compatibility aliases.
- Use search tools to resolve names before asking for IDs. `list_worktree_groups` returns `{worktreeGroups:[...]}`; `get_worktree_group` takes `groupId` and returns `worktreeGroup`, `detailsRevision`, project and writable paths.
- Call `create_worktree_group` only if the user explicitly asks to create a Blackholes group/task. A request to fix, inspect or build code, or the checkout being managed by Blackholes, does not authorize creating a group. Work directly in the intended checkout otherwise.
- Creating a project from only a name needs no path or technology question: call `create_project`. Explicit source paths link repositories without moving their files.
- Inside an existing group, change only its attached writable worktrees returned by MCP; original repositories are read-only context. Add/remove attached repositories with `add_worktree_group_repositories` / `remove_worktree_group_repositories`. Never construct worktrees manually.

## Keep context current

- Before implementation, read `get_current_context`, `get_worktree_group` and repository instructions. Read linked authoritative ClickUp, Slack, GitHub or other sources when needed.
- `description` is a living document. Its first paragraph is the short summary (up to 320 characters shown). After meaningful changes and before finishing, reread the group and use `update_worktree_group` to REPLACE stale context. Preserve current requirements, user constraints, essential decisions and unresolved work.
- NEVER append a changelog, dated updates, completed-action lists, a progress history or a journal. Remove obsolete context while preserving still-relevant information. Do not use legacy note-append tools for current context.
- `links` is a collection of `{url, label?, kind}` entries, with kind `pull_request` or `context`. Keep multiple PRs and context sources (ClickUp, Slack, documents). Preserve relevant links, remove obsolete/duplicate entries. Recording a URL does not authorize remote writes.
- Patch only changed fields. Omission preserves a field; `description:null` clears the document, and `links:[]` clears all links. A supplied links array replaces the whole collection. Pass `detailsRevision` as `expectedRevision`; on conflict reread and reconcile instead of blindly overwriting another agent.
- `.blackholes-task-details.md` and `.blackholes-task.json` are generated compatibility filenames. Update context through MCP. Legacy `.blackholes-note.md` remains additional read-only context for new workflows.

## Launch and navigation

- `create_worktree_group` defaults to `startAgent:true`, launching a visible terminal with the calling provider and project permission setting. Include all user constraints in `agentPrompt` or description. Set `startAgent:false` for planning/backlog-only requests and explicit instructions not to start; do not launch them afterwards.
- For an authorized batch, create each group with `startAgent:false`, then use `start_worktree_group_agents` once with `groups:[{groupId,prompt,agent?,model?}]` (1–8). Optional batch `agent` / `model` provide defaults; otherwise the caller/provider default applies.
- Inspect creation `agentLaunch` and every batch result. `started` confirms a terminal launch; `reused` keeps an existing terminal without resubmitting; `error` is a failure. Retry failed launches on the existing group, never recreate it or duplicate successful launches. The receiving agent implements directly without redelegating.
- `open_project_terminal` opens a plain project/repository shell and optionally runs one command without a group. It creates a new terminal every call: do not retry a started launch. Use `reveal:true` only when the user wants to see it now.
- Use `open_worktree_group` / `open_project` only when asked to open/show/go to them. Terminal agents show progress and results in their terminals. Completion notifications are optional; if explicitly requested, call `notify_worktree_group_ready` after finishing, as the final Blackholes call.

## Close groups

- `close_worktree_groups` takes 1–32 `groupIds`; `close_worktree_group` takes one `groupId`. Both require `confirm:true`. Read details before destructive operations and honor the user's scope.
- Restored tabs without live processes close automatically, returning Claude/Codex session IDs. `terminals` defaults to `fail`; `close_idle` allows idle/attention terminals, and `close_all` also allows working terminals. Caller groups are protected when identifiable; detached external clients can lack caller information.
- `uncommittedChanges` and `extraFiles` default to `fail`; only explicitly choose `discard` within the user's authorization. Managed worktrees and the group folder are deleted to free disk; branches remain unless `deleteBranches:true`.
- Inspect every deleted/blocked result, reason, partial cleanup, sessions and bytes freed. A blocked group does not stop other groups.

If MCP orchestration is unavailable, explain that the local integration needs repair rather than silently replacing it with UI or filesystem mutations.
