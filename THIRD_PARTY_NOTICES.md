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

## React and Lucide

The embedded WebKit navigation and chat bundles include React and React DOM,
licensed under the MIT License, and Lucide icons through `lucide-react`,
licensed under the ISC License. Their source packages and complete license
texts are available from their respective npm distributions.

## BlockNote and Mantine

The React note workspace includes BlockNote (`@blocknote/core`,
`@blocknote/react`, and `@blocknote/mantine`) under the Mozilla Public License
2.0. BlockNote's Mantine integration uses `@mantine/core` and `@mantine/hooks`,
which are licensed under the MIT License. Complete license texts and source
metadata are included in their npm distributions.

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

## Agent runtimes

Packaged macOS apps include the official Node.js 22.23.2 runtime and npm/npx.
Their bundled license and third-party notices are copied from the verified upstream
distribution to `Contents/Resources/node/LICENSE` and `licenses/NODE.txt` in the app.
The archive version and SHA-256 values are pinned in `scripts/fetch-node`.

The Claude Agent SDK and its platform binaries are proprietary Anthropic
components, governed by the legal agreements referenced in their distributed
`LICENSE.md` files and [Anthropic's legal documentation](https://code.claude.com/docs/en/legal-and-compliance).
They are not covered by any license chosen for Blackholes' own source code.
The other SDKs and runtimes retain their package licenses; exact versions are
recorded in `agent-sidecar/package-lock.json`.
