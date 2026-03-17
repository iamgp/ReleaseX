# Command Reference

## Global flags

```text
--config <PATH>   Path to config file (default: relx.toml)
--dry-run         Preview actions without mutating state
--verbose         Enable debug output
--no-color        Disable ANSI color output
```

## `relx init`

Create a starter config file by inspecting the repository.

```bash
relx init
relx init --dry-run
```

## `relx validate`

Parse and validate the configuration file.

```bash
relx validate
relx validate --config path/to/relx.toml
```

## `relx status`

Show release state for the current repository.

```bash
relx status
relx status --short
relx status --json
relx status --since=v1.2.0
relx status --channel
```

`status` includes:

- current and proposed versions
- unreleased commits
- package selection in monorepos
- release PR status when GitHub access is available
- latest published registry version when project metadata can be resolved

## `relx healthcheck`

Run release pre-flight validation.

```bash
relx healthcheck
relx healthcheck --only config
relx healthcheck --only git
relx healthcheck --only github
relx healthcheck --only build
relx healthcheck --only registry
```

`--only pypi` is still accepted as an alias for Python-oriented workflows, but `registry` is the preferred category name.

Exit codes:

- `0`: all checks passed
- `1`: one or more errors
- `2`: warnings only

## `relx workspace`

Print the detected monorepo or workspace structure.

```bash
relx workspace
```

## `relx generate-ci`

Generate a GitHub Actions release workflow.

```bash
relx generate-ci
relx generate-ci --dry-run
relx generate-ci --provider github
```

If the target workflow already exists and differs, `relx` prints a diff and refuses to overwrite automatically.

## `relx release`

Main release entrypoint.

### Snapshot mode

```bash
relx release --snapshot
```

Runs local release validation and writes outputs under `.relx/snapshot/`.

### `relx release pr`

Create or update the release PR.

```bash
relx release pr
relx release pr --dry-run
relx release pr --pre-release beta
relx release pr --channel beta
relx release pr --finalize
```

### `relx release tag`

Create the git tag and GitHub Release.

```bash
relx release tag
relx release tag --dry-run
relx release tag --channel beta
relx release tag --finalize
```

### `relx release publish`

Upload built distributions.

```bash
relx release publish
relx release publish --dry-run
```

## Pre-release kinds

Accepted values for `--pre-release`:

- `alpha`
- `beta`
- `rc`
- `post`
- `dev`
