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

For monorepos, `relx`:

1. inspects commits since the latest tag
2. maps changed paths to package roots
3. computes a bump per package
4. selects only packages that changed and need a release

## Cascade bumps

Enable dependency-driven patch bumps:

```toml
[workspace]
cascade_bumps = true
```

If `cli` depends on `core` and `core` changes, `cli` can receive a patch bump even if no files in `cli` changed directly.

This now works for:

- Python workspaces when package dependencies can be resolved from `pyproject.toml`
- Cargo workspaces using crate dependency tables
- Go workspaces using `require` entries from member `go.mod` files

## Version mismatch warnings

`relx workspace` warns when workspace members have different versions. This is useful for spotting drift in repos that intend to keep package versions aligned.

## Current limits

- `uv.lock` diff analysis is not yet used to generate dependency-only changelog sections
- workspace member mismatch detection is advisory only
