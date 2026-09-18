# macOS release procedure

This guide covers building, signing, notarizing, and publishing Blackholes.
Run commands from the repository root unless a step says otherwise. Examples
use placeholders and shell variables; supply values for the build environment.
Keep those variables in the same shell when continuing between sections, and
stop if a command or validation fails.

## Tooling and local configuration

The build machine needs macOS, Xcode command-line tools, the Rust toolchain
specified in `rust-toolchain.toml`, Node.js/npm as specified in [Building](BUILDING.md), Git,
and GitHub CLI (`gh`) authenticated for the release repository.

Packaging uses the current Node process architecture: `arm64`, or `x64` mapped
to the asset suffix `x86_64`. It does not produce a universal binary. Use a
matching build environment for each architecture and the same source commit
and version when combining their assets in one release.

Keep signing keys, notarization credentials, exported certificates with private
keys, account identifiers, and machine-specific configuration outside Git.
A local environment file may be used if it is ignored by Git. Review
`git diff --cached` before committing; `.gitignore` does not protect a file that
is already tracked. Notarization logs and build artifacts also stay local.

### Developer ID certificate

Install a **Developer ID Application** certificate with its matching private
key in the build machine's Keychain. An Apple Development certificate or a
certificate without its private key cannot substitute for this distribution
identity. Supply the identity through `BLACKHOLES_SIGNING_IDENTITY`; do not put
an actual person's name or team ID in this guide.

### Notarization credentials

Choose a local Keychain profile name and configure it interactively:

```sh
export BLACKHOLES_NOTARY_PROFILE='YOUR_LOCAL_PROFILE_NAME'
xcrun notarytool store-credentials "$BLACKHOLES_NOTARY_PROFILE"
```

Reuse that profile on subsequent releases. Enter credentials through the
interactive prompt rather than command arguments or committed files. Keychain
access dialogs and Apple Account authentication are separate operations.

### Sparkle signing key

Sparkle uses a separate update-signing key. The packaging script uses the
Keychain account name `blackholes`:

```sh
sparkle_dir=$(bash scripts/fetch-sparkle)
"$sparkle_dir/bin/generate_keys" --account blackholes -p
```

This reads the public key. Only for initial setup, use `generate_keys --account
blackholes` without `-p` to create or reuse a key. Preserve the private key across
releases: installed applications trust the public key embedded in their bundle.
If moving to another build machine, recover the existing key securely.

Set the packaging inputs locally:

```sh
export BLACKHOLES_SIGNING_IDENTITY='Developer ID Application: YOUR_NAME (YOUR_TEAM_ID)'
export BLACKHOLES_SPARKLE_PUBLIC_KEY='YOUR_SPARKLE_PUBLIC_KEY'
export BLACKHOLES_ACK_RUNTIME_LICENSES=1
```

The script verifies that the public key matches the Keychain key. The public
key is embedded in distributed apps; the private key remains in the Keychain.
Review `LICENSING.md`, `THIRD_PARTY_NOTICES.md`, and the bundled components'
licenses before setting the acknowledgment variable.

## Prepare the version and source

Use the GitHub repository configured in `scripts/package-release.mjs` and the
application's update feed. The placeholder below must match that destination;
changing a shell variable does not reconfigure the application or packager.

```sh
release_repository='OWNER/REPOSITORY'
repo_root=$(git rev-parse --show-toplevel)
git status --short
git branch --show-current
git remote -v
gh release list --repo "$release_repository"
```

Choose an unused stable version greater than the published version. Update:

- The root package version in `Cargo.toml`.
- The `blackholes-rust` package version in `Cargo.lock`.
- The displayed application version in `README.md`.

Do not replace dependency versions. `Cargo.toml` supplies the application and
bundle versions; frontend package versions are independent. Write public
release notes in `docs/releases/VERSION.md` and commit them with the release
source. Include changes, installation instructions, known limitations, and
only the checks actually performed. Keep private operational notes outside Git.

Build using the repository entry point:

```sh
./scripts/build-release
git status --short
git diff --stat
git diff --check
```

