# Performance contract

Blackholes Rust targets 40–100 long-lived terminals, including several coding agents producing output concurrently. This is a design target, not a verified capacity claim; the manual benchmark document describes how to measure it.

## Runtime rules

1. PTY output is push-based; idle terminals have no polling loop.
2. Every PTY reader uses a fixed 8 KiB buffer and parses VT output on its own Rust worker, outside the GPUI input/render thread.
3. Repaint requests use a capacity-one channel and an 8 ms minimum interval, so bursts are coalesced instead of growing a queue.
4. Terminal metadata events use a bounded channel.
5. Scrollback is capped at 20,000 lines per terminal.
6. Only the selected tab and its visible split terminals enter the GPUI element tree.
7. Adjacent terminal cells with the same style are shaped as one text batch; adjacent backgrounds are painted as one quad.
8. Font metrics are measured once per terminal theme/font configuration, not once per frame.
9. Parsed terminal state remains in Rust and is never serialized to React or the Node agent process.
10. Clone, duplication, and worktree operations run outside the GPUI foreground executor.
11. SQLite uses WAL mode; only metadata is stored, never terminal output.
12. Opening a repository reads only its root directory; descendants are loaded on demand when expanded.
13. Directory results are cached, and filesystem change bursts are coalesced through a capacity-one channel before refreshing expanded directories.
14. Repository changes use the platform's native filesystem watcher; the explorer adds no polling loop.
15. File reads and atomic saves stay native. React receives bounded UTF-8 document snapshots through the in-process WebView bridge; no localhost service or Node frontend server is involved.
16. Editor saves are debounced by 650 ms and run outside the GPUI foreground executor. Leaving a dirty file forces one final atomic snapshot.
17. Editable documents are bounded to 8 MiB and 50,000 lines before they cross into the React workspace.
18. The terminal element tree is absent while a file, project note, or task note is visible; PTYs continue running without being rendered.
19. Only the repository shown in the open explorer has a filesystem watcher or live Git-change refresh; repository count does not multiply background Git work.
20. Git-change events ignore `.git`, are coalesced for 350 ms in a bounded 2,048-path batch, and allow only one status or selected-file diff request in flight. Identical results do not trigger a repaint.
21. React virtualizes visible diff rows, and displayed diffs are capped at 20,000 rows.

## Intended measurements

- Input-to-echo latency in the focused terminal while 0, 25, 50, and 99 other terminals stream.
- Frame time while one visible terminal streams and the others are hidden.
- CPU while all terminals are idle.
- Resident memory per additional shell and per additional agent.
- Switching to a terminal with 20,000 lines of history.
- Closing a terminal and an entire tab under output load.
- Opening a large repository and expanding a directory with thousands of immediate children.
- File-tree update latency and CPU during structural changes versus content-only write bursts.
- Opening, scrolling, editing, and saving a 50,000-line source file through the React editor.
- Keeping a diff open while unrelated files change continuously, verifying bounded Git processes and no unchanged-result repaint loop.

The visual workspace uses three system-WebKit React roots: navigation, the central workspace, and quick open. The selected terminal replaces the central view with native GPUI rendering. The UI requires no local HTTP server; the Node process runs agent adapters, and OpenCode starts its own local agent server. External agents, shells, language servers, compilers, and antivirus software can dominate total system use.
