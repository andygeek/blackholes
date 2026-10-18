# MCP and terminal agent integration

[Documentation](README.md) · [Worktree Group details](TASK-DETAILS.md)

## Connect external AI clients

The desktop configures the Blackholes MCP in Codex and Claude Code profiles on
startup, including after updates. Worktree Group-agent launches also receive the connection
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

## Open project terminals

`open_project_terminal` opens a visible shell outside any worktree group, in the project
root or in one of its repositories. An optional `command`, such as a local dev
server, is typed at the prompt as if the user entered it:

```json
{ "projectId": "<project-id>", "repositoryId": "<repository-id>", "command": "pnpm dev --port 3001" }
```

The command runs in the same interactive login shell as a manually opened
terminal, stays in its history, and Ctrl-C returns to the prompt. It must be a
single line of at most 1,000 bytes. The terminal appears under the project or
repository in the sidebar and is restored as a plain shell when the session is
reopened; the command is not replayed. By default the current view is kept and a
toast links to the terminal; `reveal:true` shows and focuses it instead. Each
call opens a new terminal, so do not retry a started launch.

## Start worktree group agents from any MCP client

Ask your agent to create implementation worktree groups. `create_worktree_group` now starts each
worktree group agent automatically (`startAgent:true`), using the caller's provider.
Pass `startAgent:false` for planning/backlog-only work or explicit requests not
to execute. `agentPrompt` can supply the implementation brief and constraints.

For a batch, the coordinating agent creates each worktree group with `startAgent:false`,
then calls `start_worktree_group_agents` once with their IDs:

```json
{
  "groups": [
    { "groupId": "<first-group-id>", "prompt": "Implement the Teams page. Do not run tests or push." },
    { "groupId": "<second-group-id>", "prompt": "Improve mobile navigation. Do not run tests or push." },
    { "groupId": "<third-group-id>", "prompt": "Improve empty states. Do not run tests or push." }
  ]
}
```

The tool launches 1–8 visible native terminals in their respective worktree group
workspaces, with the implementation brief already submitted. They run
concurrently and appear beneath their worktree groups; the coordinating session keeps focus.
Each agent is told to read worktree group details, any preserved legacy notes, and repository instructions, work only
in the attached worktrees, and show its progress and result in its terminal.
No completion notification is required.

Provider selection uses a per-worktree group `agent`, then the batch `agent`, then the
calling terminal/provider or MCP client's identity. Supported values are
`codex`, `claude`, `gemini`, and `opencode`. Other MCP clients can
specify one of these providers explicitly. Model selection works the same way:
a per-worktree group `model`, then the batch `model` (`agentModel` in `create_worktree_group`), then
the provider's configured default. The value is passed to the CLI as `--model`,
for example `claude-opus-5-5`, and the terminal keeps it when its session is
restored. The launcher reuses provider profile directories
when supplied by the caller (Codex, Claude, and Gemini), without copying credentials.
Worktree Group terminals run the installed CLI through the same interactive login shell
as manually opened terminals. Missing commands and provider startup errors are
visible in that terminal.
The initial launch injects the Blackholes MCP connection through invocation
settings for Codex/Claude/OpenCode and workspace settings in the managed worktree group
container for Gemini. It does not require an MCP registration in the inherited profile.

Each worktree group uses its project's **Start agents without permission prompts** setting.
For these prompted launches, Codex also receives a session-only trust override
for the worktree group directory, Claude acknowledges its bypass-mode confirmation through
session settings, and Gemini receives `--skip-trust`. User configuration files
are not rewritten. Existing sign-in and provider onboarding are still required;
Blackholes does not type arbitrary confirmation responses into a terminal.

Creation returns `agentStartRequested` and, when requested, `agentLaunch` alongside
the worktree group metadata. Worktree Group creation can succeed while launching fails: retry that
existing worktree group ID instead of recreating it. Launch results distinguish started
terminals, already-open terminals, and per-worktree group errors. Retrying a live worktree group/provider does not submit its prompt again. A launch
confirmation does not mean the provider has authenticated or completed the work.
Initial prompts are not persisted or replayed when restoring terminal sessions.
Reopen the updated Blackholes application and reconnect existing MCP clients
after installing a build that adds the tool.

## Terminal startup and current provider integrations

Blackholes uses the developer's installed CLI rather than a bundled Claude/Codex
version or a pinned model. Provider options remain session-local. Claude's
`preferredNotifChannel: "terminal_bell"`, Codex's `tui.notification_*` settings,
and the existing resume/permission flags are still supported by their current
CLI documentation.

