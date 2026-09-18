# Building Blackholes

[Documentation](README.md)

Run these commands from the repository root.

Requires macOS 13+, Node.js 20.19+, Git, and the stable Rust toolchain configured in `rust-toolchain.toml`.

```bash
./scripts/build-release
./target/release/blackholes-rust
```

The build script prepares frontend dependencies, generates the React bundles for the three WebViews and the lazy-loaded editor, and compiles the release binaries. It does not launch the application. Use this script for release builds so Rust embeds the current frontend assets.

On macOS it also downloads the pinned, checksum-verified Sparkle framework into
`target/` for the native updater bridge. Bare development executables cannot
self-update. Signed `.app` releases show a title-bar update button and use GitHub
Release assets; see [Releasing and updates](RELEASING.md) for packaging,
signing, notarization, and publishing prerequisites. Published macOS builds are available from [GitHub Releases](https://github.com/andygeek/blackholes/releases/latest).

On macOS, the red window button hides Blackholes without discarding its live
agents, terminals, or unsaved window state. Click the Dock icon to show the same
window again. Quitting the application is separate from hiding its window.
