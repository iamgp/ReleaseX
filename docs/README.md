# ReleaseX Documentation

This directory contains the full product documentation for ReleaseX and the `relx` CLI.

## Guides

- [Getting Started](./getting-started.md)
- [Configuration Reference](./configuration.md)
- [Command Reference](./commands.md)
- [CI and Automation](./ci.md)
- [Publishing](./publishing.md)
- [Channels and Pre-releases](./channels.md)
- [Monorepos and uv Workspaces](./workspaces.md)
- [Rust and Go Roadmap](./ecosystem-roadmap.md)
- [Troubleshooting and Operations](./troubleshooting.md)

## What ReleaseX Does

`relx` automates releases for Git repositories. Today it is strongest on Python workflows, and the planned ReleaseX expansion adds first-class Rust and Go ecosystem support on top of the same core release engine.

The release model is intentionally conservative:

1. Commits accumulate on the release branch.
2. `relx release pr` prepares the next release as a PR.
3. A maintainer reviews and merges the PR.
4. CI runs `relx release tag`.
5. CI optionally runs `relx release publish`.

This preserves human approval for every release while removing the repetitive mechanics.
