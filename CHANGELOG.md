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



## v10.11.20260903 _[unreleased; planned for 2026-09-03]_

### Added

- **amaru-kernel**: `PeerCandidate` is `Socket` (already a `Peer`), `Host` (hostname+port, A/AAAA) or `Srv` (DNS name). `Host`/`Srv` names are a validated [`DnsName`] (no colons, brackets, or IP literals). Outbound mix pools are `PeerCandidate`s (static included); Host/SRV names are resolved on each dial so DNS changes are picked up. Host lookup takes the first viable A/AAAA; SRV lookup tries `_cardano._tcp.<name>` in RFC 2782 priority order and stops at the first viable target address. Ledger relays pass through as candidates (socket, hostname+port, or CIP-0155 SRV when the port is omitted).
- **amaru-pure-stage**: `Effects::detach` runs an external effect without occupying the airlock. The transition is resumed with `()` immediately; when `run()` completes the interpreter applies the provided constructor and enqueues the value on the calling stage’s bulk mailbox.
- **amaru-pure-stage**: `Effects::set_timeout` / `clear_timeout` (and `_at` slot variants) arm coalesced protocol timers: many slots, one armed schedule, without storing a `ScheduleId`. Typestate remainders can require `SetTimeout` / `ClearTimeout`.
- **amaru-pure-stage**: `SimulationRunning::run(Run)` with `TimeAdvance` / `Externals`. Default stops at the next wakeup and leaves unresolved `UntilResolved` effects as `Busy`. `complete_external` injects a world-driven blocking result; `complete_detach` injects a world-driven detached result selected by effect type and predicate (not issue order). Breakpoints are inspect-only (`breakpoint_effect` must be dropped before the next `run`). `interpret_breakpoint` applies that effect without continuing the run loop; `discard_breakpoint` leaves the stage suspended (for `complete_external`). `tm_*` matchers apply to both traces and the borrowed breakpoint effect.
- **amaru-pure-stage**: opt-in typestate layer over `Effects`. A protocol state exposes a receive constructor that consumes a receive allowance (by input variant, not the stage mailbox type) and returns the remaining legal effects. The next state comes only from `Session::finish` (into a live enum); `Send<Role, T>` names a destination whose mailbox implements `From<T>`.
- **amaru-pure-stage**: `define_messages!` declares a protocol message enum and generates a struct per variant plus `From` / `FromMailbox` conversions. Extra `#[derive]` on the enum applies to every struct; per-variant attributes apply only to that struct. Manual impls (`Encode`, …) are written after the macro as usual.
- **amaru-protocols**: pipelined BlockFetch installs the 60s `StBusy` / `StStreaming` agency timeout from the networking blueprint; Idle and Done clear it.
- **amaru-tui**: the Peers card shows the bootstrap `PeerCandidate` next to a resolved socket address when that name came from a Host or SRV lookup.
- **amaru**: allow operators to select the OTLP providers constructed at startup with
  `--with-open-telemetry=<SIGNALS>` or `AMARU_WITH_OPEN_TELEMETRY=<SIGNALS>`; it accepts any comma-separated subset of
  `metrics`, `traces`, and `logs`, and defaults to all three signals when enabled without a value.

### Changed

