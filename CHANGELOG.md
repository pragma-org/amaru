# Changelog

<!--
Keep a changelog that is human-readable and structured. Each version shall
have its own entry and, every change ought to be categorized as one of the
following types:

- `Added`: for new features.
- `Changed`: for changes in existing functionality.
- `Deprecated`: for soon-to-be removed features.
- `Removed`: for now removed features.
- `Fixed`: for any bug fixes.
- `Security`: in case of vulnerabilities.

Other guiding principles:

- Changelogs are for humans, not machines.
- There should be an entry for every single version.
- The same types of changes should be grouped.
- Versions and sections should be linkable.
- The latest version comes first.
- The release date of each version is displayed.
- Entry should follow a simple format:

  ```
  - **CRATE/AREA**: SHORT DESCRIPTION [(COMMA-SEPARATED PRS/ISSUES)]
    [\n]
    [OPTIONAL LONG DESCRIPTION]
  ```

  For example:

  ```
  - **amaru-ouroboros**: properly wipe KES key material in unused method `SecretKey::from_bytes` ([#881](https://github.com/pragma-org/amaru/issues/881))
  ```
-->

## v10.10.20260625 _[unreleased; planned for 2026-06-25]_

### Changed

- **amaru-kernel**: allow null-length era params, so custom testnets can skip leading eras (encoded as empty eras with identical start/end bounds and a zero epoch size). ([#959][])

## [v10.10.20260618](https://github.com/pragma-org/amaru/releases/tag/v10.10.20260618)

### Added

- **amaru**: allow individual global parameters override in `run` and `bootstrap` commands, to facilitate custom testnets; see `--help-global-parameters` ([#886][])
- **amaru**: `create-snapshots` can be fully local, using a cardano-node's db at a specific location and using local `--snapshot` points instead of resolving them through Koios. ([#886][])

### Changed

- **amaru-kernel**: `ConsensusParameters` and `GlobalParameters` now live in their own modules instead of being paired with `ProtocolParameters`. Still exported at the top-level. ([#886][])
- **amaru-kernel**: remove `From` instances between `NetworkName` and `GlobalParameters`, `ProtocolParameters` and `EraHistory` in favor of faillible `as_*`.
- **pure-stage**: rename to `amaru-pure-stage` ([#954][])

### Removed

- **amaru-protocols**: remove the interim batch block-fetch API and keep the streaming `FetchBlocks` API ([#778][], [#942][])
- **amaru-kernel**: `TESTNET_GLOBAL_PARAMETERS` is gone; must now be provided manually. The `TESTNET_ERA_HISTORY` is also gone, in favor of `EraHistory::default()` ([#886][])

### Fixed

- **amaru**: fix `--help` being displayed as a debug Rust value instead of properly formatted. ([#953][])
- **amaru**: resolve era history from snapshots instead of inferring them from network (required for custom testnets). ([#886][])
- **amaru-ouroboros**: default to `0` as leader relative stake when the leader has no stake (instad of crashing due to a division by zero) ([#886][])
- **amaru-ouroboros**: skip leader-schedule check if active_slot_coeff is greater than or equal to 1 (degenerate case) ([#886][])
- **amaru-ledger**: allow restoring with less than 3 stake distributions, but raise a warning. ([#886][])

## [v10.10.20260611](https://github.com/pragma-org/amaru/releases/tag/v10.10.20260611)


[#778]: https://github.com/pragma-org/amaru/issues/778
[#886]: https://github.com/pragma-org/amaru/pull/886
[#942]: https://github.com/pragma-org/amaru/pull/942
[#953]: https://github.com/pragma-org/amaru/pull/953
[#954]: https://github.com/pragma-org/amaru/pull/954
[#959]: https://github.com/pragma-org/amaru/pull/959
