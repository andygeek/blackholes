# Contributor guidance

## Scope and workflow

- Read `README.md` and `CONTRIBUTING.md` before making changes.
- Keep changes focused on the requested task and preserve unrelated work.
- Do not assume contributors share the maintainer's local tools, accounts, or
  workspace setup. Use integrations only when relevant to the requested work
  and within the user's authorization.
- Keep personal workflow preferences and machine-specific configuration outside
  shared repository instructions.

## Build

- For changes that require compilation, run `./scripts/build-release` from the
  repository root. It prepares the embedded frontend assets and dependencies
  before compiling the Rust release binaries.
- Do not substitute a direct `cargo build`: it can embed outdated frontend assets
  or omit required build preparation.
- Use `./scripts/build-frontend` for frontend-only iteration, followed by
  `./scripts/build-release` for the final build.

## Verification and sensitive data

- Follow the user's instructions about running tests, launching applications,
  starting local servers, and publishing changes.
- Report what was actually checked and any remaining limitations. Do not present
  a successful build as proof of runtime behavior.
- Never commit credentials, signing keys, private conversations, customer data,
  or local application state. Review logs and screenshots before sharing them.
