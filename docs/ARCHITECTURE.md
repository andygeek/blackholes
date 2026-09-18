# Architecture

[Documentation](README.md)

Rust owns application state, local operations, and processes. React renders the visual workspace inside the system's WebKit. Terminals have a separate native rendering path.

```mermaid
flowchart TD
    UI["React / WebKit: navigation, workspace, quick open"] <-->|JSON commands and events| App["Rust / GPUI application"]
    App --> Services["Rust services: projects, task details, files, settings"]
    Services --> Data["SQLite, JSON files, Git worktrees"]
    App --> Terminal["Native terminal: GPUI + Alacritty + PTY"]
    Terminal --> Providers["Installed agent CLIs"]
    App --> Usage["Local CLI usage helpers"]
    Providers <-->|STDIO| MCP["Blackholes MCP"]
    MCP --> Services
    MCP -->|Local event socket| App
```

- **UI:** three independent React roots handle navigation, the central workspace, and quick open. HTML and production bundles are embedded in the application; the UI needs no web server.
- **Terminal:** `portable-pty` runs the shell, `alacritty_terminal` interprets terminal output, and GPUI draws it. Terminal bytes never pass through React or xterm.js.
- **Sessions:** provider CLIs run in visible native terminals. Blackholes manages their placement, profile, process lifecycle, and supported session restoration.
- **Usage:** small helpers query plan limits for the computer’s Claude and Codex accounts. Authentication stays in the provider CLI. The helpers do not run chat turns or retain conversation history.
- **MCP:** the same Rust executable runs as a STDIO MCP server with the `mcp` argument. It exposes project/task details, legacy note compatibility, navigation, terminal agent launches, and completion notifications.
- **Source Control:** `services/git_history.rs` reads bounded Git history and
  immutable commit comparisons on background workers. React draws parent-based
  graph lanes and reuses the Monaco diff surface. `ui/app/git_history.rs` guards
  asynchronous selections, watches repository and worktree Git metadata, and
  coordinates explicit fetch/push commands. Push validates the preview's branch
  and HEAD, sends one immutable commit to one branch without force or tags, and
  prevents application updates while the remote operation is running.
- **Manual commits:** `services/source_control.rs` separates index and working
  changes, stages explicit paths, and commits a locked index snapshot through
  normal Git. Request tokens cover index entries, HEAD, and branch so concurrent
  agent changes require a refresh. Hooks and signing retain their Git behavior;
  pending files are never automatically staged or restored.
- **Repository search:** `services/repository_search.rs` scans Git-listed text
  files on background workers with bounded results and cancellable requests.
  Search matches carry UTF-16 columns for selection in Monaco. Repository and
  request identifiers prevent an older query from replacing a newer result.

The UI has no localhost server. Agent models, tools, skills, and custom MCPs are
configured through the provider CLI. New terminal sessions inherit its local
account configuration. Previously saved sessions can still restore legacy
isolated profiles; Blackholes no longer creates or authenticates those profiles.

Agents resolve the intended project through the Blackholes MCP before working.
Tasks and worktrees are optional, unless the user or project instructions require
them. Task agents work in their attached worktrees and show progress in their
terminals. Long-lived processes also belong in visible terminals.

### Source map

| Location | Responsibility |
|---|---|
| `src/main.rs`, `src/ui/app.rs` | Application startup, state, navigation, and UI coordination |
| `src/services/` | Projects, Git tasks and details, files, legacy notes, persistence, terminals, account usage, and MCP integration |
| `src/ui/terminal.rs` | Native terminal input and rendering |
| `frontend/src/` | React navigation, workspace, quick open, and shared components |
| `provider-tools/` | Installed CLI metadata queries for plan usage |
| `src/bin/blackholes-mcp.rs` | Local MCP server |

The application coordinator is large, and some native rendering code remains alongside React surfaces. Workflow instructions live in MCP guidance and generated project context; keep these aligned when changing agent behavior. Startup updates known legacy task-only rules in managed project instruction blocks while preserving custom text.