Agent commands, including manually opened and restored sessions, run as arguments
to the interactive login shell. They are not typed into the PTY while shell
initialization may still be reading input, and long inline MCP settings do not
hit the PTY's canonical input line limit. After the agent exits, an interactive
shell remains. Plain project-terminal commands still run at the shell prompt and
remain in shell history.

Claude, Codex and OpenCode receive the current Blackholes MCP connection on every
agent launch, including resume and custom profiles. Gemini's file-based MCP
injection remains limited to prompted managed worktree group launches. Codex's managed
connection defaults to a 610-second tool timeout so batch worktree group closure is not cut
short by its default 60-second timeout. Persistent profile synchronization keeps
an explicitly configured timeout; the in-app launch override uses 610 seconds. Codex plan queries use `app-server --listen
stdio://` with the stable initialize/account/rate-limit methods; they create no
thread or model turn. Claude plan queries still use its internal read-only
control protocol and report unavailable if that protocol changes.

Session-ID tracking hooks run asynchronously and send only a small local event;
they do not create app directories or open the database. Setup replaces only the
Blackholes handler, preserving other handlers even when they share a matcher
group. Codex SessionStart tracking has no source matcher, so forks and future
start sources are included. Current Codex requires review of new or changed hook
definitions through `/hooks`; Blackholes does not bypass that trust check or
mark third-party hooks trusted.

Claude Code 2.1.295+ can report lifecycle state through **OSC 7501 Program Status
Protocol**. The terminal answers capability queries, handles root and child
records and validates protocol size/text limits. A working record takes
precedence over a blocked/completed child so `close_idle` cannot mistake a
working terminal for an idle one. Native reports take precedence over Warp's
legacy events; older agents keep the existing fallback. Message/title contents
are validated but are not stored or shown outside the terminal. Standard
Alacritty-generated terminal-query replies are also returned to the PTY.

If startup pauses before the provider process exists, inspect shell initialization
rather than changing provider versions. In particular, `pyenv init --path` can
run `pyenv rehash`, whose lock wait defaults to 60 seconds. Use
`pyenv init --path --no-rehash` in the developer's shell configuration when rehash
on every terminal startup is unnecessary. Do not remove a lock while another
rehash may own it. This is a local shell preference, not a repository-wide
configuration change.

