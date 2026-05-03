# Changelog

## [1.2.0] - 2026-03-31

### Added
- add --skip-published flag for idempotent publishes

### Contributors
Thanks to our contributors for this release:
- @iamgp (2 commits)

## [1.1.4] - 2026-03-28

### Fixed
- publish only changed release_set packages

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.1.3] - 2026-03-28

### Fixed
- keep release_set PRs on one branch

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.1.2] - 2026-03-28

### Fixed
- keep release_set root releases on repo tags (#35)

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.1.1] - 2026-03-28

### Fixed
- preserve contributor identity across releases (#33)

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.1.0] - 2026-03-28

### Added
- add release_set monorepo mode (#31)

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.0.8] - 2026-03-27

### Fixed
- ignore nested workspace version files for root packages

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.0.7] - 2026-03-27

### Fixed
- honor unified monorepo release flow

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.0.6] - 2026-03-27

### Fixed
- publish release assets from autorelease

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.0.5] - 2026-03-26

### Changed
- v1.0.1 (#17)
- v1.0.2 (#22)
- v1.0.3 (#24)
- v1.0.4 (#26)

### Fixed
- sync lockfile after 1.0.0 release (#16)
- prevent release PR loop and regenerate lockfiles on release (#19)
- sync lockfile after version bump (#20)
- bound unified monorepo release refs (#21)
- clean up release automation (#23)
- streamline release follow-up automation (#25)
- run autorelease from current checkout
- sync Cargo.lock for v1.0.4

### Contributors
Thanks to our contributors for this release:
- @iamgp (8 commits)

## [1.0.4] - 2026-03-26

### Changed
- v1.0.1 (#17)
- v1.0.2 (#22)
- v1.0.3 (#24)

### Fixed
- sync lockfile after 1.0.0 release (#16)
- prevent release PR loop and regenerate lockfiles on release (#19)
- sync lockfile after version bump (#20)
- bound unified monorepo release refs (#21)
- clean up release automation (#23)
- streamline release follow-up automation (#25)

### Contributors
Thanks to our contributors for this release:
- @iamgp (6 commits)

## [1.0.3] - 2026-03-26

### Changed
- v1.0.1 (#17)
- v1.0.2 (#22)

### Fixed
- sync lockfile after 1.0.0 release (#16)
- prevent release PR loop and regenerate lockfiles on release (#19)
- sync lockfile after version bump (#20)
- bound unified monorepo release refs (#21)
- clean up release automation (#23)

### Contributors
Thanks to our contributors for this release:
- @iamgp (5 commits)

## [1.0.2] - 2026-03-26

### Changed
- v1.0.1 (#17)

### Fixed
- sync lockfile after 1.0.0 release (#16)
- prevent release PR loop and regenerate lockfiles on release (#19)
- sync lockfile after version bump (#20)
- bound unified monorepo release refs (#21)

### Contributors
Thanks to our contributors for this release:
- @iamgp (4 commits)

## [1.0.1] - 2026-03-18

### Fixed
- sync lockfile after 1.0.0 release (#16)

### Contributors
Thanks to our contributors for this release:
- @iamgp (1 commit)

## [1.0.0] - 2026-03-18

### Added
- add auto-tag workflow and expand test coverage (#9)
- stabilise for 1.0 release (#12)

### Breaking Changes
- stabilise for 1.0 release (#12)

### Changed
- re-trigger autorelease after v0.4.0 binaries published
- fix placeholder URLs, add badges, expand tests and docs for 1.0 readiness
- cargo fmt

### Fixed
- use correct action reference in generate-ci output (#11)

### Contributors
Thanks to our contributors for this release:
- @iamgp (6 commits)

## [0.4.0] - 2026-03-17

### Added
- add ecosystem-aware project detection
- add rust publish support
- improve ecosystem-specific release workflows
- add cargo workspace introspection
- add go workspace and publish planning
- add crates.io release checks
- support workspace release analysis for rust and go
- support cascade bumps in go workspaces

### Changed
- sync lockfile after 0.3.0 release
- rename pyrls to relx
- document ecosystem-aware relx setup
- cover go and rust workspace flows
- make registry checks ecosystem-aware
- align workspace and branch checks with channels
- update workspace and publish guidance

### Contributors
Thanks to our contributors for this release:
- @iamgp (15 commits)

## [0.3.0] - 2026-03-17

### Added
- polish branch-aware release channels

### Changed
- reconcile 0.2.0 release metadata

### Contributors
Thanks to our contributors for this release:
- @iamgp (2 commits)
