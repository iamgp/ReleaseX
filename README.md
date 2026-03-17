# ReleaseX

Automated release tooling for Git repositories. `relx` handles version bumps, changelogs, release PRs, GitHub Releases, and ecosystem-specific publishing from a single binary.

ReleaseX now auto-detects Python, Rust, and Go repositories for config generation and build checks. Python remains the deepest publishing/workspace integration today.

Full documentation lives under [`docs/`](./docs/README.md).

<!-- badges -->
<!-- ![CI](https://github.com/OWNER/ReleaseX/actions/workflows/ci.yml/badge.svg) -->
<!-- ![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg) -->

## Features

- **Conventional Commits** — derives version bumps from commit messages (`fix:` → patch, `feat:` → minor, `feat!:` → major)
- **PEP 440 versions** — full support for standard, pre-release (`a`, `b`, `rc`), post-release, and dev versions
- **Changelog generation** — auto-generates `CHANGELOG.md` in [Keep a Changelog](https://keepachangelog.com/) format
- **Release PRs** — opens and maintains a PR that accumulates changes; release happens when *you* merge it
- **GitHub Releases** — creates git tags and GitHub Releases with changelog notes on PR merge
- **Ecosystem-aware publishing** — Python via `uv` or `twine`, Rust via `cargo publish`, with Go packaging still evolving
- **Monorepo support** — independent versioning and release PRs for multiple packages in one repo
- **Single binary** — written in Rust, no runtime dependencies

## Installation

### From GitHub Releases

Download the latest binary for your platform:

```bash
# Linux (x86_64)
curl -L https://github.com/OWNER/ReleaseX/releases/latest/download/relx-linux-x86_64 -o relx
chmod +x relx
sudo mv relx /usr/local/bin/
```

### From source

```bash
cargo install --path .
```

Or build directly:

```bash
git clone https://github.com/OWNER/ReleaseX.git
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
ecosystem = "python"                   # "python" | "rust" | "go"; optional if auto-detected

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
provider = "uv"                         # "uv", "twine", or "cargo"
repository = "pypi"                     # repository name or custom URL
# repository_url = "https://..."       # optional: explicit index URL
dist_dir = "dist"                       # directory containing built distributions
trusted_publishing = false              # enable OIDC Trusted Publisher (no token needed)
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

# ── Monorepo ─────────────────────────────────────────────────────
[monorepo]
enabled = false                         # set to true for multi-package repos
packages = []                           # list of package directories
release_mode = "unified"                # "unified" (one PR) or "per_package" (one PR each)
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

Generate a `relx.toml` config file by auto-detecting your project layout. Detects Python, Rust, and Go repositories and configures version files accordingly. Fails if a config file already exists.

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

#### `relx release tag`

Create a git tag and GitHub Release with the changelog section as release notes. Typically called by CI after the release PR is merged. Labels the merged PR with `autorelease: tagged`.

```bash
relx release tag
relx release tag --dry-run
```

#### `relx release publish`

Publish distributions to PyPI (or a custom index) using the configured provider (`uv` or `twine`). Requires `[publish] enabled = true` in config.

```bash
relx release publish
relx release publish --dry-run
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

      - uses: ReleaseX/action@v1
        with:
          command: release pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  publish:
    runs-on: ubuntu-latest
    if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/')
    steps:
      - uses: actions/checkout@v4

      - uses: ReleaseX/action@v1
        with:
          command: release publish
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

Pre-release and finalization support is planned via `--pre-release` and `--finalize` flags on the release commands (see the [PRD](prd.md) for roadmap details).

## Monorepo Support

Enable monorepo mode to manage multiple Python packages in a single repository with independent versioning.

```toml
# ReleaseX.toml
[monorepo]
enabled = true
packages = [
  "packages/core",
  "packages/cli",
  "packages/sdk",
]
release_mode = "per_package"  # or "unified"
```

- **`per_package`** — one release PR per changed package
- **`unified`** — one PR covering all changed packages

Each package directory should contain its own `pyproject.toml`. relx detects which packages have changed and creates version bumps independently.

When monorepo mode is enabled, the `[[version_files]]` requirement at the top level is relaxed — version files are resolved per package instead.

## License

MIT