References: [Claude terminal integration](https://code.claude.com/docs/en/terminal-config),
[Claude hooks](https://code.claude.com/docs/en/hooks),
[Codex hooks](https://learn.chatgpt.com/docs/hooks),
[Codex app-server](https://learn.chatgpt.com/docs/app-server), and
[Program Status Protocol specification](https://www.superlogical.com/rex/docs/build/program-status).

## Close worktree groups and reclaim disk

`close_worktree_groups` closes **1–32 distinct worktree groups** independently. `close_worktree_group` accepts
one `groupId` and the same policies. Both require `confirm: true` and a running
**updated** Blackholes app: the app owns live PTYs and session persistence. The
MCP does not infer process liveness from database history or rewrite
`app-session.json` behind the app. If the app cannot be reached, the operation
fails without an offline destructive fallback. A mixed-version app rejects the
new command; use matching app/MCP versions when ready to install an update.

```json
{
  "groupIds": ["<worktree group-uuid-1>", "<worktree group-uuid-2>"],
  "confirm": true,
  "terminals": "close_idle",
  "uncommittedChanges": "fail",
  "extraFiles": "fail",
  "deleteBranches": false
}
```

| Input | Default | Behavior |
| --- | --- | --- |
| `terminals` | `"fail"` | A live process blocks. `"close_idle"` permits idle/attention terminals; `"close_all"` also permits working terminals. Unknown live states require `close_all`. |
| `uncommittedChanges` | `"fail"` | Tracked changes, untracked files **and ignored files** block and are listed. `"discard"` permits their deletion. |
| `extraFiles` | `"fail"` | Files/directories at the worktree group root outside managed worktrees and Blackholes-generated files block and are listed, e.g. `pdf-proof/`. `"discard"` permits their deletion. |
| `deleteBranches` | `false` | Preserve worktree group branches. `true` also force-deletes the local worktree group branches; Git still refuses branches checked out elsewhere. |

Restored/exited tabs with no live process close automatically even with
`terminals: "fail"`. Historical task-session state does not block closure.
Live terminals are checked using the app's child handles. Closure sends SIGHUP
to the terminal's owned process groups, waits up to three seconds, then sends
SIGKILL and verifies exit for up to two seconds. It runs outside the UI thread.
The app prevents terminal relaunch in worktree groups while they are closing.

The **entire calling worktree group** is protected when its terminal ID/worktree group ID environment,
MCP process ancestry, or MCP working directory identifies it. This prevents both
killing the caller and deleting its working directory. Close that worktree group from the
UI or a client rooted in another project. An externally launched/detached MCP
server may expose neither the client's real cwd nor its ancestry/environment;
in that case caller identification is unavailable. `close_all` is still explicit.

Preflight checks every worktree and file policy before closing any terminal of
a worktree group. Worktrees must be registered to the project's source repositories and
inside the managed worktree group directory. Worktree Group/worktree symlinks and escaped paths are
rejected. Extra symlinks are reported as extra files; discard removes the link,
without following it. Blackholes removes each worktree via `git worktree remove`,
prunes the origin, and removes the worktree group directory. Source checkouts survive. Locked worktrees remain protected by Git and can
block cleanup; Blackholes does not override their locks. Unknown provider
configuration directories are conservatively treated as extra files.
Missing worktree directories after a partial cleanup can be retried using the
project's source repository metadata.

The result is an object with `results`, one entry per input worktree group, and a nullable
`persistenceError`. Each entry contains:

- `groupId`, `status` (`"deleted"` or `"blocked"`), and nullable `reason`.
- `closedTerminals`: IDs of closed tabs; `sessions`: `[{"agent":"codex"|"claude","sessionId":"..."}]`.
- `removedWorktrees`: repository ID, worktree path, branch and `branchDeleted`.
- `discardedExtraFiles`: extra root paths deleted with explicit consent.
- `bytesFreed`: estimated allocated bytes removed, from filesystem block counts.
  This excludes shared Git objects and is not a volume free-space delta;
  compression, hardlinks and copy-on-write may affect actual reclaimed space.

One blocked worktree group does not stop the remaining worktree groups. Filesystem/process cleanup
is not transactional: a late failure can leave a `blocked` worktree group with some closed
terminals or removed worktrees. Completed actions remain in its result; the worktree group
record stays for inspection/retry. Check `persistenceError` before assuming the
session snapshot was saved. Bridge timeouts mean an unknown outcome: inspect
before retrying. Completed closure results, including Claude/Codex IDs, are kept
in the local database's `app_meta` key `task-closure:<taskId>` for UI and MCP
closures. The result and successful worktree group-record removal are saved in one SQLite transaction.
This does not delete the providers' own session history.

The UI **Close group** action uses this same service. Its single confirmation
lists terminals, tracked/untracked/ignored changes and extra root paths. It
explicitly discards that inventory and keeps branches. If that inventory changes
before confirmation or while terminals stop, closure blocks for another review.

### MCP result objects

`list_worktree_groups` returns `{ "worktreeGroups": [...] }`; `list_projects` returns
`{ "projects": [...] }`. All tools return an object in `structuredContent`,
including errors. A defensive `{ "result": ... }` envelope handles any future
non-object tool result. Text content describes the same object.

### Verification without live app data

The closure tests create temporary repositories/directories and owned PTY
subprocesses. They do not discover app paths, launch Blackholes, use its sockets,
or load its real session/database. From the repository root on macOS:

```bash
./scripts/build-release
BLACKHOLES_SPARKLE_DIR="$(./scripts/fetch-sparkle)" cargo test --lib --bin blackholes-mcp
```

The build prepares Sparkle and embedded frontend assets; neither command starts
the desktop app or a localhost server. A separate runtime check of the UI and
MCP bridge requires an isolated app data directory and is not covered by a
successful build/unit-test run.

## Worktree Group vocabulary and document updates

New clients discover Worktree Group tool names and `groupId` / `groupIds` inputs.
`start_worktree_group_agents` takes `groups:[{groupId,prompt}]`; list/search results
use `worktreeGroups`. Task-named tools remain callable with their existing inputs
and result keys, including `list_tasks` returning `{tasks:[...]}`. Reconnect MCP
clients after installing the updated build.

`create_worktree_group` / `update_worktree_group` accept one living `description`
and `links:[{url,label?,kind:pull_request|context}]`. The first paragraph supplies
the short summary. Agents must rewrite stale context after meaningful changes
and before finishing, preserving requirements and unresolved work without
appending a history. Use `expectedRevision` from `get_worktree_group`. A links
array replaces the whole collection; `[]` clears it. See [the complete document
API, compatibility rules and review steps](TASK-DETAILS.md).
