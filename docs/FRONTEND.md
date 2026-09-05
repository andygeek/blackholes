# Frontend and native bridge

> Documentation status: Work in progress.

Blackholes uses React and TypeScript inside three system-WebKit views. Rust owns application state, persistence, filesystem/Git operations, PTYs, and agent processes.

## UI surfaces

| React entry | Content | Rust bridge |
|---|---|---|
| `frontend/src/navigation/main.tsx` | Sidebar, projects, tasks, agents, and menus | `src/ui/navigation_webview.rs` |
| `frontend/src/chat/main.tsx` | Chat, settings, notes, explorer, editor, and diffs | `src/ui/orchestrator_chat.rs` |
| `frontend/src/quick-open/main.tsx` | Project/task and file search overlays | `src/ui/quick_open_webview.rs` |

Shared helpers and avatars live in `frontend/src/shared/`. Settings and file views live in `chat/WorkspaceSurface.tsx`; notes use `chat/NotionNoteEditor.tsx`.

When a terminal is selected, the central WebView is hidden and GPUI renders the native terminal. Navigation remains a separate surface. Terminal output never passes through React.

The GPUI title bar shows the installed version and the update action outside all
WebViews, including settings. `src/services/updater.rs` connects to the native
Sparkle delegate in `native/updater.m`; React never downloads or installs an
executable. See [Releasing and updates](RELEASING.md).

## Communication

React sends JSON commands through `window.ipc.postMessage(...)`, using the `postNative` helper. Rust performs the operation and sends JSON events back through:

- `window.blackholesNative.receive(event)` — central workspace.
- `window.blackholesNavigation.receive(event)` — navigation.
- `window.blackholesQuickOpen.receive(event)` — quick open.

The Rust command enums define accepted payloads. Central events include `hydrate` for conversations and `workspace_surface` for settings, notes, and files. Agent text arrives incrementally through chat events.

These views load embedded HTML, not HTTP pages. The Node agent process does not serve the UI.

## Notes and files

BlockNote provides rich note editing. Changes are debounced for 650 ms and sent to Rust as blocks plus Markdown. Rust saves a rich JSON sidecar and an agent-readable Markdown file; external Markdown edits invalidate stale rich data.

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

- `assets/generated/chat.js` and `chat.css`
- `assets/generated/navigation.js`
- `assets/generated/quick-open.js`
- `assets/generated/editor.js` and `editor.css` (lazy-loaded Monaco runtime)

Rust embeds these files with `include_str!`. Keep generated bundles committed and regenerate them after frontend changes. Handwritten styles live under `assets/chat/`, `assets/navigation/`, `assets/quick-open/`, and `assets/agent-avatar.css`.

Use `lucide-react` for interface icons and give icon-only buttons accessible labels. Agent artwork belongs in the shared `AgentAvatar` component.
