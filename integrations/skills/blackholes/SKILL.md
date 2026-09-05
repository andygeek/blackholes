---
name: blackholes
description: Use whenever the user mentions Blackholes as the local project and task manager, or asks to manage Blackholes projects, repositories, tasks, branches, worktrees, notes, or notifications. Prefer the Blackholes MCP over filesystem discovery and Computer Use. Never create a Blackholes task unless the user explicitly requests one.
---

<!-- managed-by: blackholes-ai-integrations -->

# Blackholes MCP

Use the MCP server named `blackholes` as the primary interface for Blackholes requests.

- Before searching the filesystem or controlling the desktop app, locate and use the Blackholes MCP tools.
- Never use Computer Use for an operation exposed by the Blackholes MCP. Use UI control only when the user explicitly requests visual interaction that the MCP cannot perform.
- When the user asks to create a project and provides only its name, call `create_project` with that name immediately. Do not ask for a path or technology.
- Resolve projects and tasks with the MCP search tools before asking the user for internal IDs.
- Call `create_task` only when the user explicitly asks to create a task in Blackholes. Do not infer task creation from a request to fix, build, inspect, or change code; from the current repository being managed by Blackholes; or from a mere mention of Blackholes as the product or project.
- Without an explicit request to create a task, work in the checkout selected by the user or the current working directory and do not create a Blackholes task or worktree.
- When the user explicitly asks to work in an existing Blackholes task, resolve it first and make code changes only in the writable worktree paths returned by `get_task` or `get_current_context`; treat its original project repositories as read-only context.
- For work performed in a Blackholes task, call `notify_task_ready` only after all requested work is finished, and make it the final Blackholes tool call.

If the Blackholes MCP is unavailable, explain that the local integration must be installed or repaired instead of silently falling back to filesystem or UI automation.
