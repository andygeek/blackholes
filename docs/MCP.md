# MCP and terminal agent integration

[Documentation](README.md) · [Task details](TASK-DETAILS.md)

## Connect external AI clients

The desktop configures the Blackholes MCP in Codex and Claude Code profiles on
startup, including after updates. Task-agent launches also receive the connection
through their launch settings.
Default profiles, existing legacy Blackholes profiles, `CODEX_HOME` / `CLAUDE_CONFIG_DIR`, existing `-work` profiles,
and the legacy Claude script profile are supported. Configuring one client does
not require the other client to exist, and a profile failure does not block the app
or the remaining profiles.

The MCP registration points to a private launcher under Application Support.
Blackholes updates that launcher to its current executable location, so an app
update does not leave each client pointing to an old build. The routing skill is
embedded in the executable; packaged releases need no repository checkout or
copy of `scripts/install-ai-integrations`. Install the app in Applications and
open it there; setup is deferred while running from a disk image or Gatekeeper's
temporary App Translocation directory.

Setup preserves unrelated MCPs, configuration values, custom Blackholes entries
and unmanaged skills. Codex TOML comments are retained. Files are replaced
atomically only when changed; unexpected concurrent edits are reported for retry.
Explicitly disabled Codex entries remain disabled. No credentials, approval modes,
shell profiles, or global PATH entries are installed or modified by this setup.
See **Settings → MCP servers → Blackholes in your terminal agents** for profile
status or to refresh the connection. Each client appears once; expand
**Configuration details** to see its profile paths, individual statuses, and any
warnings. The client shows **Needs attention** if any profile has a setup issue.
"Configured" describes registration, not CLI installation, authentication, or a
verified agent session. Sign in through the provider CLI itself.

The repository script remains available as an optional manual tool for advanced
profiles and development:

```bash
./scripts/install-ai-integrations
./scripts/install-ai-integrations status
```

Restart the external client session after installation. The installer supports `--codex`, `--claude`, `--codex-home PATH`, `--claude-home PATH`, `--binary PATH`, and `uninstall`. It honors custom profile locations and manages only its own registrations and skill files.

## Start task agents from any MCP client

Ask your agent to create implementation tasks. `create_task` now starts each
task agent automatically (`startAgent:true`), using the caller's provider.
Pass `startAgent:false` for planning/backlog-only work or explicit requests not
to execute. `agentPrompt` can supply the implementation brief and constraints.

For a batch, the coordinating agent creates each task with `startAgent:false`,
then calls `start_task_agents` once with their IDs:

```json
{
  "tasks": [
    { "taskId": "<first-task-id>", "prompt": "Implement the Teams page. Do not run tests or push." },
    { "taskId": "<second-task-id>", "prompt": "Improve mobile navigation. Do not run tests or push." },
    { "taskId": "<third-task-id>", "prompt": "Improve empty states. Do not run tests or push." }
  ]
}
```

The tool launches 1–8 visible native terminals in their respective task
workspaces, with the implementation brief already submitted. They run
concurrently and appear beneath their tasks; the coordinating session keeps focus.
Each agent is told to read task details, any preserved legacy notes, and repository instructions, work only
in the attached worktrees, and show its progress and result in its terminal.
No completion notification is required.

Provider selection uses a per-task `agent`, then the batch `agent`, then the
calling terminal/provider or MCP client's identity. Supported values are
`codex`, `claude`, `gemini`, and `opencode`. Other MCP clients can
specify one of these providers explicitly. The launcher reuses provider profile directories
when supplied by the caller (Codex, Claude, and Gemini), without copying credentials.
Task terminals run the installed CLI through the same interactive login shell
as manually opened terminals. Missing commands and provider startup errors are
visible in that terminal.
The initial launch injects the Blackholes MCP connection through invocation
settings for Codex/Claude/OpenCode and workspace settings in the managed task
container for Gemini. It does not require an MCP registration in the inherited profile.

Each task uses its project's **Start agents without permission prompts** setting.
For these prompted launches, Codex also receives a session-only trust override
for the task directory, Claude acknowledges its bypass-mode confirmation through
session settings, and Gemini receives `--skip-trust`. User configuration files
are not rewritten. Existing sign-in and provider onboarding are still required;
Blackholes does not type arbitrary confirmation responses into a terminal.

Creation returns `agentStartRequested` and, when requested, `agentLaunch` alongside
the task metadata. Task creation can succeed while launching fails: retry that
existing task ID instead of recreating it. Launch results distinguish started
terminals, already-open terminals, and per-task errors. Retrying a live task/provider does not submit its prompt again. A launch
confirmation does not mean the provider has authenticated or completed the work.
Initial prompts are not persisted or replayed when restoring terminal sessions.
Reopen the updated Blackholes application and reconnect existing MCP clients
after installing a build that adds the tool.
