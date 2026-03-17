# Configuration Reference

`relx` is configured with `relx.toml` at the repository root.

## Full example

```toml
[project]
ecosystem = "python"

[release]
branch = "main"
tag_prefix = "v"
changelog_file = "CHANGELOG.md"
pr_title = "chore(release): {version}"
release_name = "{tag_name}"

[versioning]
strategy = "conventional_commits"
initial_version = "0.1.0"

[[version_files]]
path = "pyproject.toml"
key = "project.version"

[[version_files]]
path = "src/mypackage/__init__.py"
pattern = '__version__ = "{version}"'

[changelog]
contributors = true
first_contribution_emoji = "🎉"
exclude_bots = true
bot_patterns = ["dependabot", "renovate", "github-actions"]

[changelog.sections]
feat = "Added"
fix = "Fixed"
refactor = "Changed"
perf = "Changed"
docs = false

[publish]
enabled = false
provider = "uv"
repository = "pypi"
dist_dir = "dist"
trusted_publishing = false
oidc = false
# repository_url = "https://test.pypi.org/legacy/"
# token_env = "PYPI_TOKEN"
# username_env = "PYPI_USERNAME"
# password_env = "PYPI_PASSWORD"

[github]
# owner = "example"
# repo = "project"
api_base = "https://api.github.com"
token_env = "GITHUB_TOKEN"
release_branch_prefix = "relx/release"
pending_label = "autorelease: pending"
tagged_label = "autorelease: tagged"

[monorepo]
enabled = false
packages = []
release_mode = "unified"

[workspace]
cascade_bumps = false

[ci]
provider = "github"
workflow_path = ".github/workflows/release.yml"

[[channels]]
branch = "main"
publish = true

[[channels]]
branch = "beta"
publish = true
prerelease = "b"

[[channels]]
branch = "1.x"
publish = true
version_range = ">=1.0.0,<2.0.0"
```

## `[release]`

- `branch`: the primary release branch to analyze
- `tag_prefix`: prefix used when creating tags, usually `v`
- `changelog_file`: changelog path to prepend release notes into
- `pr_title`: release PR title template, with `{version}` placeholder
- `release_name`: GitHub Release title template, with `{tag_name}` and `{version}` placeholders

## `[project]`

- `ecosystem`: optional explicit ecosystem override; supported values are `python`, `rust`, and `go`

If omitted, `relx` auto-detects the repository type from files such as `pyproject.toml`, `Cargo.toml`, and `go.mod`.

## `[versioning]`

- `strategy`: currently `conventional_commits`
- `initial_version`: version used when no tag or version can be read yet

## `[[version_files]]`

Each entry identifies a file that contains the package version.

Use `key` for structured files:

```toml
[[version_files]]
path = "pyproject.toml"
key = "project.version"
```

Use `pattern` for free-form text files:

```toml
[[version_files]]
path = "src/mypackage/__init__.py"
pattern = '__version__ = "{version}"'
```

## `[changelog]`

- `contributors`: include contributor attribution in release notes
- `first_contribution_emoji`: marker used for first-time contributors
- `exclude_bots`: omit likely automation accounts
- `bot_patterns`: custom bot match patterns

Use `[changelog.sections]` to map commit types to section names. Set a type to `false` to exclude it.

## `[publish]`

- `enabled`: enables `relx release publish`
- `provider`: `uv`, `twine`, or `cargo`
- `repository`: package repository name, usually `pypi`
- `repository_url`: explicit upload URL for custom indexes or TestPyPI
- `dist_dir`: artifact directory
- `trusted_publishing`: indicate trusted publishing is intended
- `oidc`: use GitHub Actions OIDC token exchange for PyPI
- `token_env`, `username_env`, `password_env`: credentials to source from environment variables

Examples:

- Python with `uv`: `repository = "pypi"`
- Python with `twine`: `repository_url = "https://test.pypi.org/legacy/"`
- Rust with `cargo`: `repository = "crates-io"` or a named Cargo registry

## `[github]`

- `owner`, `repo`: optional explicit GitHub coordinates; otherwise auto-detected from `origin`
- `api_base`: use this for GitHub Enterprise
- `token_env`: environment variable holding the GitHub API token
- `release_branch_prefix`: prefix for generated release branches
- `pending_label`, `tagged_label`: labels managed by `relx`

## `[monorepo]`

- `enabled`: treat the repository as multi-package
- `packages`: explicit package roots
- `release_mode`: `unified` or `per_package`

If `packages` is empty and a `uv` workspace is present, `relx` can auto-discover members.

## `[workspace]`

- `cascade_bumps`: if true, packages depending on bumped workspace packages receive patch bumps

## `[ci]`

- `provider`: currently `github`
- `workflow_path`: destination for generated workflow YAML

## `[[channels]]`

Channels map branches to release behavior.

- `branch`: branch name or maintenance line name
- `publish`: whether releases from this branch should be published
- `prerelease`: `a`, `b`, or `rc`
- `version_range`: simple guard such as `>=1.0.0,<2.0.0`

Examples:

```toml
[[channels]]
branch = "main"
publish = true

[[channels]]
branch = "beta"
publish = true
prerelease = "b"

[[channels]]
branch = "next"
publish = false
```
