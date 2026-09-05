# Manual performance benchmark

Use release mode for all comparisons. Debug builds intentionally trade runtime speed for diagnostics.

## Baseline

1. Quit unrelated high-load applications, but keep the security software you normally use.
2. Open Activity Monitor and record Blackholes/terminal CPU and memory after 60 seconds idle.
3. Use the same shell, prompt, working directory, window size, and scrollback-producing command in Blackholes Rust and Warp.
4. Do not compare `cargo run` or `cargo build` time; compare the already-built executables.

## 30-terminal workload

1. Start the release binary.
2. Create one project or import an existing project.
3. Open 30 terminal sessions across tabs; include the same mix of Shell, Codex, Claude, and Gemini used in normal work.
4. Record idle CPU and resident memory after 60 seconds.
5. Keep one terminal focused and type continuously while several others produce output.
6. Check typing/echo latency, tab switching, scrollback, pane resizing, and close behavior.
7. Repeat the same workload in Warp.

## Repository explorer workload

1. Use the same large repository in Blackholes Rust and Warp, with generated folders such as `target` or `node_modules` present but collapsed.
2. Record the time from selecting the repository until its root entries are visible.
3. Expand the same sequence of directories, including one directory with thousands of immediate children.
4. Compare scrolling frame time, expansion latency, idle CPU, and resident memory.
5. With the explorer open, create, rename, and remove a file from a terminal and record how quickly the affected directory updates.
6. Run a command that writes many existing files without changing directory structure and confirm the explorer remains responsive and does not continuously refresh.

## Embedded editor workload

1. Open the same small source file in Blackholes Rust and Warp and compare first-paint latency, typing latency, scrolling, search, and undo/redo.
2. Repeat with a source file near 50,000 lines and record CPU, resident memory, and frame consistency.
3. Edit continuously for 30 seconds and confirm saves are coalesced rather than writing once per keypress.
4. Click a terminal in the navigation tree and confirm the editor disappears, the terminal becomes visible, and the edited file contains the final snapshot.

## What to report

- Mac model, chip, RAM, and macOS version.
- Number and type of sessions.
- Blackholes Rust and Warp idle CPU/RAM.
- Whether lag appears only during output, Git operations, notifications, or antivirus scanning.
- A short screen recording if keypress-to-echo lag is visible.

If performance regresses, capture a Time Profiler trace for the `blackholes-rust` process while reproducing it. That distinguishes UI/render time from PTY parsing and from load created by external agent processes.
