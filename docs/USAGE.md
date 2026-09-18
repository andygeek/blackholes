# Using Blackholes

[Documentation](README.md) · [Download](https://blackholes.dev/)

## Installation, projects, and sessions

Blackholes uses the agent CLIs installed on your computer: `codex`, `claude`,
`gemini`, `opencode`, and `agy` (Antigravity terminals). Install and update the
providers you use with their own tools; no provider CLI is included in the app.
Missing providers do not prevent the app or other providers from working.
Connect your own provider account in Settings → Accounts. New terminal sessions
use the selected system or isolated profile for their provider. Existing sessions
keep their profile. Usage queries plan limits for the selected account; unsupported
queries are shown as unavailable.

Terminals resolve commands after your interactive login shell starts. Provider
authentication and usage queries resolve the installed executable
using that shell's PATH, including shell-configured Homebrew and version managers.
Conventional user installation directories are fallback search locations. The
lookup runs off the UI thread and is bounded; shell functions and aliases remain
terminal features. A missing CLI produces an installation message, without
downloading a replacement. Updating a CLI takes effect on its next launch;
already-running sessions keep their current executable.

Packaged apps retain a private Node.js engine for provider account usage queries.
It is not added to your terminal PATH. These helpers read metadata through the
installed Claude and Codex CLIs without submitting prompts. Blackholes does not
bundle an agent SDK or run its own chat engine; conversation execution and history
belong to the provider CLI in each terminal.

On first launch and after updates, Blackholes also prepares its MCP connection
and routing skill in the user's Codex and Claude Code profiles. This runs inside
the app, with no installer script, CLI process, or global Node requirement. It
works with either client installed, both, or neither; prepared profiles can be
used when a client is installed later. Start a new external agent session after
setup. Provider sign-in remains separate.

On a Mac without Apple's Git command-line tools, Blackholes opens setup settings
with an installation button. Apple's installer requires user confirmation; the app
does not silently modify the system. Project-specific dependencies such as Docker,
language toolchains, or build SDKs remain requirements of the user's repositories.

New projects live under `~/Blackholes_projects` by default. Each project is a
container for repositories, project skills, `CLAUDE.md`, `AGENTS.md`, and shared instructions.
Project creation offers two modes for local repositories:

- **Link existing (default):** create symbolic links directly in the project
  root, without a `repos/` wrapper or moving/copying the originals. These links
  work in Finder, terminals and coding agents without Blackholes or its MCP.
  Existing environments and pending changes remain available.
  Subsequent edits affect the original repositories; this is not isolation.
- **Copy into project:** copy Git history and the actual working files, including
  staged/unstaged changes, untracked files, ignored `.env` files, and installed
  dependencies. Preserve the index and local Git excludes without copying Git
  hooks or linking Git storage to the original. Symbolic links remain links and
  may refer outside the copy. Stop processes that write into the source before
  copying; special files such as sockets are rejected rather than silently lost.
  This is a filesystem copy, not a portable replacement for external databases,
  containers, system dependencies, or machine-specific absolute paths.

GitHub imports always clone into a child repository folder. Selected project
skills and instructions apply to linked repositories as well as managed copies;
the project context stays in the container, not in the linked originals.
Use **Add repository** in the project's `+` menu for the same local link/copy
options or GitHub imports. A repository's `…` menu offers removal with an explicit
confirmation and the exact affected path. Removing a link preserves the original;
removing a managed repository moves its full folder to Trash (including uncommitted
changes and environment files). Emptying Trash permanently deletes those data.
Close project terminals and finish active agents first; repositories used by tasks
or other projects and project root folders are protected. Existing project paths and custom project-root settings are
preserved; changing the default is not a migration of previous workspaces.

Create a project by naming it and selecting the repositories to include. Add a
local repository or a folder containing repositories, review the detected list,
and repeat to add sources from other locations. GitHub URLs can be added one at
a time to the same list. Selected repositories are linked, copied, or cloned as
requested. If none are selected, Blackholes creates a new Git repository inside
the project container, using the container's name and an initial commit so it is
ready for coding and task worktrees. The same project form is used
over native terminals and other workspace views without closing terminal sessions.
The compact form reveals GitHub input on demand and keeps location details
collapsed. Existing database-only links are materialized when project context is
prepared on startup. Existing files and conflicting links are never overwritten.

Terminal rows use the detected provider's icon. They do not show persistent
loading dots or green presence badges; launching or focusing a CLI alone is not
treated as submitting an agent turn.

Packaged apps send macOS notifications with Blackholes' own name and icon. The
first notice requests notification permission if it has not been granted yet.
Clicking a notice brings Blackholes forward, restores its minimized window, and
opens the corresponding task or terminal. The destination
is kept in the notice so clicks can also be handled after relaunch, using normal
session restoration. A notice for a removed destination still opens the app.
Receiving a notice alone never changes the selected workspace. Bare development
executables retain in-app notices and the attention sound, but do not send macOS
notifications under Terminal's identity.

The Agents section lists terminal agent sessions. Press and hold a card by its
icon or title, then drag to reorder the list. The cursor changes
only once the hold activates reordering; the insertion marker and edge
scrolling help with long lists. Keyboard users can focus a card and use
`Option+Up` / `Option+Down`. The order survives app restarts and does not change
project/task ownership or the project tree.

Closing a terminal agent from Agents, the project tree or its terminal pane
requires confirmation. It stops the terminal and removes it from session
restoration, without deleting repository files or provider-saved conversations.
Agent close confirmations use the shared modal with the sidebar visible beneath its
dimmed, blurred backdrop, including when a native terminal is active.
Editing a project's visible name, icon and color uses that same shared modal,
preserving the sidebar and current workspace beneath the backdrop. Appearance
changes are saved only on confirmation; repository and folder names stay unchanged.

`Cmd+O` searches projects, tasks and terminal sessions (including plain shells). Search by agent/provider name, terminal title, project, task or
repository context. Terminal results open the existing session rather than creating a duplicate; project results open the session overview and task results open task details.
The palette accounts for the sidebar when centering and adapts to narrow windows.
Its search input supports `Cmd+A`, `Cmd+C`, `Cmd+X`, and `Cmd+V` through the native clipboard.

Use the `+` on a task or repository to open **Blank Terminal**, **Claude**, or
**Codex**. A task’s `…` menu contains **Edit task** and **Delete task**. The double
up-chevron above the project tree collapses all project and task accordions.

## Workspaces and task details

- Organize projects with one or more Git repositories.
- Create tasks with separate branches and worktrees for selected repositories.
- Launch, organize, and restore coding-agent sessions in visible terminals.
- Configure provider accounts, project terminal permissions, and agent instructions.
- Connect external agents to project and task management through the Blackholes MCP.
- Browse and edit files, inspect Git diffs, and search with `Cmd+O` and `Cmd+P`.
- Keep a task objective, acceptance criteria, PR link, and external task link synchronized with agents through MCP.
- Run native terminals with tabs, splits, scrollback, and session restoration.

Worktrees separate working files and branches. They are not containers or security sandboxes. The file workspace provides focused editing and diffs, not a full IDE or language-server environment.

### Task details and links

Click a task's title or choose **Edit task** to open its details. Keep the objective
in **Objective and description**, define completion conditions in **Acceptance
criteria**, and add optional **Pull request** and **External task** links. The
external link can point to ClickUp, Jira, GitHub Issues, or another HTTP(S) page;
Blackholes stores the URL without fetching or synchronizing its contents.

Use **Save changes** or `Cmd+S` while editing. Drafts survive navigation within
the current app session. If an agent changes the same task while you are editing,
the app preserves your draft and asks you to reconcile it with the latest values.
The details page also lists sessions and attached repositories.

There are no separate project/task notes rows. Project titles open a session
overview. Existing task notes appear under **Previous notes**; project notes and
all previous note files remain on disk. Keep project-wide context in project
instructions. See [Task details and MCP](TASK-DETAILS.md) for fields, update
semantics, and legacy compatibility.

### Terminal agent permissions

In **Project settings → Terminals**, enable **Start agents without permission
prompts** to opt in for that project and its tasks. It is off by default and
saved locally per project. Blackholes reads it whenever it starts or restores
a terminal agent, including after quitting and reopening the app. Existing
Claude/Codex session IDs and profiles are preserved.

| Agent | Launch flag | Reference |
| --- | --- | --- |
| Claude Code | `--dangerously-skip-permissions` | [CLI reference](https://code.claude.com/docs/en/cli-reference) |
| Codex | `--dangerously-bypass-approvals-and-sandbox` | [CLI reference](https://developers.openai.com/codex/cli/reference/) |
| Antigravity (`agy`) | `--dangerously-skip-permissions` | [Using the CLI](https://antigravity.google/docs/cli/using/) |
| OpenCode | `--auto` | [CLI reference](https://opencode.ai/docs/cli/) |
| Gemini | `--approval-mode=yolo` | [CLI reference](https://geminicli.com/docs/cli/cli-reference/) |

Use only with trusted projects: this allows file changes and commands without
confirmation. Codex also disables its sandbox; OpenCode preserves explicit deny
rules. This does not change running agents, manually typed commands,
or provider configuration files. Turning it off stops Blackholes from adding
bypass flags on subsequent launches; the provider's own settings still apply.
It does not add new session-resume support to providers.

## Local data

Application data lives in the macOS Application Support directory resolved by `src/paths.rs`.

| Storage | Contents |
|---|---|
| `blackholes-local.db` | SQLite WAL database for projects, tasks, settings, and events |
| `app-session.json` | Saved UI layout and terminal session metadata |
| `task-workspaces/` | Task worktrees |
| `agent-profiles/` | Isolated provider profiles |

Task details are stored with task records in SQLite and exported to `.blackholes-task-details.md` and `.blackholes-task.json` inside each managed task workspace. Previous Markdown notes and rich-block sidecars are preserved. Terminal output is not stored in SQLite.
Legacy built-in bot history, if present, is left on disk but is no longer read or
modified. Existing provider conversation files remain managed by their CLIs.
