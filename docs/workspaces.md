# Monorepos and Workspaces

`relx` supports multi-package repositories in two ways:

- explicit `[monorepo]` configuration
- `uv` workspace auto-discovery
- Cargo workspace auto-discovery
- `go.work` auto-discovery

## Explicit monorepo configuration

```toml
[monorepo]
enabled = true
packages = ["packages/core", "packages/cli", "packages/sdk"]
release_mode = "unified"
```

`release_mode` values:

- `unified`: one release PR for all selected packages
- `release_set`: one release PR for whatever changed, short release titles, and publish only the packages that changed
- `per_package`: a release PR set for individually changed packages

## uv workspace auto-discovery

If the root `pyproject.toml` defines `tool.uv.workspace.members`, `relx` can discover packages automatically.

Example layout:

```text
pyproject.toml
uv.lock
packages/
  core/pyproject.toml
  cli/pyproject.toml
  sdk/pyproject.toml
```

Run:

```bash
relx workspace
```

Example output:

```text
relx workspace

Workspace root: pyproject.toml
Discovery: uv workspace (tool.uv.workspace.members)
Members:
  packages/core (mypackage-core 1.2.3)
  packages/cli (mypackage-cli 1.1.0) — depends on mypackage-core
  packages/sdk (mypackage-sdk 2.0.1)
```

## Cargo workspace auto-discovery

If the root `Cargo.toml` defines `workspace.members`, `relx` can discover Rust crates automatically.

Example layout:

```text
Cargo.toml
crates/
  core/Cargo.toml
  cli/Cargo.toml
```

Example output:

```text
relx workspace

Workspace root: Cargo.toml
Discovery: cargo workspace (workspace.members)
Members:
  crates/core (core 1.2.3)
  crates/cli (cli 1.2.3) — depends on core
```

## Go workspace auto-discovery

If the repository root contains a `go.work` file with `use` entries, `relx` can discover Go modules automatically.

Example layout:

```text
go.work
services/
  api/go.mod
  worker/go.mod
```

Example output:

```text
relx workspace

Workspace root: go.work
Discovery: go workspace (go.work use)
Members:
  services/api (api 0.9.0)
  services/worker (worker 1.1.0) — depends on api
```

## Package selection

For `release_set` and `per_package` modes, `relx` resolves a **per-package release baseline** and analyses each package from that package's last release, not from the most recent repository tag.

1. Resolve the package release identity (preferred tag: `<package>/v<version>`, for example `phlo/v0.15.2`).
2. Collect commits after that baseline that belong to the package (paths are owned by the longest matching package root; `.relx/` and changelog/lockfile bookkeeping do not bump a package by themselves).
3. Compute a conventional-commit bump **per selected package**.
4. Select only packages that changed and need a release.

`unified` mode still uses a shared `v<version>` tag as the common baseline.

Use repeatable `--package <name>` on `relx status`, `relx release plan`, `relx release prepare`, `relx release pr`, and `relx release tag` to prepare a subset. Preparing A leaves B's files, version, and baseline untouched even if B has unreleased changes. Each selected package keeps its own bump; `--next-version` is the only explicit override that applies one version to every selected package.

If a selected release falls outside a dependent's declared dependency range, the plan shows the required metadata change. The dependent must be selected explicitly or via `[workspace] cascade_bumps = true`. Compatible ranges are left unchanged and do not cascade.

```bash
relx release plan --json --package package-a
relx release prepare --package package-a
relx release pr --dry-run --package package-a
```

## Cascade bumps

Enable dependency-driven patch bumps when a published constraint would otherwise be invalid:

```toml
[workspace]
cascade_bumps = true
```

If `cli` depends on `core` with no compatible published range and `core` changes, `cli` can receive a patch bump even if no files in `cli` changed directly. A core release that remains inside a provider's declared range does **not** rewrite that range or bump the provider.

This now works for:

- Python workspaces when package dependencies can be resolved from `pyproject.toml`
- Cargo workspaces using crate dependency tables
- Go workspaces using `require` entries from member `go.mod` files

## Version mismatch warnings

`relx workspace` warns when workspace members have different versions. This is useful for spotting drift in repos that intend to keep package versions aligned.

## Release identity and legacy shared tags

Independent package tags use `<package>/v<version>`. Existing shared tags such as `v0.15.1` are preserved. Do not infer that every historical shared tag published every package. Map the releases that actually published a package:

```toml
[monorepo]
enabled = true
release_mode = "release_set"
packages = [".", "packages/polaris"]
first_release_packages = ["brand-new-distribution"]

[[monorepo.legacy_releases]]
tag = "v0.15.1"
commit = "df1788c2dbaa71121b06a8c58d3c7975767db55b"
packages = ["phlo", "phlo-polaris"]
```

- `[[monorepo.legacy_releases]]` is an explicit mapping from a shared tag (and optional commit) to the packages that tag published.
- `[monorepo].first_release_packages` marks genuinely new packages so missing history is not treated as a broken migration.
- If a repository has never created any `<package>/v*` tags and has no legacy mapping, `relx` still uses the latest shared tag so existing shared-tag repos keep working until they opt in.
- Beta/prerelease tags cannot consume stable history: a stable plan ignores `package/v1.1.0b1` and starts from the last stable package tag.

The Phlo values above are an example of how to record `v0.15.1` at `df1788c2dbaa71121b06a8c58d3c7975767db55b`. They are not hardcoded into ReleaseX.

## Release manifest

`relx release plan --json`, `relx release prepare`, and release PRs write a schema 2 manifest to `.relx/release-manifest.json` (override with `[release].plan_file`). Each selected entry records package name and path, previous and proposed versions, baseline reference and commit, bump/selection reason, relevant changes, and the intended package tag.

The manifest also records `preparation_base` (ref + commit) and a `source_digest` over `covered_paths` (selected package trees plus declared shared bookkeeping). After a squash merge or merge queue, `relx release verify-plan` and `relx release tag` accept the merged tree when those paths still match. They do not require the pre-merge commit SHA to equal the merge SHA. A missing, stale, tampered, or incompatible manifest fails with an actionable error so downstream CI can build exactly the reviewed packages.

```bash
relx release prepare --package phlo-polaris
relx release verify-plan
```
