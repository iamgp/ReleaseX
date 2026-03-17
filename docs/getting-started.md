# Getting Started

## Install

Build from source:

```bash
git clone https://github.com/iamgp/ReleaseX.git
cd ReleaseX
cargo build --release
./target/release/relx --version
```

Or install directly into Cargo's binary path:

```bash
cargo install --path .
```

## Repository requirements

`relx` expects:

- a Git repository
- a Python, Rust, or Go repository
- commit messages that follow Conventional Commits
- a GitHub remote for PR and release automation

Optional features add more requirements:

- `uv`, `twine`, `cargo`, or `goreleaser` depending on your ecosystem and publish setup
- `GITHUB_TOKEN` for GitHub API access
- registry credentials or trusted publishing support for package uploads

## Initialize configuration

Run:

```bash
relx init
```

This creates a starter `relx.toml` by detecting:

- the repository ecosystem (`python`, `rust`, or `go`)
- the default release branch
- GitHub owner and repository name
- version-bearing files such as `pyproject.toml`, `Cargo.toml`, `__init__.py`, or a generated `VERSION` file for Go repositories

Preview without writing:

```bash
relx init --dry-run
```

## Basic workflow

Make changes and commit using Conventional Commits:

```bash
git commit -m "feat: add async support"
git commit -m "fix: handle empty config"
```

Inspect the pending release:

```bash
relx status
relx healthcheck
```

Open or update the release PR:

```bash
relx release pr
```

After the PR is merged, tag and release from CI:

```bash
relx release tag
```

If publishing is enabled:

```bash
relx release publish
```

## Local validation

Use snapshot mode to run the release pipeline locally without pushing tags or publishing:

```bash
relx release --snapshot
```

This writes output under `.relx/snapshot/`, including:

- `CHANGELOG_ENTRY.md`
- `RELEASE_PR_BODY.md`
- `manifest.json`
- built artifacts in `.relx/snapshot/dist/`

## First commands to learn

```bash
relx validate
relx status
relx healthcheck
relx generate-ci --dry-run
relx workspace
```
