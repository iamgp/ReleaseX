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
skip_published = false
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
commit_author = "github-actions[bot]"
commit_email = "41898282+github-actions[bot]@users.noreply.github.com"

[monorepo]
enabled = false
packages = []
release_mode = "unified"

[workspace]
cascade_bumps = false

[workspace.dependencies]
enabled = true

[[workspace.dependencies.rules]]
dependency = "core"
dependents = ["packages/*"]
when = "dependency_selected"
range = ">={version},<{next_minor}"

[[release.transformers]]
name = "sync-release-manifest"
command = ["python3", "scripts/sync_release_manifest.py"]
timeout_seconds = 60
inputs = ["registry/support/v1.json"]
outputs = ["registry/support/v1.json"]

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

### Workspace dependency rules and transformers

`[workspace.dependencies]` rewrites internal Python requirements after package versions are updated and before the lockfile refresh. Rules apply to all matching workspace packages; `when` is `dependency_selected` (the default), `dependent_selected`, or `always`. Range templates support `{version}`, `{current_version}`, `{major}`, `{minor}`, `{patch}`, `{next_major}`, and `{next_minor}`. Extras and environment markers are preserved; direct URL/path requirements are rejected.

`[[release.transformers]]` runs a repository-provided command in the isolated release workspace after dependency synchronization. It receives the versioned `ReleaseWorkspacePlan` JSON on stdin and must emit `{ "schema_version": 1, "changed_files": [...] }` to stdout. Every changed path must be declared in `outputs`; transformer processes inherit only `PATH`, not credentials or the parent environment. `timeout_seconds` defaults to 60 and terminates commands that exceed it.

For simple derived version references, prefer checked declarative replacements instead of a transformer. Each selected package expands `{name}`, `{path}`, `{current_version}`, and `{next_version}`. `files` uses workspace-relative globs and matching is literal, not regex. `expected_matches` is required for every expanded package/file operation, so missing or duplicate references abort preparation before a branch is pushed. Use `packages` only when a shared glob needs to target particular package names.

```toml
[[release.replacements]]
files = ["registry/support/*.json"]
for_each = "selected_packages"
search = '"name": "{name}", "version": "{current_version}"'
replace = '"name": "{name}", "version": "{next_version}"'
expected_matches = 1

[[release.replacements]]
files = ["services/**/*.yaml"]
packages = ["phlo-api"]
search = "image: ghcr.io/acme/{name}:{current_version}"
replace = "image: ghcr.io/acme/{name}:{next_version}"
expected_matches = 1
```

Replacements run after version and dependency synchronization, before external transformers and lockfile refresh. Dry runs report each proposed literal substitution and its checked match count. Keep `[[release.transformers]]` for complex transformations that cannot be represented as literal substitutions.

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
- `provider`: `uv`, `twine`, `cargo`, or `goreleaser`
- `repository`: registry name, such as `pypi`, `testpypi`, or `crates-io`
- `repository_url`: explicit upload URL for custom indexes or TestPyPI
- `dist_dir`: artifact directory
- `trusted_publishing`: indicate trusted publishing is intended
- `oidc`: use GitHub Actions OIDC token exchange for PyPI
- `skip_published`: skip packages already published to the registry (can also use `--skip-published` CLI flag)
- `token_env`, `username_env`, `password_env`: credentials to source from environment variables

Examples:

- Python with `uv`: `repository = "pypi"`
- Python with `twine`: `repository_url = "https://test.pypi.org/legacy/"`
- Rust with `cargo`: `repository = "crates-io"` or a named Cargo registry
- Go with `goreleaser`: `repository = "github"` and `dist_dir = "dist"`

## `[github]`

- `owner`, `repo`: optional explicit GitHub coordinates; otherwise auto-detected from `origin`
- `api_base`: use this for GitHub Enterprise
- `token_env`: environment variable holding the GitHub API token
- `release_branch_prefix`: prefix for generated release branches
- `pending_label`, `tagged_label`: labels managed by `relx`
- `commit_author`, `commit_email`: git identity used for bot-created release commits

## `[monorepo]`

- `enabled`: treat the repository as multi-package
- `packages`: explicit package roots
- `release_mode`: `unified`, `release_set`, or `per_package`

If `packages` is empty and a `uv` workspace is present, `relx` can auto-discover members.

## `[workspace]`

- `cascade_bumps`: if true, packages depending on bumped workspace packages receive patch bumps

## `[ci]`

- `provider`: currently `github`
- `workflow_path`: destination for generated workflow YAML

## `[prerelease]`

Controls optional Python monorepo prerelease safety checks. Defaults are inert unless
`enabled = true`.

- `enabled`: enable prerelease workspace dependency syncing and verification

```toml
[prerelease]
enabled = true

[prerelease.workspace]
include_root = true
sync_root_dependencies = true
sync_root_extras = ["defaults", "core-services"]

[prerelease.verify]
lock = true
build = true
inspect_wheel_metadata = true
emit_install_command = true
```

For Python `release_set` monorepos, beta release PRs include the root package,
rewrite selected root dependency/extras constraints to the beta workspace package
versions, run `uv lock`, build root and selected packages, inspect root wheel
metadata, and emit an explicit PyPI install verification command. Final releases
created with `relx release pr --finalize` select packages currently on prerelease
versions and rewrite those constraints back to stable versions.

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

## `[promotion]`

Promotion mode supports a two-long-lived-branch delivery model
(`feature/fix → develop → main`, `hotfix/* → main`). The preview
produces a single PR to production carrying code plus versioning; the tag
remains the version authority.

```toml
[promotion]
enabled = true
development_branch = "develop"
# production_branch = "main"   # defaults to [release].branch
hotfix_prefixes = ["hotfix/"]
tag_pattern = "v*"
release_branch_prefix = "relx/promote"
# baseline_version = "0.2.0"   # bootstrap floor for the active tag line
preview_marker = "<!-- relx-release-preview -->"
```

- `enabled`: turn on `relx release preview-pr` / `relx release finalize`
- `development_branch`: long-lived branch promoted to production
- `production_branch`: production branch; falls back to `[release].branch`
  when empty
- `hotfix_prefixes`: head-branch prefixes accepted as hotfix promotion PRs
  alongside the development branch
- `tag_pattern`: glob (`*`, `?`) selecting the active tag series. Use e.g.
  `v0.*` to retire historical `v1.*` tags and restart the active line
- `release_branch_prefix`: prefix for generated promotion branches, used
  only when `[[version_files]]` is configured
- `baseline_version`: explicit bootstrap version for the active line. Tags
  below it are ignored; when no matching tag exists yet, it becomes the
  current version instead of `versioning.initial_version`
- `preview_marker`: HTML marker identifying the sticky preview comment,
  used only for pre-existing user-owned PRs in tag-only mode

Two preview paths:

- **Versioned** — with `[[version_files]]`, preview generates a
  `relx/promote/*` branch from the promotion head with the version bump and
  changelog entry, opening a single PR to production that carries code plus
  versioning. `finalize` additionally asserts the merged version files equal
  the previewed version.
- **Tag-only** — without `[[version_files]]`, no branch or file change is
  made; the `develop → main` PR itself carries the preview in its
  relx-managed body, or in one sticky comment when the PR is user-owned.

Workflow sketch:

1. `relx release preview-pr` calculates the next version from Conventional
   Commits absent from the production baseline and publishes the preview on
   the promotion PR.
2. Reviewers approve the PR with the exact version and notes visible.
3. After merge, `relx release finalize --pr <number>` verifies the preview
   is still fresh and creates the annotated tag plus GitHub Release.