The build prepares frontend dependencies and embedded assets before compiling
Rust. Do not replace it with a direct `cargo build`. Review and commit the
intended source, release notes, version files, and generated bundles, following
`CONTRIBUTING.md` and `DCO`. Stage paths explicitly so unrelated changes are not
included. Packaging requires a clean tree, including untracked files.

Record the committed version and source:

```sh
release_version=$(node -e 'const text = require("node:fs").readFileSync("Cargo.toml", "utf8"); const match = text.match(/^version = "([0-9]+\.[0-9]+\.[0-9]+)"$/m); if (!match) process.exit(1); console.log(match[1]);')
release_tag="v${release_version}"
release_arch=$(node -p 'process.arch === "x64" ? "x86_64" : process.arch')
release_commit=$(git rev-parse HEAD)
release_notes="${repo_root}/docs/releases/${release_version}.md"
git status --porcelain
```

The status output must be empty. Keep the recorded commit unchanged throughout
packaging and publication. Existing published tags and archives remain intact.

## Package, sign, and notarize the app

```sh
node scripts/package-release.mjs
```

The script performs these steps automatically:

1. Checks the version, clean source tree, signing configuration, and Sparkle key.
2. Fetches checksum-pinned runtime/framework distributions.
3. Runs `./scripts/build-release` and rejects generated changes to the source tree.
4. Creates a fresh `.app` with its resources, licenses, and Sparkle framework.
5. Signs nested executables and frameworks, then the app, using hardened runtime
   and timestamps. The bundled Node executable has its own entitlements.
6. Verifies signatures, uploads a temporary app ZIP to Apple, and waits for a result.
7. Retrieves the notarization log, requires `Accepted`, and staples and validates
   the app's ticket.
8. Creates the update ZIP from the stapled app, generates its signed appcast,
   archives the corresponding source commit, and writes initial checksums.

The script does not create a DMG, create or push Git tags, upload to GitHub,
publish a release, or install the application. Those are separate steps below.
It creates a unique output directory under `target/release-artifacts/` and prints
the path. Record that exact directory, replacing the placeholder:

```sh
release_output="${repo_root}/target/release-artifacts/ACTUAL_OUTPUT_DIRECTORY"
release_app="${release_output}/staging/Blackholes.app"
release_downloads="${release_output}/github"
release_dmg="${release_downloads}/Blackholes-${release_version}-${release_arch}.dmg"
```

Review the output before continuing:

```sh
plutil -p "$release_app/Contents/Info.plist"
cat "$release_app/Contents/Resources/SOURCE.txt"
cat "$release_output/notarization.json"
cat "$release_output/notarization-log.json"
codesign --verify --deep --strict --verbose=2 "$release_app"
xcrun stapler validate "$release_app"
git rev-parse HEAD
git status --porcelain
```

Require the intended version, source commit, bundle identifier, feed URL, and
public key; Apple status `Accepted`; valid signatures and ticket; unchanged
HEAD; and a clean tree. Review Apple's reported issues even if the submission
was accepted. Keep raw logs local because they may contain account or build
machine details.

If the build changed generated assets, review and commit them, record the new
source commit, and package again. Do not bypass the clean-tree check.

## Create and notarize a DMG

Use the app from the exact successful packaging output. Do not use an installed
copy or a bundle from another attempt. Choose a new DMG path if one already
exists; do not overwrite a previously published artifact.

```sh
dmg_staging=$(mktemp -d "${TMPDIR:-/tmp}/blackholes-dmg.XXXXXXXX")
ditto "$release_app" "$dmg_staging/Blackholes.app"
ln -s /Applications "$dmg_staging/Applications"
hdiutil create -volname Blackholes -srcfolder "$dmg_staging" -format UDZO "$release_dmg"
codesign --force --sign "$BLACKHOLES_SIGNING_IDENTITY" --timestamp "$release_dmg"
xcrun notarytool submit "$release_dmg" \
  --keychain-profile "$BLACKHOLES_NOTARY_PROFILE" \
  --wait --output-format json > "$release_output/dmg-notarization.json"
```

Review the result and require `Accepted`. Copy its submission ID into a local
variable, retrieve the log, then staple and validate the DMG:

