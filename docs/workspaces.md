# Monorepos and uv Workspaces

`relx` supports multi-package repositories in two ways:

- explicit `[monorepo]` configuration
- `uv` workspace auto-discovery

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

## Version mismatch warnings

`relx workspace` warns when workspace members have different versions. This is useful for spotting drift in repos that intend to keep package versions aligned.

## Current limits

- `uv.lock` diff analysis is not yet used to generate dependency-only changelog sections
- workspace member mismatch detection is advisory only
