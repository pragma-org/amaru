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

## v10.11.20260723 _[unreleased; planned for 2026-07-23]_

### Added

- **amaru-ledger**: trace spans for the ledger rules (phase-one and phase-two). ([#1056][])
- **amaru-ledger**: run scripts in parallel within the same transaction. ([#1056][])

### Fixed

- **amaru-ledger**: use effective collateral when collecting epoch fees for phase-2-invalid transactions. ([#1048][])
- **amaru-consensus / amaru-protocols**: do not log an ERROR when block-fetch is paused because no upstream peers are connected yet; keep ERROR for real fetch timeouts after peers were contacted. ([#1050](https://github.com/pragma-org/amaru/issues/1050))
- **amaru-plutus**: encoding divergence between rational number present in governance actions and those present in protocol parameters. ([#1053][])
- **amaru-ledger**: restore some spans in the ledger at the debug level. ([#1056][])
- **amaru**: support Cardano ledger peer snapshots via `--peer-snapshot` / `AMARU_PEER_SNAPSHOT` for cold-start big-ledger peers in peer selection (complements `--peer-address`) ([#1047](https://github.com/pragma-org/amaru/pull/1047))
- **amaru**: embed best-effort peer snapshots for known networks (for example mainnet, preprod, preview) at build time from cardano-foundation/cardano-configurations; used by default when `--peer-snapshot` is omitted

## [v10.11.20260716](https://github.com/pragma-org/amaru/releases/tag/v10.11.20260716)

### Added

- **amaru-ledger**: validate the minimum transaction fee during phase-one. ([#820][])
- **amaru-ledger**: enforce the per-transaction and per-block limits on the total size of reference scripts. ([#820][])
- **amaru-ledger**: add more state elements to the validation context, enabling the introduction of ledger predicates that depend on state such as pools, governance, and more. ([#831][], [#896][], [#902][], [#915][], [#975][], [#1017][])
- **amaru-ledger**: validate value preservation across (valid and invalid) transactions. ([#892][], [#831][])
- **amaru-ledger**: validate account reward balance at unregistration (and fail when non-zero). ([#899][], [#1033][])
- **amaru**: add / modify tracing spans to conform to [EDR-26](https://github.com/pragma-org/amaru/blob/main/engineering-decision-records/026-tracing-span-design.md). ([#996][])
- **amaru**: add a demo showcasing `amaru` as a relay node supporting both chainsync (to synchronize downstream nodes) and txsubmission (to diffuse transactions upstream). ([#1029][])

### Removed

- **amaru**: no more `--force` flag on `snapshot create`. ([#1039][])

### Fixed

- **amaru-consensus**: use slot height instead of block height for block forecast, to allow coping better with low density chains (fixes the regression in syncing time on Preview/PreProd). ([#1027][])
- **amaru-ledger**: reduce rationale number before serializing them to JSON in epoch summary. ([#1024][])
- **amaru-ledger**: store the stake pool deposit for refunds instead of defaulting protocol parameters. ([#1031][])
- **amaru-ledger**: do not overwrite already counted pool deposit refunds with newer refund from same credential. ([#1026][])
- **amaru-ledger**: do not enforce max protocol version in block header; fix node stalling after hard fork on Preview. ([#1043][])
- **amaru**: normalize snapshot metadata (all having a parent point now) and ensure better discoverability of local snapshots. ([#1039][])

## [v10.10.20260709](https://github.com/pragma-org/amaru/releases/tag/v10.10.20260709)

### Added

- **amaru-ledger**: keep treasury donations as first-class ledger state, so protocol pots and derived summaries retain the corresponding accounting information. ([#1010][])
- **amaru-ledger**: validate the minimum transaction fee during phase-one. ([#820][])
- **amaru-ledger**: enforce the per-transaction and per-block limits on the total size of reference scripts. ([#820][])

### Changed

- **amaru**: make sure bootstrap can't be done with unsupported snapshots; similarly prevent startup if state becomes unsupported. ([#1000][])
- **amaru**: more robust procedure and tooling to produce stake-distribution epoch snapshots for conformance testing and comparison with the haskell node. ([#985][])
- **amaru**: fix snapshot import of pools with no metadata. ([#1013][])
- **amaru**: stake-distribution conformance tests now auto-detect which local epochs are available in `ledger.<network>.db` and run those by default, while leaving uncovered fixtures visible as ignored tests. ([#1010][])
- **amaru-ledger**: load the initial in-memory stake distributions in parallel at startup, to reduce restore time when opening the node from existing snapshots. ([#1010][])

### Fixed

- **amaru**: align the cardano-node reference snapshots in the haskell-node-extractor with the post-rewards epoch state, so treasury, reserves, accounts, and voting stake match what Amaru snapshots at epoch end. ([#1010][])
- **amaru-consensus**: be more conservative when fetching ahead from peers during long low-density periods, to avoid requesting headers whose stake distribution is not ready yet. ([#1010][])
- **amaru-ledger**: make epoch transitions and rewards calculations more resilient around restarts, interrupted transitions, and weak chain-growth periods near epoch boundaries. ([#1010][])
- **amaru-ledger**: preserve delegators across DRep re-registration, while still ignoring stale DRep delegations that pre-date the current registration. ([#1010][])
- **amaru-ledger**: fix epoch-boundary stake-distribution accounting by remembering recently pruned proposals and counting just-ratified treasury withdrawals in DRep voting stake. ([#1010][])
- **amaru-ledger**: stop treating governance proposals as expired one epoch too early when rebuilding the proposal forest for ratification. ([#1010][])
- **amaru-ledger**: fix pool registration shadowing retirements when done in the last k blocks of an epoch. ([#1010][])

## [v10.10.20260702](https://github.com/pragma-org/amaru/releases/tag/v10.10.20260702)

### Added

- **workflows**: dedicated workflow to create and publish bootstrapping snapshots for all networks. ([#951][])

### Changed

- **amaru-ledger**: redesigned the `VolatileDB`, providing faster lookups and a design that is easier to reason about. ([#963][], [#983][])

- **amaru**: restructure the CLI into noun-based command groups, following the guidelines in [EDR-019](https://github.com/pragma-org/amaru/blob/main/engineering-decision-records/019-guidelines-for-writing-cli.md): `node` (`run`, `bootstrap`, `reset`), `snapshot` (`create`), and a hidden `dev` group for debugging tools (`chain`, `ledger`, `traces`). The previous top-level commands (`run`, `daemon`, `bootstrap`, `reset-to-epoch`, `create-snapshots`, `dump-chain-db`, `remove-validation-status`, `fetch-chain-headers`, `migrate-chain-db`, `remove-chain`, `dump-traces-schema`) remain as hidden, backward-compatible aliases. ([#973][])
- **amaru-kernel**: allow null-length era params, so custom testnets can skip leading eras (encoded as empty eras with identical start/end bounds and a zero epoch size). ([#959][])

### Fixed

- **amaru-uplc**: make cost models semantics-aware and cleanup various parts and exposed API for in the UPLC crate ([#988][])
- **amaru**: remove the `--quiet` flag occurences in CI & scripts; silence can be obtained by setting `AMARU_LOG=off` already ([#981][]).

## [v10.10.20260625](https://github.com/pragma-org/amaru/releases/tag/v10.10.20260625)

### Fixed

- **amaru-ledger**: report ledger RocksDB lock contention with dedicated startup guidance. ([#769][])

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

[#769]: https://github.com/pragma-org/amaru/issues/769
[#778]: https://github.com/pragma-org/amaru/issues/778
[#820]: https://github.com/pragma-org/amaru/pull/820
[#831]: https://github.com/pragma-org/amaru/pull/831
[#886]: https://github.com/pragma-org/amaru/pull/886
[#892]: https://github.com/pragma-org/amaru/issues/892
[#896]: https://github.com/pragma-org/amaru/issues/896
[#899]: https://github.com/pragma-org/amaru/issues/899
[#902]: https://github.com/pragma-org/amaru/issues/902
[#915]: https://github.com/pragma-org/amaru/issues/915
[#942]: https://github.com/pragma-org/amaru/pull/942
[#951]: https://github.com/pragma-org/amaru/pull/951
[#953]: https://github.com/pragma-org/amaru/pull/953
[#954]: https://github.com/pragma-org/amaru/pull/954
[#959]: https://github.com/pragma-org/amaru/pull/959
[#963]: https://github.com/pragma-org/amaru/pull/963
[#973]: https://github.com/pragma-org/amaru/pull/973
[#975]: https://github.com/pragma-org/amaru/pull/975
[#981]: https://github.com/pragma-org/amaru/pull/981
[#983]: https://github.com/pragma-org/amaru/pull/983
[#985]: https://github.com/pragma-org/amaru/pull/985
[#988]: https://github.com/pragma-org/amaru/pull/988
[#996]: https://github.com/pragma-org/amaru/pull/996
[#1000]: https://github.com/pragma-org/amaru/pull/1000
[#1010]: https://github.com/pragma-org/amaru/pull/1010
[#1013]: https://github.com/pragma-org/amaru/pull/1013
[#1017]: https://github.com/pragma-org/amaru/pull/1017
[#1024]: https://github.com/pragma-org/amaru/pull/1024
[#1026]: https://github.com/pragma-org/amaru/pull/1026
[#1027]: https://github.com/pragma-org/amaru/pull/1027
[#1029]: https://github.com/pragma-org/amaru/pull/1029
[#1031]: https://github.com/pragma-org/amaru/pull/1031
[#1033]: https://github.com/pragma-org/amaru/pull/1033
[#1039]: https://github.com/pragma-org/amaru/pull/1039
[#1043]: https://github.com/pragma-org/amaru/pull/1043
[#1048]: https://github.com/pragma-org/amaru/pull/1048
[#1053]: https://github.com/pragma-org/amaru/pull/1053
[#1056]: https://github.com/pragma-org/amaru/pull/1056