```sh
cat "$release_output/dmg-notarization.json"
dmg_submission_id='SUBMISSION_ID_FROM_RESULT'
xcrun notarytool log "$dmg_submission_id" \
  --keychain-profile "$BLACKHOLES_NOTARY_PROFILE" \
  "$release_output/dmg-notarization-log.json"
cat "$release_output/dmg-notarization-log.json"
xcrun stapler staple "$release_dmg"
xcrun stapler validate "$release_dmg"
codesign --verify --verbose=2 "$release_dmg"
```

App and DMG notarization are separate submissions. Stop on an invalid or pending
submission, failed signature verification, or failed ticket validation.

### Continue waiting for an existing submission

An interrupted wait does not require another upload. Retain the submission ID
from the result or command output and query or wait for that submission:

```sh
submission_id='EXISTING_SUBMISSION_ID'
xcrun notarytool info "$submission_id" \
  --keychain-profile "$BLACKHOLES_NOTARY_PROFILE"
xcrun notarytool wait "$submission_id" \
  --keychain-profile "$BLACKHOLES_NOTARY_PROFILE" \
  --timeout 30m --output-format json
```

Apple continues processing if this wait times out. After acceptance, retrieve
the log and continue with the remaining packaging steps for those exact bytes.
The package script currently has no checkpoint/resume option; rerunning it
starts a new packaging attempt. Do not treat a successful resumed Apple wait
as evidence that the script's remaining steps have run.

## Verify the appcast and final artifacts

The package script generates `appcast-ARCH.xml` while its download directory
contains the update ZIP. It adds the source archive afterward. Do not rerun
`generate_appcast` against a folder containing mixed source, DMG, and update
archives without isolating the intended update payload.

Inspect the generated XML for the version, minimum macOS version, architecture,
ZIP URL, exact archive length, and Ed25519 signature. The update enclosure points
to the final ZIP containing the stapled app. The temporary notarization ZIP and
manual-install DMG are different files.

Do not modify the update ZIP after generating its feed. Compute final checksums
after DMG stapling, which changes its bytes:

```sh
(
  cd "$release_downloads" || exit 1
  shasum -a 256 \
    "Blackholes-${release_version}-${release_arch}.dmg" \
    "Blackholes-${release_version}-${release_arch}.zip" \
    "Blackholes-${release_version}-source.tar.gz" \
    "appcast-${release_arch}.xml" > SHA256SUMS.txt
  shasum -a 256 -c SHA256SUMS.txt
)
```

Each entry must report `OK`. Publish only these artifacts:

| Asset | Purpose |
| --- | --- |
| `Blackholes-VERSION-ARCH.dmg` | Manual installer containing the app and an Applications shortcut |
| `Blackholes-VERSION-ARCH.zip` | Sparkle update payload containing the stapled app |
| `appcast-ARCH.xml` | Signed update metadata referencing the ZIP |
| `Blackholes-VERSION-source.tar.gz` | Corresponding source from the packaged commit |
| `SHA256SUMS.txt` | Checksums of the release artifacts, excluding itself |

Do not upload staging directories, notarization submissions or logs, private
configuration, account profiles, or application data. GitHub's automatic source
archives are separate from the installable app.

## Create a tag and GitHub draft

Confirm the intended source commit has been published to the release repository
and that local HEAD still matches it. Push only the reviewed source commits
using the repository's normal contribution workflow. Then create and push the
version tag to the corresponding remote:

```sh
git rev-parse HEAD
git status --porcelain
git tag "$release_tag" "$release_commit"
git push origin "refs/tags/${release_tag}"
```

If the tag exists or the push fails, inspect the local and remote state before
continuing. Do not replace an existing published tag or force-push release history.

Create a draft with the explicit asset list:

```sh
gh release create "$release_tag" \
  "$release_dmg" \
  "$release_downloads/Blackholes-${release_version}-${release_arch}.zip" \
  "$release_downloads/appcast-${release_arch}.xml" \
  "$release_downloads/Blackholes-${release_version}-source.tar.gz" \
  "$release_downloads/SHA256SUMS.txt" \
  --repo "$release_repository" --verify-tag --target "$release_commit" \
  --title "Blackholes ${release_version}" --notes-file "$release_notes" --draft

gh release view "$release_tag" --repo "$release_repository" \
  --json tagName,targetCommitish,isDraft,isPrerelease,assets,url
git ls-remote origin "refs/tags/${release_tag}"
```

