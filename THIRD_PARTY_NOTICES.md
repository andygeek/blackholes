# Third-party notices

Blackholes' original source is covered by [MPL-2.0](LICENSE). This does not
replace the following third-party licenses or any notices in individual files.
See [LICENSING.md](LICENSING.md) for scope and redistribution responsibilities.

## Sparkle

The macOS updater integrates Sparkle 2.9.6 under its MIT-style license and
bundled third-party terms. The full distribution notice is preserved at
[licenses/SPARKLE.txt](licenses/SPARKLE.txt) and copied into packaged releases.
The framework is fetched from the official Sparkle release with a pinned SHA-256
checksum; it is not checked into this repository.

## Monaco Editor

The embedded file editor includes Monaco Editor under the
[MIT License](licenses/MONACO.txt), together with its
[third-party notices](licenses/MONACO-THIRD-PARTY.txt).
The exact package version is recorded in `frontend/package-lock.json`.

## Geist font

The bundled Geist Mono font retains the
[SIL Open Font License](assets/fonts/OFL-Geist.txt).

## Provider brand assets

The OpenCode symbols in `assets/icons/opencode.svg` and `opencode-dark.svg`
come from the [official brand assets](https://opencode.ai/brand), distributed in
[anomalyco/opencode](https://github.com/anomalyco/opencode/tree/dev/packages/console/app/src/asset/brand)
under the [MIT License](licenses/OPENCODE.txt). Redundant SVG masks and clip paths
were removed; the original geometry and light/dark colors are retained.

`assets/icons/antigravity.png` is the unmodified full-color icon from
[Google Antigravity's official press assets](https://antigravity.google/press),
downloaded from https://antigravity.google/assets/image/brand/antigravity-icon__full-color.png.
These brand marks identify the corresponding third-party tools; their trademarks
belong to their respective owners and are not covered by Blackholes' MPL license.
Their inclusion does not imply affiliation or endorsement.

## React and Lucide icons

The embedded WebKit navigation and workspace bundles include React and React DOM,
licensed under the MIT License, and Lucide icons through `lucide-react`,
licensed under the ISC License. Their source packages and complete license
texts are available from their respective npm distributions.


## Unicode character data

`src/ui/terminal/unicode_strokes.rs` contains semantic stroke weights generated
from the [Unicode 17.0.0 Character Database](https://www.unicode.org/Public/17.0.0/ucd/UnicodeData.txt).
The data is covered by the [Unicode License V3](licenses/UNICODE.txt).
The generator is `scripts/generate-terminal-glyph-data.mjs`. No font outlines
are included in the generated data.

## Rust dependencies

The vendored GPUI Component library retains its Apache-2.0 license at
`vendor/gpui-component/LICENSE-APACHE`. The terminal uses `alacritty_terminal`
under Apache-2.0. Other dependencies retain the licenses and notices supplied
in their respective source distributions. `Cargo.lock` records their versions.

## Node runtime

Packaged macOS apps include the official Node.js 22.23.2 engine for provider account metadata queries.
Its license and third-party notices are copied from the verified upstream
distribution to `Contents/Resources/node/LICENSE` and `licenses/NODE.txt` in the app.
The archive version and SHA-256 values are pinned in `scripts/fetch-node`.

Provider CLIs and agent SDKs are not distributed with Blackholes.
