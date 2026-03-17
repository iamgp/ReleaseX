# CI and Automation

## Generate a workflow

Start with:

```bash
relx generate-ci
```

Preview first:

```bash
relx generate-ci --dry-run
```

`relx` reads:

- `relx.toml`
- `pyproject.toml`
- publishing settings
- monorepo settings

It chooses a build step based on the detected backend and includes a publish job when publishing is enabled.

## Typical GitHub Actions workflow

```yaml
name: Release

on:
  push:
    branches: [main]

permissions:
  contents: write
  pull-requests: write
  id-token: write

jobs:
  release-pr:
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
    if: startsWith(github.ref, 'refs/tags/')
    steps:
      - uses: actions/checkout@v4
      - uses: astral-sh/setup-uv@v5
      - run: uv build
      - uses: ReleaseX/action@v1
        with:
          command: release publish
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

## Required GitHub permissions

For PR and release automation:

- `contents: write`
- `pull-requests: write`

For PyPI OIDC trusted publishing:

- `id-token: write`

## Recommended pipeline shape

1. On push to the release branch, run `relx release pr`.
2. On tag creation, run `relx release publish`.
3. Optionally run `relx healthcheck` and `relx status --json` in CI for observability.

If you use channels for both stable and beta releases, trigger on both branches:

```yaml
on:
  push:
    branches: [main, beta]
```

Then:

- pushes to `beta` produce beta release PRs and beta tags
- pushes to `main` produce stable release PRs and stable tags

No special GitHub Action input is required; the branch and channel config drive the behavior.

## Maturin projects

When the build backend includes `maturin`, `relx generate-ci` emits a `maturin-action` build step instead of a plain `uv build`.

## Existing workflow files

If the configured workflow file already exists and differs from the generated output, `relx`:

1. prints a line-oriented diff
2. refuses to overwrite it automatically

This is intentionally conservative to avoid trashing hand-maintained workflows.