Require the recorded source tag and all expected assets. Check their names,
sizes, and checksums, and review the public release notes. If an upload was
interrupted, inspect the draft and use `gh release upload` for the missing
verified assets. Do not blindly overwrite existing uploads or publish a partial
draft.

For multiple architectures, build each from the same version and source commit.
Combine their installers, update ZIPs, appcasts, and final checksums in the same
release, with one corresponding source archive.

## Publish and check distribution

Once the draft is complete, publish it as the stable Latest release:

```sh
gh release edit "$release_tag" --repo "$release_repository" --draft=false --latest
gh api "repos/${release_repository}/releases/latest" \
  --jq '{tag_name, draft, prerelease, assets: [.assets[] | {name, size, digest, browser_download_url}]}'
```

Confirm the returned tag and asset list. The updater's stable feed URL uses
`releases/latest/download/appcast-ARCH.xml`; both the feed and its ZIP URL must
be publicly accessible without authentication. Do not put GitHub credentials
in the application or feed.

If a download page pins a release URL, update it after the asset is public using
that website repository's deployment instructions. Verify the deployed link;
a source commit alone does not establish that the website deployed successfully.

Publish a higher version to correct a distributed build. Replacing an existing
signed archive or version tag breaks source, signature, and update consistency.

## Runtime validation and cleanup

Compilation, signature verification, and Apple acceptance do not prove runtime
behavior. Record which checks were actually performed, including any manual
installation, Gatekeeper, provider startup, update from an older signed build,
cancellation, restart guards, and offline/error handling. Exercise signature
rejection only with isolated test artifacts, never by changing public assets.
Follow the task's authorization for running tests, launching apps, installing
updates, or changing an installed application.

Retain final artifacts and local notarization records. Remove temporary staging
copies only by their recorded paths after they are no longer needed. Leave the
installed application, application data, provider profiles, repositories,
worktrees, and signing keys out of build cleanup.

## Troubleshooting

| Condition | Next step |
| --- | --- |
| Packaging rejects a dirty tree | Review and commit intended source and generated files; keep private configuration outside tracked files. |
| Signing identity is unavailable | Check the distribution certificate, matching private key, and Keychain access on the build machine. |
| Notarization authentication fails | Repair the Keychain profile interactively; do not place credentials in logs or examples. |
| Apple reports `In Progress` | Query or wait for the existing submission ID without uploading it again. |
| Apple reports `Invalid` | Retrieve the submission log, correct the reported package/signature issues, and build new artifacts. |
| Sparkle public key does not match | Use the existing update-signing key trusted by installed clients. |
| Feed or download returns 404 | Check publication state, repository visibility, Latest selection, and exact asset names. |
| No update is offered | Compare the installed and feed versions, architecture, minimum OS, and configured feed URL. |
| Restart is deferred | Save pending work and close active sessions before retrying. |
| Download page still links to an older installer | Check its deployed URL and deployment result. |

## Implementation reference

| Source | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `README.md`, `docs/BUILDING.md` | Version and build requirements |
| `scripts/build-release`, `scripts/build-frontend` | Compilation and embedded frontend assets |
| `scripts/package-release.mjs` | Packaging, signing, app notarization, ZIP, appcast, source, and initial checksums |
| `scripts/fetch-node`, `scripts/fetch-sparkle` | Pinned distributions and checksum verification |
| `assets/node-entitlements.plist` | Entitlements for the bundled Node runtime |
| `native/updater.m`, `src/services/updater.rs`, `src/ui/app.rs` | Update integration and restart coordination |
| `src/paths.rs` | Local application data paths |
| `CONTRIBUTING.md`, `DCO` | Contribution and commit requirements |
| `LICENSING.md`, `THIRD_PARTY_NOTICES.md`, `licenses/` | Distribution notices |
