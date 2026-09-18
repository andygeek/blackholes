# Frontend and native bridge

> Documentation status: Work in progress.

Blackholes uses React and TypeScript inside three system-WebKit views. Rust owns application state, persistence, filesystem/Git operations, PTYs, and agent processes.

## UI surfaces

| React entry | Content | Rust bridge |
|---|---|---|
| `frontend/src/navigation/main.tsx` | Sidebar, projects, tasks, agents, and menus | `src/ui/navigation_webview.rs` |
| `frontend/src/workspace/main.tsx` | Home, project overview, task details, settings, explorer, editor, diffs, and shared overlays | `src/ui/workspace_webview.rs` |
| `frontend/src/quick-open/main.tsx` | Project/task and file search overlays | `src/ui/quick_open_webview.rs` |

Shared helpers and provider icons live in `frontend/src/shared/`. Settings and file views live in `workspace/WorkspaceSurface.tsx`; task details and the project session overview use `workspace/TaskDetails.tsx`.

When a terminal is selected, the central WebView is hidden and GPUI renders the native terminal. Navigation remains a separate surface. Terminal output never passes through React.

Agents and Projects have independent section toggles. Their collapsed state is
stored in `AppSession` through `set_sidebar_section_collapsed` and included in
navigation hydration; older sessions default to expanded. Hidden content stays
mounted, and the grid reallocates its space to the remaining open section.
The toggle buttons expose `aria-expanded` and `aria-controls`; section folding
does not change the expanded project/task IDs or stop terminal sessions.

The GPUI title bar shows the installed version and the update action outside all
WebViews, including settings. `src/services/updater.rs` connects to the native
Sparkle delegate in `native/updater.m`; React never downloads or installs an
executable. See [Releasing and updates](RELEASING.md).

The native bottom bar (`src/ui/usage_bar.rs`) is also outside the WebViews.
It displays local Claude/Codex account limits through independent background
queries at startup and on manual refresh. Reset countdowns update locally each
minute. Its height is reserved by the root layout, so it does not cover terminal
rows or React content. The inline value prefers a valid general five-hour limit,
then weekly, then another reported limit, always using its actual label.
The native popup above each provider includes only limits with valid usage
percentages and sizes itself to those sections. The refresh button supplies
its own arrow as the loading icon so it stays visible while rotating. A separate
window keeps the panel above WKWebViews without hiding the workspace. It closes
on focus loss, Escape, or actual parent window
geometry changes. GPUI bounds notifications caused only by transferring focus
to the panel are ignored. App shortcuts and Dock reopening continue to target
the main window.
The right side displays open terminal handles and current-process resident
memory from macOS libproc, sampled every five seconds. It excludes subprocesses.
The terminal container has no focus-colored outline.

## Communication

React sends JSON commands through `window.ipc.postMessage(...)`, using the `postNative` helper. Rust performs the operation and sends JSON events back through:

- `window.blackholesNative.receive(event)` — central workspace.
- `window.blackholesNavigation.receive(event)` — navigation.
- `window.blackholesQuickOpen.receive(event)` — quick open.

The Rust command enums define accepted payloads. Central events include `workspace_surface` for home, settings, project overview, task details, and files, plus `app_modal` and `quick_open` for overlays. The quick-open input sends clipboard requests through the native bridge, using the open ID and request ID to reject stale paste responses.

These views load embedded HTML, not HTTP pages. Provider usage helpers do not serve the UI or execute chat turns.

## Task details and files

Task metadata uses plain text fields and explicit saves. `save_task_details` sends
a patch and the metadata revision; `task_details_saved` acknowledges the request
with canonical values or an error. The desktop and MCP use the same validation
and database edit path. React keeps unsaved drafts across navigation, preserves
them on conflicts, and tracks unsaved state for the updater guard. Neither side
replaces the task's sessions or repository membership when editing metadata.

Legacy task Markdown is displayed read-only when present. The rich note editor
and its BlockNote/Mantine dependencies have been removed. Existing Markdown and
sidecar files remain available for compatibility. See [Task details](TASK-DETAILS.md).

The explorer loads directories on demand through Rust and receives filesystem updates. File reads, size limits, and atomic saves stay in Rust. React handles editing and the virtualized Git diff view.

## Build

```bash
./scripts/build-release
```

This installs missing dependencies, prepares the pinned Sparkle headers on macOS,
type-checks and builds the React entries and lazy editor, then compiles Rust
release binaries. For frontend bundles only:

```bash
./scripts/build-frontend
```

Vite targets Safari 16 and emits self-contained IIFE bundles:

- `assets/generated/workspace.js`
- `assets/generated/navigation.js`
- `assets/generated/quick-open.js`
- `assets/generated/editor.js` and `editor.css` (lazy-loaded Monaco runtime)

Rust embeds these files with `include_str!`. Keep generated bundles committed and regenerate them after frontend changes. Handwritten styles live under `assets/workspace/`, `assets/navigation/`, `assets/quick-open/`.

Use `lucide-react` for interface icons and give icon-only buttons accessible labels. Provider logos belong in the shared `TerminalProviderIcon` component.
