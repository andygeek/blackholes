# Contributing to Blackholes

Thank you for helping improve Blackholes. Bug reports, documentation, design
feedback, and focused code contributions are welcome.

## Discuss and prepare your change

- Search existing issues and pull requests before starting.
- Discuss large features, dependency additions, and architectural changes with
  the maintainer before investing substantial time.
- Keep pull requests focused. Explain the problem, the solution, and any
  user-visible effects; include screenshots for visual changes where useful.
- Follow [AGENTS.md](AGENTS.md), preserve unrelated changes, and respect all
  dependency and asset licenses.
- Do not commit credentials, account exports, private conversations, signing
  material, local databases, or customer data. Sanitize logs and screenshots.
- Review any AI-assisted contribution yourself. Do not submit code you cannot
  explain or do not have the right to contribute.

## License and DCO sign-off

By submitting a contribution for inclusion, you offer it under the license
applicable to the affected files. Blackholes' original code uses [MPL-2.0](LICENSE).
Third-party files retain their existing licenses. You retain the copyright you
own; no copyright assignment or additional Contributor License Agreement is
required. See [LICENSING.md](LICENSING.md).

Every commit submitted in a pull request must include a `Signed-off-by` trailer certifying
the [Developer Certificate of Origin 1.1](DCO). Read the DCO before signing.
For your own commits, Git can add the trailer with:

```sh
git commit -s -m "fix: describe the change"
```

The trailer has this form:

```text
Signed-off-by: Your Name <your-email@example.com>
```

Use an identity and email you are comfortable publishing; GitHub's no-reply
email is acceptable. Never add someone else's sign-off without their permission.
The sign-off certifies submission rights, not that you personally authored
every line of a dependency or other properly licensed material.
Maintainers review sign-offs before merging and preserve attribution and
sign-offs when combining commits. Missing sign-offs must be supplied by the
contributor. The DCO is currently a review requirement, not an automated merge gate.

## Voluntary contributions and compensation

Contributions are voluntary unless a separate written agreement provides
otherwise. Submitting or accepting a pull request does not by itself create an
employment or contractor relationship, entitlement to payment, equity,
royalties, revenue sharing, or ownership of the Blackholes business.

Any paid work, bounty, or revenue-sharing arrangement must be agreed separately
in writing before the work begins. Do not assume compensation from an issue,
feature request, review, or merged contribution.

The project may offer sponsorship options, paid support, integrations, or
separate commercial services while continuing to respect the rights granted
under its open-source licenses. A contribution does not create a right to a
share of those revenues. These policies do not waive anyone's non-waivable
rights under applicable law.

## Maintenance and decisions

Andy Eulogio Sulluchuco ([@andygeek](https://github.com/andygeek)) is the lead
maintainer. The maintainer decides the roadmap, reviews, release timing, and
repository access. Submitting a contribution does not automatically grant
merge permissions, a maintainer role, or acceptance of the change.

Contributors should receive respectful review and accurate attribution. The
project does not guarantee review times, support availability, or that every
proposed feature will be accepted. Please discuss disagreements constructively.

## Build and verification notes

Use `./scripts/build-release` for changes that require compilation. It builds
the embedded frontend assets before compiling Rust. Do not substitute a direct
`cargo build` that may embed stale assets.

Describe exactly what you built or checked in your pull request, what you did
not check, and any known limitations. Do not claim that unrun tests passed.
Automation and AI assistants must follow the repository and user instructions
about whether to run tests or start applications or local servers.
