# ReleaseX

Automated release tooling for Git repositories. `relx` handles version bumps, changelogs, release PRs, GitHub Releases, and ecosystem-specific publishing from a single binary.

ReleaseX now auto-detects Python, Rust, Go, and TypeScript repositories for config generation and build checks. Python remains the deepest publishing/workspace integration today.

Full documentation lives under [`docs/`](./docs/README.md).

![CI](https://github.com/iamgp/ReleaseX/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)

## Features

- **Conventional Commits** — derives version bumps from commit messages (`fix:` → patch, `feat:` → minor, `feat!:` → major)
- **PEP 440 versions** — full support for standard, pre-release (`a`, `b`, `rc`), post-release, and dev versions
- **Changelog generation** — auto-generates `CHANGELOG.md` in [Keep a Changelog](https://keepachangelog.com/) format
- **Release PRs** — opens and maintains a PR that accumulates changes; release happens when *you* merge it
- **GitHub Releases** — creates git tags and GitHub Releases with changelog notes on PR merge
- **Ecosystem-aware publishing** — Python via `uv` or `twine`, Rust via `cargo publish`, Go via `goreleaser`, and TypeScript via `npm publish`
- **Monorepo support** — independent versioning and release PRs for multiple packages in one repo
- **Single binary** — written in Rust, no runtime dependencies

## Installation

### From GitHub Releases

Download the latest binary for your platform:

```bash
# Linux (x86_64, static binary)
curl -L https://github.com/iamgp/ReleaseX/releases/latest/download/relx-x86_64-unknown-linux-musl -o relx
chmod +x relx
sudo mv relx /usr/local/bin/
```

### From source

```bash
cargo install --path .
```

Or build directly:

```bash
git clone https://github.com/iamgp/ReleaseX.git
cd ReleaseX
cargo build --release
# Binary at ./target/release/relx
```

## Quick Start

```bash
# 1. Initialize config in your repository
relx init

# 2. Make some commits using Conventional Commits format
git commit -m "feat: add user authentication"
git commit -m "fix: handle empty config gracefully"

# 3. Check what relx would do
relx status

# 4. Create a release PR on GitHub
relx release pr
```

## Configuration

All configuration lives in `relx.toml` at the repo root. Running `relx init` auto-detects your project layout and generates a starting config.

```toml
# ── Project type ─────────────────────────────────────────────────
[project]
ecosystem = "python"                   # "python" | "rust" | "go" | "typescript"; optional if auto-detected

# ── Release settings ─────────────────────────────────────────────
[release]
branch = "main"                         # branch to watch for new commits
tag_prefix = "v"                        # tag format: v1.2.3
changelog_file = "CHANGELOG.md"         # path to changelog file
pr_title = "chore(release): {version}"  # release PR title template

# ── Versioning ───────────────────────────────────────────────────
[versioning]
strategy = "conventional_commits"       # only supported strategy for now
initial_version = "0.1.0"              # version to use if no tags exist

# ── Version files ────────────────────────────────────────────────
# Where to read and write the version string.
# Each entry needs either `key` (for structured files) or `pattern` (for text files).

[[version_files]]
path = "pyproject.toml"
key = "project.version"                 # dotted key into the TOML structure

[[version_files]]
path = "src/mypackage/__init__.py"
pattern = '__version__ = "{version}"'   # {version} is replaced with the actual version

[[version_files]]
path = "setup.cfg"
key = "metadata.version"

# ── Changelog ────────────────────────────────────────────────────
# Map commit types to changelog sections.
# Set to false to exclude a commit type from the changelog entirely.
[changelog]
sections.feat = "Added"
sections.fix = "Fixed"
sections.refactor = "Changed"
sections.perf = "Changed"
sections.docs = false                   # excluded from changelog

# ── Publishing (opt-in) ─────────────────────────────────────────
[publish]
enabled = false                         # publishing is never on by default
provider = "uv"                         # "uv", "twine", "cargo", "goreleaser", or "npm"
repository = "pypi"                     # repository name or custom URL
# repository_url = "https://..."       # optional: explicit index URL
dist_dir = "dist"                       # directory containing built distributions
trusted_publishing = false              # enable OIDC Trusted Publisher (no token needed)
# skip_published = true                 # skip packages already on PyPI (for retries)
# username_env = "PYPI_USERNAME"        # env var for username (optional)
# password_env = "PYPI_PASSWORD"        # env var for password (optional)
# token_env = "PYPI_TOKEN"             # env var for API token (optional)

# ── GitHub ───────────────────────────────────────────────────────
[github]
# owner = "myorg"                       # auto-detected from git remote
# repo = "myproject"                    # auto-detected from git remote
api_base = "https://api.github.com"     # override for GitHub Enterprise
token_env = "GITHUB_TOKEN"             # env var to read the token from
release_branch_prefix = "relx/release" # branch name prefix for release PRs
pending_label = "autorelease: pending"  # label applied to open release PRs
tagged_label = "autorelease: tagged"    # label applied after tagging
commit_author = "github-actions[bot]"   # git author for bot-created release commits
commit_email = "41898282+github-actions[bot]@users.noreply.github.com"

# ── Monorepo ─────────────────────────────────────────────────────
[monorepo]
enabled = false                         # set to true for multi-package repos
packages = []                           # list of package directories
release_mode = "unified"                # "unified", "release_set", or "per_package"
```

## CLI Reference

### Global flags

```
--config <PATH>   Path to config file (default: relx.toml)
--dry-run         Print what would happen without making changes
--verbose         Enable debug output
--no-color        Disable ANSI colour output
```

### Commands

#### `relx init`

Generate a `relx.toml` config file by auto-detecting your project layout. Detects Python, Rust, Go, and TypeScript repositories and configures version files accordingly. Fails if a config file already exists.

```bash
relx init
relx init --dry-run   # preview the generated config without writing it
```

#### `relx status`

Analyze commits since the last release and display a summary: current version, proposed bump, next version, pending changelog entries, and package plan details.

```bash
relx status
relx status --dry-run
```

#### `relx current-version`

Print the current version for build-time injection. Uses version files, or the active tag baseline when `[promotion]` is enabled.

```bash
relx current-version
relx current-version --json
```

#### `relx lint`

Verify commits since the latest tag follow Conventional Commits. Exits non-zero on failure.

```bash
relx lint
relx lint --since=v0.7.0
```

#### `relx validate`

Parse and validate the config file. Reports the release branch and number of configured version files.

```bash
relx validate
relx validate --config path/to/relx.toml
```

#### `relx release pr`

Create or update the release PR on GitHub. The PR includes the proposed changelog entry, version bump, and is labeled `autorelease: pending`. In monorepo mode, creates one PR per package or a unified PR depending on config.

```bash
relx release pr
relx release pr --dry-run
```

For a recovery release where a published version cannot be reused, pass an exact,
stable `--next-version`. It replaces only the conventional-commit-derived version;
the normal changelog, channel range, workspace dependency, replacement, and lockfile
preparation still run. It must be newer than every selected package's current version
and cannot be combined with pre-release flags or a prerelease channel. This is a
one-off CLI/action input, not a `[versioning]` setting: do not change
`initial_version` for an existing project.

```bash
# Current source is 0.12.1; public 0.13.0 cannot be consumed, so prepare 0.14.0.
relx release pr --next-version 0.14.0
```

#### `relx release tag`

Create a git tag and GitHub Release with the changelog section as release notes. Typically called by CI after the release PR is merged. Labels the merged PR with `autorelease: tagged`.

```bash
relx release tag
relx release tag --dry-run
```

For an overridden recovery release, pass the same value to tag as an assertion.
`release tag` always derives the tag from the prepared source version and fails if it
does not equal `--next-version`, preventing a tag/source mismatch.

```bash
relx release tag --next-version 0.14.0
```

#### `relx release publish`

Publish artifacts using the configured provider. Python uses `uv` or `twine`, Rust uses `cargo`, Go uses `goreleaser`, and TypeScript uses `npm`. Requires `[publish] enabled = true` in config.

```bash
relx release publish
relx release publish --dry-run
relx release publish --skip-published  # skip packages already on PyPI (useful for retries)
```

#### `relx release preview-pr`

Preview a promotion release on the development → production PR. Requires `[promotion] enabled = true` in config. With `[[version_files]]` configured it cuts a generated `relx/promote/*` branch from `develop`, applies the version bump and changelog entry, and opens a single PR to `main` carrying code plus versioning. Without version files (tag-only) it finds or creates the `develop → main` PR directly, publishing the preview in the PR body — or in one idempotent sticky comment on pre-existing user-owned PRs. Emits `pr_number`, `version`, `tag_name`, `release_notes`, `source_sha`, and `base_sha` outputs.

```bash
relx release preview-pr
relx release preview-pr --head hotfix/login-loop --pr 42
relx release preview-pr --dry-run
```

#### `relx release release`

Cut the release for a merged promotion PR. Requires `[promotion] enabled = true` in config. Fails rather than tagging when the PR changed after its preview. Emits `release_created`, `version`, and `tag_name` outputs.

```bash
relx release release --pr 42
relx release release --dry-run
```

#### `relx release forward-port`

Open a production → development PR carrying hotfixes back to `develop`. No-ops when `develop` already contains production. Run after `release` in the production workflow.

```bash
relx release forward-port
relx release forward-port --dry-run
```

## GitHub Actions

The recommended workflow uses the `ReleaseX/action` wrapper, which downloads the correct binary for your runner — no Rust or Node runtime needed.

```yaml
# .github/workflows/release.yml
name: Release

on:
  push:
    branches: [main]

permissions:
  contents: write
  pull-requests: write
  id-token: write  # for OIDC PyPI publishing

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - uses: iamgp/ReleaseX@v1
        with:
          command: release pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  publish:
    runs-on: ubuntu-latest
    if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/')
    steps:
      - uses: actions/checkout@v4

      - uses: iamgp/ReleaseX@v1
        with:
          command: release publish
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

### Recovery version override

Use the Action's explicit `next-version` input for an audited recovery release; it
is passed as `--next-version` only to the requested relx command. Keep the same
value on the tag workflow so it verifies the merged source version.

```yaml
- uses: iamgp/ReleaseX@v1.4.1 # pin to a ReleaseX release that includes this feature
  with:
    command: release pr
    next-version: 0.14.0
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

# After that PR is merged:
- uses: iamgp/ReleaseX@v1.4.1
  with:
    command: release tag
    next-version: 0.14.0
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

## How It Works

relx follows the **Release PR model**:

1. **Scan** — on every push to main, relx analyzes new commits since the last release tag
2. **Accumulate** — it opens (or updates) a Release PR containing the proposed version bump and changelog entry
3. **Release** — when a maintainer merges the PR, CI calls `relx release tag` to create the git tag and GitHub Release
4. **Publish** — optionally, CI calls `relx release publish` to push distributions to PyPI

This gives maintainers **human-in-the-loop control** — releases only happen when you merge the PR.

### Conventional Commits

Version bumps are derived from commit messages:

| Commit type | Version bump | Example |
|---|---|---|
| `fix:` | Patch | `1.0.0` → `1.0.1` |
| `feat:` | Minor | `1.0.0` → `1.1.0` |
| `feat!:` or `BREAKING CHANGE:` | Major | `1.0.0` → `2.0.0` |

## Pre-release Versions

relx supports PEP 440 pre-release versions:

- Alpha: `1.2.0a1`
- Beta: `1.2.0b1`
- Release candidate: `1.2.0rc1`
- Post-release: `1.2.0.post1`
- Dev: `1.2.0.dev1`

Use `--pre-release` (`alpha`, `beta`, `rc`) and `--finalize` flags on the release commands to manage pre-release workflows.

## Monorepo Support

Enable monorepo mode to manage multiple Python packages in a single repository with independent versioning.

```toml
# relx.toml
[monorepo]
enabled = true
packages = [
  "packages/core",
  "packages/cli",
  "packages/sdk",
]
release_mode = "per_package"  # or "unified" / "release_set"
```

- **`unified`** — one PR covering all changed packages, one repo-style tag/release, publish the whole workspace
- **`release_set`** — one PR for whatever changed, short release titles, and publish only the packages that changed
- **`per_package`** — one release PR per changed package

Each package directory should contain its own `pyproject.toml`. relx detects which packages have changed and creates version bumps independently.

When monorepo mode is enabled, the `[[version_files]]` requirement at the top level is relaxed — version files are resolved per package instead.

## License

MIT
