# Channels and Pre-releases

Channels let you map Git branches to release behavior.

## Channel model

```toml
[[channels]]
branch = "main"
publish = true

[[channels]]
branch = "beta"
publish = true
prerelease = "b"

[[channels]]
branch = "alpha"
publish = true
prerelease = "a"

[[channels]]
branch = "1.x"
publish = true
version_range = ">=1.0.0,<2.0.0"
```

## Current behavior

`pyrls` uses channel config for:

- `pyrls status --channel`
- branch-aware `pyrls status` version previews
- `pyrls release pr --channel ...`
- `pyrls release tag --channel ...`
- release PR base branch resolution
- prerelease numbering via the PyPI project history when available
- simple version-range guards

## Examples

### Stable releases from `main`

```toml
[[channels]]
branch = "main"
publish = true
```

### Beta releases from `beta`

```toml
[[channels]]
branch = "beta"
publish = true
prerelease = "b"
```

Then:

```bash
pyrls release pr --channel beta
```

or from the `beta` branch:

```bash
pyrls release pr
```

On the `beta` branch, `pyrls` will:

- preview `next_version` as a beta version in `status`
- target `beta` as the release PR base branch
- generate tags like `v1.2.3b1`

### Maintenance branch guard

```toml
[[channels]]
branch = "1.x"
publish = true
version_range = ">=1.0.0,<2.0.0"
```

This prevents a `2.0.0` release from being cut from the maintenance line.

## Pre-release numbering

If PyPI is reachable and a project name can be resolved, `pyrls` tries to increment prerelease numbers based on existing releases:

- `1.2.3b1`
- `1.2.3b2`
- `1.2.3b3`

If PyPI cannot be queried, `pyrls` falls back to local version bumping.

## Manual pre-release flags

You can still use the lower-level flags directly:

```bash
pyrls release pr --pre-release beta
pyrls release tag --pre-release rc
pyrls release tag --finalize
```

Use channels when you want branch-driven behavior. Use flags when you want an explicit one-off override.