- **amaru-consensus**: `BlockValidator` now runs the ledger on a dedicated thread that owns the ledger state exclusively; operations are requested through a bounded channel and answered via per-request reply channels, replacing the shared lock around the state. `BlockValidator` is no longer generic over the store types. ([#1094][])
- **amaru-consensus**: after an outbound handshake, peer selection sends `SetLocalUse(Diffusion)` so fetch and share follow actual local use. Peer-sharing cadence is 300s then 900s (blueprint defaults).
- **amaru-protocols**: `LocalUseApplied` is the source of `may_initiate`; fetch and share route only to connections whose local use is Diffusion.
- **amaru-protocols**: `SetLocalUse` stops initiator groups with `MsgDone` (last-to-finish; 300s diffusion / 120s maintenance). Expected child death installs a mux done-trap; unexpected death still tears the bearer. Responders reset in place after `MsgDone`.
- **amaru-protocols**: connection stage is one Established session with `LocalUse` (None / Maintenance / Diffusion). The manager no longer redials on drop; peer selection refills outbound slots.
- **amaru-protocols**: mux times SDU assembly and send: unbounded wait for the first header byte, then 10s during the first Handshake and 30s afterwards for the rest of that SDU. Overflow of that timer tears the bearer down.
- **amaru-protocols**: handshake combines version data as in the node-to-node spec: network magics must match, `initiatorOnlyDiffusionMode` and `query` are OR, `peerSharing` is AND. The initiator checks that `MsgAcceptVersion` carries that agreed record. Outbound connections still offer initiator-only diffusion (duplex is not advertised until both halves run).
- **amaru-protocols**: `NetworkOps::connect` takes a `Peer`. Outbound dialling no longer resolves names; that stays in peer selection.
- **amaru-node**: add world test simulation, both with generated fake chain and with a real chain fragment from preprod.
- **amaru-node**: `Telemetry::install` no longer reads `AMARU_OPEN_TELEMETRY_SIGNALS`; embedders that select OTLP signals must pass `TelemetryOptions` to `install_with_options`.

### Removed

- **amaru-pure-stage**: single-step simulation API (`try_effect`, `effect()`, `resume_*`, `run_one_step`, `run_until_*`). `Effect::assert_*` / `extract_*` helpers are gone; use `tm_*` against traces or `breakpoint_effect()`.

### Fixed

- **amaru-node**: the stake-distribution callback no longer holds a strong `Resources` handle, so dropping a node graph closes its dummy RocksDB ledgers instead of leaking file descriptors across simulation runs.
- **amaru**: replace repeated low-level OpenTelemetry export errors with batched state-transition messages: one warning listing unavailable signals, one info listing recovered signals, and a fresh warning after a later failure.
- **amaru-bootstrap**: emit phase lifecycle events and periodic progress heartbeats when bootstrap runs without a terminal, so redirected and service logs no longer appear stuck during long imports.
- **amaru-node**: attempt to flush and shut down every configured OpenTelemetry provider even when an earlier provider reports an error.

## [v10.11.20260827](https://github.com/pragma-org/amaru/releases/tag/v10.11.20260827)

### Added

- **amaru-node**: allow embedders to disable sysinfo-backed system metrics collection through `TelemetryOptions`.

### Changed

- **amaru-observability**: field rendering is declared on the schema (`%` Display, `?` Debug, otherwise `Serialize + JsonSchema`). Call sites no longer take `%` / `?`; `@` remains for `tracing::Value` passthrough. `amaru dev traces dump` emits the JSON-sink schema of each field. `Point` traces encode as `[slot, hash, height]` with a CBOR byte-string hash ([#1263](https://github.com/pragma-org/amaru/issues/1263)).
- **amaru-kernel**: `Hash` (and newtypes / `FixedBytes`) serialize as CBOR byte strings on the tracing path; JSON serde still uses hex. JSON/console/TUI/OTEL span sinks render those bytes as hex; OTEL logs keep byte strings.
- **amaru**: use the `amaru-observability` crate with all other amaru crates so that all tracing events have a schema (#1266).
- **amaru-ledger**: restructured script validations and sped up phase-one validation by up to ~11.5%


### Fixed

- **amaru-protocols**: cancel the tx-submission responder's inflight-fetch timeout as soon as the peer replies, so bursty inbound traffic cannot fill the priority mailbox and panic ([#1270](https://github.com/pragma-org/amaru/issues/1270)).
- **amaru-consensus**: resume outbound sync after a mux drop: forget the dead `Connected` entry before the replacement handshake, and do not immediately re-dial a banned static peer while the manager still holds the live connection ([#1265](https://github.com/pragma-org/amaru/issues/1265)).
- **amaru-consensus**: report clock-skew versus non-monotonic slot failures distinctly instead of the same “expected at least” message ([#1265](https://github.com/pragma-org/amaru/issues/1265)).
- **amaru-protocols**: include the `Peer` on mux receive/send failure logs ([#1265](https://github.com/pragma-org/amaru/issues/1265)).
- **amaru-stores**: abort on chain-store header and block loads whose content does not hash to the requested key (including the block body hash), including diagnostic scans ([#1261](https://github.com/pragma-org/amaru/issues/1261)).
- **amaru-ledger**: drop the block-body-hash rule; the chain store now enforces it on load ([#1261](https://github.com/pragma-org/amaru/issues/1261)).
- **workflows**: executable permissions are now correctly preserved in the release workflow.
- **amaru-ledger**: correctly calculate an output's minimum lovelace value.
- **amaru-ledger**: do not re-encode locally submitted transactions to obtain their size.
- **amaru-protocols**: preserve bytes of transactions flowing through the mempool.
- **amaru-tui**: tweak block dissemination metrics headers (fetch → select, sync → fetch)
- **amaru-kernel**: decode reward accounts as a network tag plus stake credential, rejecting malformed reward accounts at deserialization.
- **amaru-ledger**: Do not validate disjoint input sets in protocol version 11+. Similarly, do not allow non-disjoint input sets in PV3, regardless of protocol version.
- **amaru-ledger**: do not allow the same script to exist in both the witness set and in a reference input
- **amaru-ledger**: fix a panic when an `is_valid=false` transaction's certificates net to a negative value

## [v10.11.20260820](https://github.com/pragma-org/amaru/releases/tag/v10.11.20260820)

### Added

- **amaru-pure-stage**: simulate how long an `ExternalEffect` occupies time (`DurationDist`: zero, constant, uniform, or until the effect future resolves). Sampled durations are scheduled when the effect is issued; `SimulationBuilder::run` now takes a Tokio `Handle` so a still-pending `run()` can be forced at that deadline. ([#1224](https://github.com/pragma-org/amaru/pull/1224))
- **amaru-consensus**: log the transaction ids evicted from the mempool when a block is adopted, and specify if those transactions were included in a block (#1243).

### Changed

- **amaru-node**: `Telemetry::install` / `install_local` take a `LogFormat` (`Plain`, `Ansi`, or `Json`) instead of a JSON boolean, so embedders can enable ANSI colour on the console sink (JSON remains exclusive with colour).
- **amaru-bootstrap**: speed up node bootstrap by streaming state archives and account imports, decoding large snapshot maps incrementally, and avoiding unnecessary optimistic-transaction conflict tracking when importing fresh database batches.
- **amaru-pure-stage**: `contramap` is now a method on `StageRef`. It no longer allocates a runtime name or adapter entry; the injection runs in the sending stage and traces record the transformed message sent to the original stage. `StageGraph::contramap` and `Effects::contramap` are removed. `Sender::send` now returns `SendError` instead of the original message (the payload cannot be recovered after a contramap injection). ([#762](https://github.com/pragma-org/amaru/issues/762))
- **amaru-ledger**: group phase-one and phase-two traces under a single trace with multiple fields for each of the measurements.

### Fixed

- **amaru-kernel**: decode transaction inputs as a set, rejecting duplicate inputs.
- **amaru**: resolve `snapshot create` and `snapshot publish` default directories relative to the runtime executable instead of the build machine's source directory.
- **amaru-tui**: calculate reported block and transaction throughput using the interval between system-metric samples.
- **amaru-tui**: show the most recent `INFO` log messages.
- **amaru-plutus**: encode `CostModels` as a map from language to cost model (#1219).
- **amaru-ledger**: compute the size of a value using the same encoding as the Haskell node.
- **amaru-ledger**: validate voters in a transaction actually exist in ledger state. ([#1138][], [#923][])
- **amaru-ledger**: reduce churn allocations during stake distribution and rewards calculations.
- **amaru-ledger**: prevent CC action of resigned members.
- **amaru-ledger**: resolve and control CC member cold credentials following ratification or in-flight proposals.
- **amaru-ledger**: disallow voters not relevant to the target proposals.
- **amaru**: use definite decoding for Conway bytes
- **amaru-ledger**: preserve dormant-epoch state during bootstrap and avoid extending the expiry of already-expired DReps.
- **amaru-ledger**: reject votes cast on governance actions that have expired. ([#1143][], [#926][])
- **amaru-stores**: prune obsolete votes when removing expired/ratified/evicted proposals.
- **amaru**: missing epoch stake distribution snapshots for mainnet, and fixed stake distribution management Makefile.
- **amaru-node**: `Telemetry` (used by `run_until`) now uses the product CBOR-aware tracing layers so structured log properties decode to numbers and strings instead of diagnostic byte strings.
- **amaru**: more homogenous traces fields representations across peers, connection id and hashes.
- **amaru**: detect local snapshots for bootstrap when they exists.
- **amaru**: resolution of recently unregistered accounts during bootstrap, later triggering a "rewards discrepancy" invariant.

## v10.11.20260813 _[unreleased]_

### Added

- **amaru-consensus**: track peer-sharing reputation (handshake advertisability, connection failures, sticky adversarial flag, and whether a successful connection was ever established) so peer selection can filter share candidates. ([#1167](https://github.com/pragma-org/amaru/issues/1167))
- **amaru-protocols**: add peer-sharing mini-protocol (client and server) and wire it into the connection manager. ([#1168](https://github.com/pragma-org/amaru/issues/1168))
- **amaru-consensus**: use shared peers to populate the peer candidate pool and serve share requests. ([#1169](https://github.com/pragma-org/amaru/issues/1169))
- **amaru-consensus**: allow configuration of peer mixture from static, shared, snapshot, and ledger peers. ([#1180](https://github.com/pragma-org/amaru/issues/1180))
- **amaru-node / embedding**: first-class embedding surface for running Amaru in-process without the product CLI or TUI (EDR 031). `NodeBuilder` for network-aware setup; `build_and_run_node(config, runtime_handle)` takes an explicit Tokio handle (never ambient context); metrics are optional on the config. Ledger observers for adopted blocks and opt-in stake-summary views (by reference; field-level clone); stop policy remains with the outer app. Examples: `run_until`, `address_watch`.
- **amaru-bootstrap**: new crate owning cold-start snapshot download/import (S3/CDN, archives, chain-store seeding). Product CLI re-exports it for compatibility.
- **amaru-observability**: shared `TelemetryCaptureLayer` / `subscribe_telemetry` for schema-oriented in-process event subscription (used by the TUI and embedders).
- **amaru-ledger**: `LedgerObservers` with `on_block` / `LedgerBlockEvent` (adopt + tip-first undo on successful fork switch) and opt-in `on_ledger_snapshot` (full `StakeSummary` by reference before slim in-memory retention).
- **amaru-observability**: schema field transport now preserves JSON primitives (`bool`/`number`/`string`) and encodes other values as CBOR (`record_bytes`); console/JSON layers decode CBOR for structured output (including real JSON arrays/objects). OTEL logs use a project-owned bridge that maps CBOR fields to nested `AnyValue` maps/lists rather than opaque bytes.
- **amaru-consensus**: add more debug events for the txsubmission stage ([#1173](https://github.com/pragma-org/amaru/issues/1173))

### Changed

- **amaru-stores**: bump chain DB schema to version 6. Existing v5 databases can be migrated: best-chain tip, anchor, and per-slot chain index values are rewritten from a header hash to a CBOR `NetworkTip` (`[network_point, block_height]`). The ledger `@tip` remains a `NetworkPoint`.
- **amaru / amaru-tui**: product observability setup no longer types against TUI-specific capture types; the TUI installs the shared observability capture layer.
- **scripts/run-until**: drives the `amaru-node` `run_until` example (observer-based stop). The example installs OTLP via `amaru_node::Telemetry` when `AMARU_WITH_OPEN_TELEMETRY` is set, so e2e metrics still flow to the collector.
- **amaru-node**: `Telemetry::install` embedder helper for fmt / JSON / OTLP (metrics + traces + logs) using the same env knobs as the product binary.
- **amaru-observability**: structured logging for complex values: schema transport preserves JSON primitives and encodes other values as CBOR (`record_bytes`); JSON traces get nested objects/arrays, OTEL logs get nested `AnyValue` maps/lists, OTEL spans upgrade homogeneous CBOR arrays to `Value::Array` (with CBOR diagnostic fallback otherwise), and console logs use CBOR diagnostic notation. ([#1182](https://github.com/pragma-org/amaru/pull/1182))
- **amaru**: stake-distribution conformance tests no longer partition `#[ignore]` vs active cases from `ledger.<network>.db` at build time (watching that live DB rebuilt `amaru` on every node write). Fixture directories are still watched so tests regenerate when snapshots change; each test soft-skips with a warning when the matching local ledger snapshot is missing.
- **amaru-ledger**: validate the governance actions a transaction votes on actually exist, counting proposals submitted earlier in the same block. ([#1139][], [#924][])
- **amaru-ledger**: reject a governance proposal whose policy is not the enacted constitution's guardrails script. ([#931][])

### Fixed

- **amaru-observability**: restore wrapping-span identity on every product tracing stack (console, JSON, OTEL, TUI) and stop double-quoting CBOR string scalars such as header hashes. Each event inlines only its wrapping span's fields; `parents` is a name array; child lines refer to the parent by id. Console uses a Java-style abbreviated path (`e.t:g.r`). Encoding is specified in EDR-033. ([#1208](https://github.com/pragma-org/amaru/issues/1208))
- **amaru-node**: when OTLP is enabled, `Telemetry` starts the same process/build gauges as the product binary (`process_*`, `cardano_node_metrics_cardano_*`) so embedders such as `run_until` satisfy the e2e metrics contract.
- **amaru-consensus**: make `select_chain` handle already-validated tips idempotently so concurrent startup recovery cannot terminate the stage. ([#1124](https://github.com/pragma-org/amaru/pull/1124))
- **amaru**: store nonces with both packaged bootstrap headers so the "nonces present ⇔ header validated" invariant holds after bootstrap. ([#1124](https://github.com/pragma-org/amaru/pull/1124))
- **amaru-ledger**: reject governance proposals whose previous action does not match the enacted root nor an in-flight proposal of the same purpose. ([#1090][], [#932][])
- **amaru-kernel**: fix the decoding of data types that were not conforming to the specification ([#1172](https://github.com/pragma-org/amaru/issues/1172))
- **amaru-ledger**: a transaction marked invalid that carries no Plutus script is no longer accepted. ([#894][])
- **amaru-pure-stage**: avoid constructing short string errors when failing to cast types in pure-stage.
- **amaru**: fix a switch to fork when fork length was than one block. (#1162, #1190).

## [v10.11.20260807](https://github.com/pragma-org/amaru/releases/tag/v10.11.20260807)

### Added

- **amaru**: log precise build identity (package version, full git commit, dirty flag, OS/arch) at INFO after tracing is set up, so operator log files identify the running binary. ([#1161](https://github.com/pragma-org/amaru/issues/1161))
- **amaru**: add `amaru node rollback` to recover after a wrongly invalidated block or to rewind to an epoch start. Supports `--immutable-tip` (chain store only) and `--epoch` (ledger snapshot reset + chain realign). Clears all descendant validation flags, sets the anchor/best tip, and culls the best-chain fragment. ([#1072](https://github.com/pragma-org/amaru/issues/1072))
- **amaru**: add `amaru mithril sync` to download verified Mithril immutable files and replay their blocks directly into the chain and ledger stores.
- **amaru**: add `amaru node rm --wipe-all-dbs` to remove the ledger and chain databases resolved from the selected network.
- **amaru-tui**: `node run` now launches a feature-rich TUI that feeds from the emitted traces and metrics to provide an out-of-the-box dashboard for Amaru. Can be disabled with `--no-tui`.
- **amaru-consensus**: add basic peer performance tracking to select up to three peers whenever fetching blocks. ([#1093](https://github.com/pragma-org/amaru/issues/1093))

### Changed

- **amaru-stores**: bump chain DB schema to version 5. Migration from earlier versions is intentionally refused: opcert sequence numbers must come from a snapshot bootstrap (`amaru node rm --wipe-all-dbs` then `amaru node bootstrap`). Version-mismatch errors on `amaru node run` now suggest `amaru dev chain migrate` / `--migrate-chain-db`. ([#1152](https://github.com/pragma-org/amaru/issues/1152))
- **amaru**: prefer `amaru node rollback --epoch` over `amaru dev ledger reset` / legacy `reset-to-epoch`. The low-level ledger-only reset is hidden; the legacy alias now performs full recovery (ledger + chain realign). ([#1072](https://github.com/pragma-org/amaru/issues/1072))
- **amaru-consensus**: skip the validation of headers whose evolved nonces are already stored, to avoid unnecessary rechecks when getting the same header from different peers. ([#1087][])
- **amaru-ledger**: keep only slim stake summaries in runtime memory, and rebuild the full account-heavy stake distribution from snapshots when computing rewards.
- **amaru-ledger**: compute rewards and stake distributions asynchronously to prevent blocking the main roll forward loop from times to times.
- **amaru**: bootstrap snapshots now are retrieved directly from R2 (no embedded manifests) and compressed with zstandard. ([#1012][])
- **amaru**: metrics are now (also) exported through gRPC on `:4317` by default instead of `:4318` over HTTP.

### Removed

- **amaru**: remove `amaru dev ledger mithril` and `amaru dev ledger sync` in favor of `amaru mithril sync`.
- **amaru-kernel**: dependency on `pallas-*`

### Fixed

- **amaru-node**: invalidate peer snapshot commit metadata cache when switching to older or newer commits. ([#1114](https://github.com/pragma-org/amaru/issues/1114))
- **amaru-kernel**: reduce memory footprint of various types on the critical path.
- **amaru-ledger**: populate `recently_pruned_proposals` when importing a cardano-node snapshot.
- **amaru-ledger**: debit the treasury when enacted withdrawals are paid out at the epoch boundary ([#1118][])
- **amaru-ledger**: preserve the `Ratified` status of proposals pruned during ratification ([#1118][])
- **amaru-kernel**: reduce memory footprint of various types on the critical path.
- **amaru-ledger**: when a leader changes it reward account, the rewards owed to the previous account must return to the treasury ([#1125](https://github.com/pragma-org/amaru/pull/1125)).
- **amaru-consensus**: fix peer_selection to only schedule a single cool-down timer and thus properly bound priority mailbox usage. ([#1112](https://github.com/pragma-org/amaru/issues/1112))
- **amaru-ledger**: validate provided treasury value matches actual treasury value (ConwayTreasuryValueMismatch) ([#1025][], [#888][])
- **amaru-ledger**: reject governance proposals whose deposit return account is not registered. ([#928][])
- **amaru-uplc**: fix the validation of Plutus scripts containing lists of BLS elements when they are empty, since the Haskell node accepts them ([#1159](https://github.com/pragma-org/amaru/issues/1159))

## [v10.11.20260730](https://github.com/pragma-org/amaru/releases/tag/v10.11.20260730)

### Added

- **amaru-ledger**: reject treasury withdrawal proposals that reference unregistered reward accounts.  ([#1032][], [#929][])
- **amaru-ledger**: introduce `StakePoolCostTooLowPOOL` coverage. ([#1037][], [#909][])
- **amaru-consensus**: add events and metrics to track the performance of headers processing. ([#1005][])
- **amaru-ledger**: benchmarks for key volatile db operations (roll forward, switch to fork and context preparation).
- **amaru-ledger**: add stateful checks on withdrawals (drep delegation requirements + existence of credentials) ([#1011][], [#890][] [#895][])
- **amaru-consensus**: track pool opcert sequence numbers in the chain store and enforce the Praos rule
  that a pool sequence number minus its last known value must be at most 1.
  Sequence numbers are migrated from header already stored in the chain store and otherwise seeded
  from the cardano-node snapshot at bootstrap. ([#1021][])
- **amaru**: automate prometheus metrics comparison with cardano-node. ([#1075](https://github.com/pragma-org/amaru/pull/1075))

### Changed

- **amaru**: use `zst` compression for all individual stake distribution snapshots.
- **amaru**: removed `#[tokio::main]`; each subcommand builds its own Tokio runtime; signals are handled via `signal-hook` on the main thread (EDR 019). Unexpected consensus stage-graph death now exits non-zero. OpenTelemetry teardown is time-bounded.
- **amaru-ledger**: track account unregistrations to avoid O(n) scan on all accounts during epoch transition calculations.
- **amaru-ledger**: add phase-one conformance coverage for `TooManyCollateralInputs`, `ScriptsNotPaidUTxO` and `IncorrectTotalCollateralField`, and move the fixture that was filed under `InsufficientCollateral` while expecting `ValueNotConservedUTxO` to the directory matching its predicate.
- **amaru**: consolidate the monitoring stack.
- **amaru-consensus**: skip the validation of headers whose evolved nonces are already stored, to avoid unnecessary rechecks when getting the same header from different peers. ([#1087][])

### Fixed

- **amaru-ledger**: reject pool retirement when the retirement epoch is out of range. ([#1036][])
- **amaru-ledger**: validate stake pool exists when attempting to unregister ([#912][], [#1034][])
- **amaru-consensus**: fix the recheck deferred headers loop ([#1078][], [#1082][])
- **amaru**: process lifecycle no longer depends on the Tokio runtime to observe SIGINT/SIGTERM; first signal requests graceful shutdown (including main-thread stage abort), second signal force-exits (exit 130). Fixes hang during catch-up roll-forward ([#1061](https://github.com/pragma-org/amaru/pull/1061)).
- **amaru-node**: support Cardano ledger peer snapshots via `--peer-snapshot` / `AMARU_PEER_SNAPSHOT` for cold-start big-ledger peers in peer selection (complements `--peer-address`) ([#1047](https://github.com/pragma-org/amaru/pull/1047))
- **amaru-node**: embed best-effort peer snapshots for known networks (for example mainnet, preprod, preview) at build time from cardano-foundation/cardano-configurations; used by default when `--peer-snapshot` is omitted
- **amaru**: fix the start/restart of a node ([#1095][], [#1098][])
- **amaru-node**: fix build script to avoid hitting the github API too frequently ([#1108](https://github.com/pragma-org/amaru/pull/1108))
- **amaru-ledger**: fix the computation of pool updates ([#1109][])
- **amaru-ledger**: fix the handling of leader accounts for unclaimed rewards ([#1101][])

## [v10.11.20260723](https://github.com/pragma-org/amaru/releases/tag/v10.11.20260723)

### Added

- **amaru-ledger**: trace spans for the ledger rules (phase-one and phase-two). ([#1056][])
- **amaru-ledger**: run scripts in parallel within the same transaction. ([#1056][])
- **amaru / amaru-node**: break out the `amaru-node` crate which can then be used as a library to embed Amaru into other applications. ([#1054](https://github.com/pragma-org/amaru/pull/1054))

### Changed

- **amaru**: move the `node reset` command under `dev ledger`, where it belongs. ([#1055][])

### Removed

- **amaru**: no more `--force` flag on `node bootstrap`; if chain or ledger directories already exist, bootstrap aborts and asks the operator to remove them manually. ([#1062](https://github.com/pragma-org/amaru/pull/1062))
- **amaru**: no more separate `amaru-ledger` binary; associated commands have been moved into the main `amaru` binary under `amaru dev ledger`. ([#1064](https://github.com/pragma-org/amaru/pull/1064))

- **amaru-ledger**: add stateful checks on withdrawals (drep delegation requirements + existence of credentials) ([#1011][], [#890][])

### Fixed

- **amaru-ledger**: use effective collateral when collecting epoch fees for phase-2-invalid transactions. ([#1048][])
- **amaru-consensus**: gracefully handle header validation deferral due to missing stake distribution, clock skew, or exceeding lead over block application; also switch back to block height for the latter. ([#1041][])
- **amaru-consensus / amaru-protocols**: do not log an ERROR when block-fetch is paused because no upstream peers are connected yet; keep ERROR for real fetch timeouts after peers were contacted. ([#1050](https://github.com/pragma-org/amaru/issues/1050))
- **amaru-plutus**: encoding divergence between rational number present in governance actions and those present in protocol parameters. ([#1053][])
- **amaru-ledger**: restore some spans in the ledger at the debug level. ([#1056][])
- **amaru**: make sure that switching to a new fork is atomic and recovers in case a block on the fork fails to validate ([#1009][])
- **amaru**: bootstrap creates the chain DB at the current schema version instead of replaying migrations on an empty store (avoids a spurious migration warning). ([#1060](https://github.com/pragma-org/amaru/pull/1062))
- **amaru-protocols**: delegate connection attempts to connector stage to avoid blocking the manager and allow up to 10 concurrent connections. ([#1058](https://github.com/pragma-org/amaru/pull/1058))
- **amaru-consensus**: fix chainsync mini-protocol lifecycle handling in `track_peers` stage to properly clean up resources when stopping to sync from a peer. ([#1059](https://github.com/pragma-org/amaru/pull/1059))
- **amaru-pure-stage**: simulation runtime now also guarantees delivery of scheduled messages; both runtimes enforce limit on priority messages in flight. ([#1066](https://github.com/pragma-org/amaru/pull/1066))
- **amaru-uplc**: fixed the CBOR encoding of `-2^64`.
- **amaru-ledger**: unbind accounts from deregistered pools. ([#1030][])

## [v10.11.20260716](https://github.com/pragma-org/amaru/releases/tag/v10.11.20260716)

### Added

- **amaru-ledger**: validate the minimum transaction fee during phase-one. ([#820][])
- **amaru-ledger**: enforce the per-transaction and per-block limits on the total size of reference scripts. ([#820][])
- **amaru-ledger**: add more state elements to the validation context, enabling the introduction of ledger predicates that depend on state such as pools, governance, and more. ([#831][], [#896][], [#902][], [#915][], [#975][], [#1017][])
- **amaru-ledger**: validate value preservation across (valid and invalid) transactions. ([#892][], [#831][])

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

## [v10.10.20260709](https://github.com/pragma-org/amaru/releases/tag/v10.10.20260709)

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
[#888]: https://github.com/pragma-org/amaru/issues/888
[#890]: https://github.com/pragma-org/amaru/issues/890
[#892]: https://github.com/pragma-org/amaru/issues/892
[#894]: https://github.com/pragma-org/amaru/issues/894
[#895]: https://github.com/pragma-org/amaru/issues/895
[#896]: https://github.com/pragma-org/amaru/issues/896
[#899]: https://github.com/pragma-org/amaru/issues/899
[#902]: https://github.com/pragma-org/amaru/issues/902
[#909]: https://github.com/pragma-org/amaru/issues/909
[#912]: https://github.com/pragma-org/amaru/issues/912
[#915]: https://github.com/pragma-org/amaru/issues/915
[#923]: https://github.com/pragma-org/amaru/issues/923
[#924]: https://github.com/pragma-org/amaru/issues/924
[#926]: https://github.com/pragma-org/amaru/issues/926
[#928]: https://github.com/pragma-org/amaru/issues/928
[#929]: https://github.com/pragma-org/amaru/issues/929
[#931]: https://github.com/pragma-org/amaru/issues/931
[#932]: https://github.com/pragma-org/amaru/issues/932
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
[#1005]: https://github.com/pragma-org/amaru/pull/1005
[#1009]: https://github.com/pragma-org/amaru/pull/1009
[#1010]: https://github.com/pragma-org/amaru/pull/1010
[#1011]: https://github.com/pragma-org/amaru/pull/1011
[#1012]: https://github.com/pragma-org/amaru/pull/1012
[#1013]: https://github.com/pragma-org/amaru/pull/1013
[#1017]: https://github.com/pragma-org/amaru/pull/1017
[#1021]: https://github.com/pragma-org/amaru/pull/1021
[#1024]: https://github.com/pragma-org/amaru/pull/1024
[#1025]: https://github.com/pragma-org/amaru/pull/1025
[#1026]: https://github.com/pragma-org/amaru/pull/1026
[#1027]: https://github.com/pragma-org/amaru/pull/1027
[#1029]: https://github.com/pragma-org/amaru/pull/1029
[#1030]: https://github.com/pragma-org/amaru/pull/1030
[#1031]: https://github.com/pragma-org/amaru/pull/1031
[#1032]: https://github.com/pragma-org/amaru/pull/1032
[#1033]: https://github.com/pragma-org/amaru/pull/1033
[#1034]: https://github.com/pragma-org/amaru/pull/1034
[#1036]: https://github.com/pragma-org/amaru/pull/1036
[#1037]: https://github.com/pragma-org/amaru/pull/1037
[#1039]: https://github.com/pragma-org/amaru/pull/1039
[#1041]: https://github.com/pragma-org/amaru/pull/1041
[#1043]: https://github.com/pragma-org/amaru/pull/1043
[#1048]: https://github.com/pragma-org/amaru/pull/1048
[#1053]: https://github.com/pragma-org/amaru/pull/1053
[#1055]: https://github.com/pragma-org/amaru/pull/1055
[#1056]: https://github.com/pragma-org/amaru/pull/1056
[#1060]: https://github.com/pragma-org/amaru/issues/1060
[#1078]: https://github.com/pragma-org/amaru/issues/1078
[#1082]: https://github.com/pragma-org/amaru/pull/1082
[#1087]: https://github.com/pragma-org/amaru/pull/1087
[#1090]: https://github.com/pragma-org/amaru/pull/1090
[#1094]: https://github.com/pragma-org/amaru/issues/1094
[#1095]: https://github.com/pragma-org/amaru/issues/1095
[#1098]: https://github.com/pragma-org/amaru/pull/1098
[#1101]: https://github.com/pragma-org/amaru/pull/1101
[#1109]: https://github.com/pragma-org/amaru/pull/1109
[#1118]: https://github.com/pragma-org/amaru/pull/1118
[#1138]: https://github.com/pragma-org/amaru/pull/1138
[#1139]: https://github.com/pragma-org/amaru/pull/1139
[#1143]: https://github.com/pragma-org/amaru/pull/1143
