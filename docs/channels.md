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

`relx` uses channel config for:

- `relx status --channel`
- branch-aware `relx status` version previews
- `relx release pr --channel ...`
- `relx release tag --channel ...`
- release PR base branch resolution
- prerelease numbering via the active ecosystem registry when available
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
relx release pr --channel beta
```

or from the `beta` branch:

```bash
relx release pr
```

On the `beta` branch, `relx` will:

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

If the active package registry is reachable and a package name can be resolved, `relx` tries to increment prerelease numbers based on existing releases:

- `1.2.3b1`
- `1.2.3b2`
- `1.2.3b3`

- Python uses PyPI history.
- Rust uses crates.io history.
- Go currently falls back to local version bumping.

If the registry cannot be queried, `relx` falls back to local version bumping.

## Manual pre-release flags

You can still use the lower-level flags directly:

```bash
relx release pr --pre-release beta
relx release tag --pre-release rc
relx release tag --finalize
```

Use channels when you want branch-driven behavior. Use flags when you want an explicit one-off override.
