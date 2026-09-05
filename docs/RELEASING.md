# macOS releases and updates

Blackholes starts at version 0.1.0. `Cargo.toml` is the version source; packaging
uses it for both macOS bundle version fields. Every published update must have
a higher stable version. Do not replace an already published version's archive.

## Update behavior

The title bar shows the installed version and a check-for-updates button. A
background discovery changes it to **Update** / **Actualizar**. Sparkle 2.9.6
handles HTTPS fetching, version and architecture checks, archive signature
verification, installation, and relaunch using its native macOS interface.
Scheduled discoveries use a quiet button instead of an unsolicited update window.
Clicking the button opens Sparkle's installation dialog; downloading and restart
remain user-approved. This is not an unattended update mechanism.

Packaged releases check every four hours. There is no GitHub token embedded in
the app and system-profile reporting is disabled. GitHub receives ordinary
download requests, including the client's IP address. Updates do not work from
a bare development executable or from a private GitHub repository without public
release assets; the development button explains the packaging requirement.

Restart is deferred while agents, queued work, open terminals, authentication,
pending file edits, or unsaved notes are present. Finish agents, close terminals,
and save changes before clicking **Restart to update**. The final confirmation
also asks users to check unsent drafts. Conversation history and local project
data stay outside the app bundle; running processes and terminal output cannot
survive an application restart.

## One-time maintainer setup

Use macOS with Xcode command-line tools, the Rust toolchain, Node.js, npm, and
your Developer ID Application certificate with its private key in the Keychain.
Packaged apps include the checksum-pinned standalone Node.js 22.23.2 distribution
(Node, npm/npx, and its license), plus provider dependencies. Users do not install
Node or provider CLIs globally. On Macs without Git developer tools, the app opens
setup settings offering Apple's user-confirmed Command Line Tools installer.
Provider authentication and repository-specific development tools remain separate.

`scripts/fetch-node` downloads the matching official arm64 or x64 archive and checks
its pinned SHA-256. Packaging puts the runtime in `Contents/Resources/node`; app
launch prefers that runtime and child CLI PATHs include its bin directory. The
Node executable is signed with its own JIT entitlements; these do not apply to the
main app. Keep the pinned version/checksums current when shipping security updates.

Prepare Sparkle and generate a separate update-signing key locally:

```sh
SPARKLE_DIR=$(bash scripts/fetch-sparkle)
"$SPARKLE_DIR/bin/generate_keys" --account blackholes
```

This creates or reuses a private Ed25519 key in the login Keychain and prints
its PUBLIC key. Keep the private key safe; do not put it in GitHub, the source
tree, or release assets. Keep using the same key for future releases. The public
key must be embedded in the first release before it is distributed.

Store notarization credentials interactively; do not put a password in shell
arguments, scripts, issues, or this repository:

```sh
xcrun notarytool store-credentials blackholes-notary
```

Follow the prompts with your Apple Account, developer team ID, and an
app-specific password. The profile name is not a secret; its password is.

## Build a release

Review third-party redistribution rights before packaging, especially the
proprietary Claude Agent SDK and runtime. MPL-2.0 does not grant those rights.
The explicit acknowledgement below records this prerequisite, not legal clearance.

Commit the intended source and generated frontend assets, then run:

```sh
export BLACKHOLES_SIGNING_IDENTITY='Developer ID Application: YOUR NAME (TEAMID)'
export BLACKHOLES_SPARKLE_PUBLIC_KEY='YOUR_PUBLIC_ED25519_KEY'
export BLACKHOLES_NOTARY_PROFILE='blackholes-notary'
export BLACKHOLES_ACK_RUNTIME_LICENSES=1
node scripts/package-release.mjs
```

The script requires a clean source tree, calls `./scripts/build-release`, builds
a fresh `.app`, installs runtime dependencies without npm lifecycle scripts,
embeds Sparkle, signs nested executables inside out, submits to Apple, saves its
log, requires `Accepted`, staples the ticket, and creates the update feed and
source archive. It never publishes to GitHub or changes the repository visibility.
Review Apple's log even when the result is accepted.

If a build changes generated bundles, commit those changes and rerun packaging.
Artifacts are placed in a unique directory under `target/release-artifacts/`;
previous artifacts are not overwritten. The script builds for the current Mac's
architecture, not a universal binary. Produce each supported architecture on a
matching build machine and keep version and source commit identical.

## Publish using GitHub Releases

On https://github.com/andygeek/blackholes/releases create a release tagged
`v0.1.0` (then `v0.1.1`, etc.) at the packaged source commit. Upload the files in
the generated `github/` directory, not the staging folder or notarization logs:

- `Blackholes-VERSION-arm64.zip` and `appcast-arm64.xml` for Apple Silicon.
- `Blackholes-VERSION-x86_64.zip` and `appcast-x86_64.xml` if supporting Intel.
- `Blackholes-VERSION-source.tar.gz` and the checksum file.

If combining architectures, include both feeds and archives in the same release
and merge their checksum entries. GitHub's automatic source ZIP is not the
installable app. The generated source archive provides the corresponding source
for the distributed Blackholes code.

Finish uploading all assets before publishing the stable release and marking it
**Latest**. Each installed architecture checks its own URL:

```text
https://github.com/andygeek/blackholes/releases/latest/download/appcast-arm64.xml
https://github.com/andygeek/blackholes/releases/latest/download/appcast-x86_64.xml
```

A private repository returns an authorization/not-found error to users; making
the source public is a separate owner decision. Do not ship access tokens to
work around private assets. A missing feed, invalid signature, or download error
must not replace the installed app. Do not edit a signed archive after generating
its appcast; rebuild and regenerate instead.

## Before the first public release

Compilation is not an end-to-end updater test. On explicit authorization, verify
an installed signed older build updating to a higher signed/notarized version,
the busy-work guard, cancellation, offline/private-feed errors, and rejection of
a tampered archive. Do not advertise working public updates until the first
signed package and its public release assets exist and this flow is checked.
