# Troubleshooting and Operations

## Start with healthcheck

Run:

```bash
relx healthcheck
```

This is the fastest way to catch:

- invalid config
- missing GitHub token
- wrong branch
- unreachable remotes
- missing build tools
- existing registry version conflicts

## Common problems

## `relx.toml could not be loaded`

Fix:

- ensure the file exists
- ensure TOML syntax is valid
- run `relx validate`

## `nothing to release`

Possible reasons:

- there are no commits since the latest tag
- commit messages do not produce a releasable bump
- in a monorepo, your commits did not touch any selected package

Inspect with:

```bash
relx status
relx status --json
```

## GitHub API failures

Check:

- `GITHUB_TOKEN` is set
- the token can access the target repo
- the configured `owner`, `repo`, and `api_base` are correct

If using GitHub Enterprise, set:

```toml
[github]
api_base = "https://github.example.com/api/v3"
```

## Release PR not found in status

`relx status` looks for an open PR on the generated release branch. If none is found:

- no PR has been created yet
- the release branch prefix differs from the configured one
- the PR was renamed or manually recreated on a different branch

## Build failures

Check:

- Python: `uv` or `twine` is installed; `pyproject.toml` exists
- Rust: `cargo` is installed; `Cargo.toml` exists
- Go: `go` is installed; `go.mod` exists; `goreleaser` is available if publishing

Verify your build works outside `relx`:

```bash
# Python
uv build

# Rust
cargo build --locked

# Go
go build ./...
```

Then try:

```bash
relx release --snapshot
```

## Publish failures

Check:

- artifacts exist in `dist_dir`
- the configured repository and repository URL are correct
- the registry credentials required by the selected provider are present
- trusted publishing is correctly configured when using Python package uploads

## Workflow generation refuses overwrite

`relx generate-ci` intentionally refuses to overwrite a differing workflow automatically. Use:

```bash
relx generate-ci --dry-run
```

Review the output or diff, then replace the workflow manually.

## Operational recommendations

- keep release automation running only on the intended release branches
- use `relx healthcheck` before enabling auto-publish
- prefer OIDC trusted publishing over static PyPI tokens when publishing Python packages
- keep `relx.toml` small and explicit
- use `--dry-run` before changing release configuration
