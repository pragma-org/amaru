# Available Spans

This document lists all available spans in Amaru, auto-generated from the code.

For information on how to use and filter these spans, see [monitoring/README.md](../monitoring/README.md).


## target: `amaru::bootstrap::accounts`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import accounts from a snapshot | size |  |
| `is_not_empty` | `TRACE` | public | Existing accounts found in the store before import |  |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::block_issuers`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import block issuers from a snapshot | count |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `count` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::constitution`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import the constitution from a snapshot | anchor, guardrails |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `anchor` | `string` | ✓ |
| `guardrails` | `string` | ✓ |

</details>

## target: `amaru::bootstrap::constitutional_committee`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import the constitutional committee from a snapshot | state | threshold, members |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `state` | `string` | ✓ |
| `threshold` | `string` |  |
| `members` | `integer` |  |

</details>

## target: `amaru::bootstrap::dreps`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import DReps from a snapshot | size |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::fetch`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `rollback` | `TRACE` | public | Received a rollback while fetching bootstrap headers | point, tip |  |

<details><summary>span: `rollback`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `string` | ✓ |
| `tip` | `array` | ✓ |

</details>

## target: `amaru::bootstrap::governance_activity`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import the governance activity from a snapshot | dormant_epochs |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `dormant_epochs` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::header`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import a single header into the chain store | header |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `header` | `string` | ✓ |

</details>

## target: `amaru::bootstrap::headers`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `fetch` | `TRACE` | public | Fetch bootstrap headers from a peer | requested_point, intersection, headers_per_point |  |
| `next_failed` | `TRACE` | public | The chain-sync client failed while requesting or awaiting the next header. Operation ∈ {request_next, await_next}. | operation, error |  |

<details><summary>span: `fetch`</summary>

| field | type | required |
| --- | --- | --- |
| `requested_point` | `string` | ✓ |
| `intersection` | `string` | ✓ |
| `headers_per_point` | `integer` | ✓ |

</details>

<details><summary>span: `next_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `operation` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

## target: `amaru::bootstrap::import`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `utxo` | `TRACE` | public | Import UTxO entries from a snapshot | size |  |

<details><summary>span: `utxo`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::nonces`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import initial nonces into the chain store | point |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `array` | ✓ |

</details>

## target: `amaru::bootstrap::opcert_sequence_numbers`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import initial opcert sequence numbers into the chain store | point |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `array` | ✓ |

</details>

## target: `amaru::bootstrap::peer`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `failed_to_connect` | `TRACE` | public | Failed to connect to a peer while bootstrapping | peer, reason |  |

<details><summary>span: `failed_to_connect`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

## target: `amaru::bootstrap::pots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import treasury/reserves/fees pots from a snapshot | treasury, reserves, fees, donations |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `treasury` | `integer` | ✓ |
| `reserves` | `integer` | ✓ |
| `fees` | `integer` | ✓ |
| `donations` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::proposal_roots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import governance proposal roots from a snapshot | constitution, constitutional_committee, hard_fork, protocol_parameters |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `constitution` | `string` | ✓ |
| `constitutional_committee` | `string` | ✓ |
| `hard_fork` | `string` | ✓ |
| `protocol_parameters` | `string` | ✓ |

</details>

## target: `amaru::bootstrap::proposals`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import governance proposals from a snapshot | size |  |
| `is_not_empty` | `TRACE` | public | Existing proposals found in the store before import |  |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::recently_pruned_proposals`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import proposals pruned at the snapshot's epoch boundary, from its ratify state | size |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::snapshot`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `download` | `TRACE` | public | Download a snapshot archive | epoch, point |  |
| `import_archive` | `TRACE` | public | Import a compressed snapshot archive | path |  |
| `import_tvar` | `TRACE` | public | Import from the tvar data | point, new_epoch_state_offset |  |
| `skip_download` | `TRACE` | public | Snapshot already downloaded; skipping download | snapshot |  |
| `unexpected_era` | `TRACE` | public | The parsed snapshot's current era is not Conway; later decoding may fail | snapshot_era |  |

<details><summary>span: `download`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `point` | `string` | ✓ |

</details>

<details><summary>span: `import_archive`</summary>

| field | type | required |
| --- | --- | --- |
| `path` | `string` | ✓ |

</details>

<details><summary>span: `import_tvar`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `array` | ✓ |
| `new_epoch_state_offset` | `integer` | ✓ |

</details>

<details><summary>span: `skip_download`</summary>

| field | type | required |
| --- | --- | --- |
| `snapshot` | `string` | ✓ |

</details>

<details><summary>span: `unexpected_era`</summary>

| field | type | required |
| --- | --- | --- |
| `snapshot_era` | `string` | ✓ |

</details>

## target: `amaru::bootstrap::snapshots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import all snapshots | count |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `count` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::stake_pools`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import stake pools from a snapshot | registered, retiring |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `registered` | `integer` | ✓ |
| `retiring` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::votes`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import governance votes from a snapshot | size |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::cli`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `error` | `TRACE` | public | Process terminated with an error. | description | cause |

<details><summary>span: `error`</summary>

| field | type | required |
| --- | --- | --- |
| `description` | `string` | ✓ |
| `cause` | `string` |  |

</details>

## target: `amaru::cli::chain_db`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `exist` | `TRACE` | public | Chain database already exists | dir, hint |  |

<details><summary>span: `exist`</summary>

| field | type | required |
| --- | --- | --- |
| `dir` | `string` | ✓ |
| `hint` | `string` | ✓ |

</details>

## target: `amaru::cli::current_epoch`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `resolve` | `TRACE` | public | Resolve the current epoch from Koios | epoch |  |

<details><summary>span: `resolve`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

## target: `amaru::cli::db_analyser`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `reuse_ledger_snapshot` | `TRACE` | public | Reuse an existing db-analyser ledger snapshot | epoch, slot, snapshot |  |
| `run` | `TRACE` | public | Run db-analyser to produce a ledger snapshot | epoch, slot | analyse_from |

<details><summary>span: `reuse_ledger_snapshot`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `snapshot` | `string` | ✓ |

</details>

<details><summary>span: `run`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `analyse_from` | `integer` |  |

</details>

## target: `amaru::cli::dev`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `run` | `TRACE` | public | A developer command started, with the arguments it resolved. Command names the subcommand, e.g. "dev chain prune". | command, network | chain_dir, ledger_dir, headers_dir, input, start, block, parent, peer_address, epoch, count, from_point, only_blocks, only_validation_results, hint |

<details><summary>span: `run`</summary>

| field | type | required |
| --- | --- | --- |
| `command` | `string` | ✓ |
| `network` | `string` | ✓ |
| `chain_dir` | `string` |  |
| `ledger_dir` | `string` |  |
| `headers_dir` | `string` |  |
| `input` | `string` |  |
| `start` | `string` |  |
| `block` | `string` |  |
| `parent` | `string` |  |
| `peer_address` | `string` |  |
| `epoch` | `string` |  |
| `count` | `integer` |  |
| `from_point` | `string` |  |
| `only_blocks` | `boolean` |  |
| `only_validation_results` | `boolean` |  |
| `hint` | `string` |  |

</details>

## target: `amaru::cli::dev::chain`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `anchor_updated` | `TRACE` | public | The chain store anchor was moved to a new hash | new_anchor |  |
| `migration_not_needed` | `TRACE` | public | The chain database is already at the current version |  |  |
| `moving_best_chain` | `TRACE` | public | The best chain hash is being moved back before removing points |  |  |
| `open_failed` | `TRACE` | public | The chain database could not be opened | error |  |
| `parent_not_found` | `TRACE` | public | A header on the path back to the best chain has no stored parent | header_hash |  |
| `point_removed` | `TRACE` | public | A point is being removed from the chain store | point |  |
| `points_to_remove` | `TRACE` | public | The number of stored points selected for removal | points |  |
| `prune_boundary` | `TRACE` | public | The pruning boundary derived from the oldest ledger snapshot | oldest_ledger_epoch, boundary_slot |  |
| `validation_cleared` | `TRACE` | public | The stored validation status of a block is being cleared | header_hash |  |

<details><summary>span: `anchor_updated`</summary>

| field | type | required |
| --- | --- | --- |
| `new_anchor` | `string` | ✓ |

</details>

<details><summary>span: `open_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `parent_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `header_hash` | `string` | ✓ |

</details>

<details><summary>span: `point_removed`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `array` | ✓ |

</details>

<details><summary>span: `points_to_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `points` | `integer` | ✓ |

</details>

<details><summary>span: `prune_boundary`</summary>

| field | type | required |
| --- | --- | --- |
| `oldest_ledger_epoch` | `integer` | ✓ |
| `boundary_slot` | `integer` | ✓ |

</details>

<details><summary>span: `validation_cleared`</summary>

| field | type | required |
| --- | --- | --- |
| `header_hash` | `string` | ✓ |

</details>

## target: `amaru::cli::dev::ledger`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `snapshot_not_found` | `TRACE` | public | A ledger snapshot to remove does not exist | epoch |  |
| `snapshot_removed` | `TRACE` | public | A ledger snapshot was removed | epoch |  |

<details><summary>span: `snapshot_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `snapshot_removed`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

## target: `amaru::cli::last_block`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `resolve` | `TRACE` | public | Resolve the last produced block for an epoch | epoch, point |  |

<details><summary>span: `resolve`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `point` | `string` | ✓ |

</details>

## target: `amaru::cli::ledger_db`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `exist` | `TRACE` | public | Ledger database already exists | dir, hint |  |

<details><summary>span: `exist`</summary>

| field | type | required |
| --- | --- | --- |
| `dir` | `string` | ✓ |
| `hint` | `string` | ✓ |

</details>

## target: `amaru::cli::mithril`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `download` | `TRACE` | public | Synchronize the cardano-node database from Mithril | from_chunk, target_dir |  |
| `download_chunks` | `TRACE` | public | Immutable chunks are being fetched from Mithril | tip, from_chunk |  |
| `ingest_completed` | `TRACE` | public | Finished replaying downloaded blocks into the stores | processed, duration_seconds, processed_per_seconds |  |
| `skip_download` | `TRACE` | public | Local cardano-node database is recent enough; skipping Mithril download | from_chunk, required_chunk, target_dir, reason |  |

<details><summary>span: `download`</summary>

| field | type | required |
| --- | --- | --- |
| `from_chunk` | `integer` | ✓ |
| `target_dir` | `string` | ✓ |

</details>

<details><summary>span: `download_chunks`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `from_chunk` | `integer` | ✓ |

</details>

<details><summary>span: `ingest_completed`</summary>

| field | type | required |
| --- | --- | --- |
| `processed` | `integer` | ✓ |
| `duration_seconds` | `number` | ✓ |
| `processed_per_seconds` | `number` | ✓ |

</details>

<details><summary>span: `skip_download`</summary>

| field | type | required |
| --- | --- | --- |
| `from_chunk` | `integer` | ✓ |
| `required_chunk` | `integer` | ✓ |
| `target_dir` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

## target: `amaru::cli::node`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `bootstrap` | `TRACE` | public | Bootstrap a node from published snapshots | chain_dir, ledger_dir, network | epoch |
| `rm` | `TRACE` | public | Remove ledger and chain database from disk | chain_dir, ledger_dir, network |  |
| `rollback` | `TRACE` | public | Roll the node databases back after a failure | chain_dir, ledger_dir, network, mode | epoch, ledger_tip, best_chain, anchor |
| `run` | `TRACE` | public | The effective configuration a node run starts with | chain_dir, ledger_dir, listen_address, max_extra_ledger_snapshots, migrate_chain_db, network, peer_address, peer_snapshot, peer_snapshot_relays, pid_file, submit_api_address, trace_buffer_min_entries, trace_buffer_max_size, trace_dump_path, peer_removal_cooldown_secs, mempool_max_bytes, tx_submission_max_window, tx_submission_fetch_batch_bytes, tx_submission_inflight_timeout_ms, tx_submission_insert_timeout_ms | era_history, global_parameters |
| `submit_api_shutdown_failed` | `TRACE` | public | The submit API did not stop cleanly during shutdown. Reason ∈ {join_error, timeout}. | reason | error |

<details><summary>span: `bootstrap`</summary>

| field | type | required |
| --- | --- | --- |
| `chain_dir` | `string` | ✓ |
| `ledger_dir` | `string` | ✓ |
| `network` | `string` | ✓ |
| `epoch` | `integer` |  |

</details>

<details><summary>span: `rm`</summary>

| field | type | required |
| --- | --- | --- |
| `chain_dir` | `string` | ✓ |
| `ledger_dir` | `string` | ✓ |
| `network` | `string` | ✓ |

</details>

<details><summary>span: `rollback`</summary>

| field | type | required |
| --- | --- | --- |
| `chain_dir` | `string` | ✓ |
| `ledger_dir` | `string` | ✓ |
| `network` | `string` | ✓ |
| `mode` | `string` | ✓ |
| `epoch` | `integer` |  |
| `ledger_tip` | `string` |  |
| `best_chain` | `string` |  |
| `anchor` | `string` |  |

</details>

<details><summary>span: `run`</summary>

| field | type | required |
| --- | --- | --- |
| `chain_dir` | `string` | ✓ |
| `ledger_dir` | `string` | ✓ |
| `listen_address` | `string` | ✓ |
| `max_extra_ledger_snapshots` | `string` | ✓ |
| `migrate_chain_db` | `boolean` | ✓ |
| `network` | `string` | ✓ |
| `peer_address` | `string` | ✓ |
| `peer_snapshot` | `string` | ✓ |
| `peer_snapshot_relays` | `integer` | ✓ |
| `pid_file` | `string` | ✓ |
| `submit_api_address` | `string` | ✓ |
| `trace_buffer_min_entries` | `integer` | ✓ |
| `trace_buffer_max_size` | `integer` | ✓ |
| `trace_dump_path` | `string` | ✓ |
| `peer_removal_cooldown_secs` | `integer` | ✓ |
| `mempool_max_bytes` | `string` | ✓ |
| `tx_submission_max_window` | `integer` | ✓ |
| `tx_submission_fetch_batch_bytes` | `integer` | ✓ |
| `tx_submission_inflight_timeout_ms` | `integer` | ✓ |
| `tx_submission_insert_timeout_ms` | `integer` | ✓ |
| `era_history` | `string` |  |
| `global_parameters` | `string` |  |

</details>

<details><summary>span: `submit_api_shutdown_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `reason` | `string` | ✓ |
| `error` | `string` |  |

</details>

## target: `amaru::cli::snapshot`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `create` | `TRACE` | public | Create snapshots for the given network | network, snapshot_output_dir, config_dir, cardano_node_db, dist_dir | epoch, snapshots |
| `created` | `TRACE` | public | Finished creating a snapshot archive | epoch, slot, archive |  |
| `package` | `TRACE` | public | Package a snapshot archive | epoch, slot, archive |  |
| `publish` | `TRACE` | public | Publish snapshot archives | network, local, remote |  |
| `skip_package` | `TRACE` | public | Snapshot archive already packaged; skipping | epoch, slot, archive, reason |  |
| `skip_upload` | `TRACE` | public | Snapshot archive already uploaded; skipping | archive |  |
| `update_index` | `TRACE` | public | Update the published snapshot index | network, snapshots |  |
| `upload` | `TRACE` | public | Upload a snapshot archive | archive |  |
| `uploaded` | `TRACE` | public | Finished uploading a snapshot archive | archive |  |

<details><summary>span: `create`</summary>

| field | type | required |
| --- | --- | --- |
| `network` | `string` | ✓ |
| `snapshot_output_dir` | `string` | ✓ |
| `config_dir` | `string` | ✓ |
| `cardano_node_db` | `string` | ✓ |
| `dist_dir` | `string` | ✓ |
| `epoch` | `integer` |  |
| `snapshots` | `string` |  |

</details>

<details><summary>span: `created`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `archive` | `string` | ✓ |

</details>

<details><summary>span: `package`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `archive` | `string` | ✓ |

</details>

<details><summary>span: `publish`</summary>

| field | type | required |
| --- | --- | --- |
| `network` | `string` | ✓ |
| `local` | `integer` | ✓ |
| `remote` | `integer` | ✓ |

</details>

<details><summary>span: `skip_package`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `archive` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `skip_upload`</summary>

| field | type | required |
| --- | --- | --- |
| `archive` | `string` | ✓ |

</details>

<details><summary>span: `update_index`</summary>

| field | type | required |
| --- | --- | --- |
| `network` | `string` | ✓ |
| `snapshots` | `integer` | ✓ |

</details>

<details><summary>span: `upload`</summary>

| field | type | required |
| --- | --- | --- |
| `archive` | `string` | ✓ |

</details>

<details><summary>span: `uploaded`</summary>

| field | type | required |
| --- | --- | --- |
| `archive` | `string` | ✓ |

</details>

## target: `amaru::consensus::block`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `adopt_failed` | `TRACE` | public | Adopting a tip as the new best chain failed. Step ∈ {adopt_tip, adopt_first_tip, drag_anchor_forward}. | tip, step, error |  |
| `apply_failed` | `TRACE` | public | A block could not be applied to the ledger. Step ∈ {validate_block, switch_to_fork}. | tip, step, error |  |
| `header_not_found` | `TRACE` | public | A header needed to adopt a tip could not be loaded. Role ∈ {incoming_tip, current_best}. | role | tip |
| `invalid` | `TRACE` | public | A block was rejected during validation | failed_tip, parent, error, detail |  |
| `invariant_violated` | `TRACE` | public | The chain store contradicts itself while adopting a tip. Invariant ∈ {header_missing, no_common_ancestor}. | tip, invariant |  |
| `mismatched_hash` | `TRACE` | public | Mismatched body hash after download, the peer is adversarial | peer, header_hash | expected, actual |
| `skip` | `TRACE` | public | Skip a block validation when it is not better than the current ledger tip | current, tip |  |
| `switch_fork` | `TRACE` | public | The ledger is switching to a different fork | current, parent |  |
| `validate_from_genesis` | `TRACE` | public | Block validation cannot proceed because the parent is the genesis block | tip, current, parent |  |

<details><summary>span: `adopt_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `step` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `apply_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `step` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `header_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `role` | `string` | ✓ |
| `tip` | `array` |  |

</details>

<details><summary>span: `invalid`</summary>

| field | type | required |
| --- | --- | --- |
| `failed_tip` | `array` | ✓ |
| `parent` | `array` | ✓ |
| `error` | `string` | ✓ |
| `detail` | `string` | ✓ |

</details>

<details><summary>span: `invariant_violated`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `invariant` | `string` | ✓ |

</details>

<details><summary>span: `mismatched_hash`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `header_hash` | `string` | ✓ |
| `expected` | `string` |  |
| `actual` | `string` |  |

</details>

<details><summary>span: `skip`</summary>

| field | type | required |
| --- | --- | --- |
| `current` | `array` | ✓ |
| `tip` | `array` | ✓ |

</details>

<details><summary>span: `switch_fork`</summary>

| field | type | required |
| --- | --- | --- |
| `current` | `array` | ✓ |
| `parent` | `array` | ✓ |

</details>

<details><summary>span: `validate_from_genesis`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `current` | `array` | ✓ |
| `parent` | `array` | ✓ |

</details>

## target: `amaru::consensus::block_source`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `known_invalid` | `TRACE` | public | A peer announced a block already known to be invalid | peer, point |  |

<details><summary>span: `known_invalid`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `point` | `array` | ✓ |

</details>

## target: `amaru::consensus::blocks`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `decode_failed` | `TRACE` | public | Failed to decode a block received from a peer | peer, error |  |
| `find_missing_failed` | `TRACE` | public | Failed to compute the set of missing blocks | error |  |
| `header_not_found` | `TRACE` | public | A header required for block fetching could not be loaded from the store | header_hash |  |
| `nothing_to_fetch` | `TRACE` | public | The batch of missing blocks is empty; resume fetching from the tip | tip, parent |  |
| `paused` | `TRACE` | public | Block fetching paused because no upstream peers are available | req_id |  |
| `point_mismatch` | `TRACE` | public | Received a block out of order: its point is not the next missing point | actual | expected |
| `recover_failed` | `TRACE` | public | Failed to check whether a stored block exists during startup recovery | error, header_hash |  |
| `recover_inconsistent` | `TRACE` | public | Startup recovery found an inconsistent stored chain. Reason ∈ {ledger_tip_is_origin, broken_chain}. | from, to, reason |  |
| `store_failed` | `TRACE` | public | Failed to persist a downloaded block | error |  |
| `timeout` | `TRACE` | public | Timed out waiting for requested blocks | req_id |  |

<details><summary>span: `decode_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `find_missing_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `header_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `header_hash` | `string` | ✓ |

</details>

<details><summary>span: `nothing_to_fetch`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `parent` | `array` | ✓ |

</details>

<details><summary>span: `paused`</summary>

| field | type | required |
| --- | --- | --- |
| `req_id` | `integer` | ✓ |

</details>

<details><summary>span: `point_mismatch`</summary>

| field | type | required |
| --- | --- | --- |
| `actual` | `array` | ✓ |
| `expected` | `array` |  |

</details>

<details><summary>span: `recover_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |
| `header_hash` | `string` | ✓ |

</details>

<details><summary>span: `recover_inconsistent`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `array` | ✓ |
| `to` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `store_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `timeout`</summary>

| field | type | required |
| --- | --- | --- |
| `req_id` | `integer` | ✓ |

</details>

## target: `amaru::consensus::chain`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `best_tip_candidate` | `TRACE` | public | A new candidate was chosen as the best tip. Reason ∈ {better_chain, previous_invalidated}. | tip, reason | previous |
| `best_tip_invalidated` | `TRACE` | public | The best tip candidate was invalidated and forks depending on it were dropped | removed |  |
| `fallback_to_origin` | `TRACE` | public | No valid candidate remains; the best chain falls back to origin |  |  |
| `fetch_next` | `TRACE` | public | Some blocks have been fetched for the current chain, decide what to do next | point, header_hash |  |
| `find_best_candidate_failed` | `TRACE` | public | Failed to select a new best candidate after an invalidation | error |  |
| `find_intersection` | `TRACE` | public | Find chain intersection point with peer | peer, intersection_slot |  |
| `forks_removed` | `TRACE` | public | Chain forks were removed because they depend on an invalid block | removed |  |
| `header_not_found` | `TRACE` | public | A header needed for chain selection could not be loaded from the store. Role ∈ {tip, best_candidate, best_candidate_parent, parent, validation_target}. | role, header_hash | tip |
| `resume_fetch` | `TRACE` | public | Where block fetching resumes from, once per request. Outcome ∈ {resume_from_best_tip, already_at_best_tip, no_best_tip}; only \`resume_from_best_tip\` sends a tip downstream and carries its \`parent\`. | outcome, point, best_tip | parent |
| `select_from_block_validation` | `TRACE` | public | Received a block validation result | point, valid, header_hash |  |
| `select_from_tip` | `TRACE` | public | Received a new tip from an upstream peer | tip, header_hash |  |
| `store_validation_failed` | `TRACE` | public | Failed to persist the validation result of a block | error, valid |  |
| `tip_accepted` | `TRACE` | public | A tip announced by an upstream peer is new and starts or extends a chain. Outcome ∈ {new_tip, from_origin, extend, fork}. | tip, outcome | parent |
| `tip_ignored` | `TRACE` | public | A tip announced by an upstream peer was not adopted. Reason ∈ {already_validated, already_invalid, already_tracked, invalid_ancestor}. | tip, reason | parent |

<details><summary>span: `best_tip_candidate`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `reason` | `string` | ✓ |
| `previous` | `array` |  |

</details>

<details><summary>span: `best_tip_invalidated`</summary>

| field | type | required |
| --- | --- | --- |
| `removed` | `integer` | ✓ |

</details>

<details><summary>span: `fetch_next`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `array` | ✓ |
| `header_hash` | `string` | ✓ |

</details>

<details><summary>span: `find_best_candidate_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `find_intersection`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `intersection_slot` | `integer` | ✓ |

</details>

<details><summary>span: `forks_removed`</summary>

| field | type | required |
| --- | --- | --- |
| `removed` | `integer` | ✓ |

</details>

<details><summary>span: `header_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `role` | `string` | ✓ |
| `header_hash` | `string` | ✓ |
| `tip` | `array` |  |

</details>

<details><summary>span: `resume_fetch`</summary>

| field | type | required |
| --- | --- | --- |
| `outcome` | `string` | ✓ |
| `point` | `array` | ✓ |
| `best_tip` | `array` | ✓ |
| `parent` | `array` |  |

</details>

<details><summary>span: `select_from_block_validation`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `array` | ✓ |
| `valid` | `boolean` | ✓ |
| `header_hash` | `string` | ✓ |

</details>

<details><summary>span: `select_from_tip`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `header_hash` | `string` | ✓ |

</details>

<details><summary>span: `store_validation_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |
| `valid` | `boolean` | ✓ |

</details>

<details><summary>span: `tip_accepted`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `outcome` | `string` | ✓ |
| `parent` | `array` |  |

</details>

<details><summary>span: `tip_ignored`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |
| `reason` | `string` | ✓ |
| `parent` | `array` |  |

</details>

## target: `amaru::consensus::chain_db_migration`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `execute` | `TRACE` | public | Migrate the database if necessary | from, to |  |
| `reset_best_chain` | `TRACE` | public | Reset the best chain to the anchor during migration so blocks are revalidated | prev_best_chain, new_best_chain |  |
| `warn` | `TRACE` | public | A database migration relies on an assumption that may not hold; see the reason | to, reason |  |

<details><summary>span: `execute`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `integer` | ✓ |
| `to` | `integer` | ✓ |

</details>

<details><summary>span: `reset_best_chain`</summary>

| field | type | required |
| --- | --- | --- |
| `prev_best_chain` | `string` | ✓ |
| `new_best_chain` | `string` | ✓ |

</details>

<details><summary>span: `warn`</summary>

| field | type | required |
| --- | --- | --- |
| `to` | `integer` | ✓ |
| `reason` | `string` | ✓ |

</details>

## target: `amaru::consensus::chainsync`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `initialized` | `TRACE` | public | A chainsync session with an upstream peer was initialized | peer, conn_id |  |
| `intersect_found` | `TRACE` | public | An intersection with the peer's chain was found | peer, conn_id, current, highest |  |
| `intersect_not_found` | `TRACE` | public | No intersection with the peer's chain was found, so chainsync with it stops | peer, highest |  |
| `reinitialized` | `TRACE` | public | A chainsync session was re-initialized while still active; prior state is purged | peer, conn_id |  |
| `roll_backward` | `TRACE` | public | A peer rolled back to an earlier point | peer, current, highest |  |
| `roll_backward_failed` | `TRACE` | public | A rollback requested by a peer could not be applied; the peer is adversarial | peer, error |  |
| `terminated` | `TRACE` | public | A chainsync session terminated and its connection state was purged | peer, conn_id |  |
| `unknown_intersection_point` | `TRACE` | public | The peer intersected on a point absent from our own store, so chainsync with it stops. Unlike \`INTERSECT_NOT_FOUND\` this points at local state, not at the peer. | peer, current, highest |  |

<details><summary>span: `initialized`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |

</details>

<details><summary>span: `intersect_found`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `current` | `array` | ✓ |
| `highest` | `array` | ✓ |

</details>

<details><summary>span: `intersect_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `highest` | `array` | ✓ |

</details>

<details><summary>span: `reinitialized`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |

</details>

<details><summary>span: `roll_backward`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `current` | `array` | ✓ |
| `highest` | `array` | ✓ |

</details>

<details><summary>span: `roll_backward_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `terminated`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |

</details>

<details><summary>span: `unknown_intersection_point`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `current` | `array` | ✓ |
| `highest` | `array` | ✓ |

</details>

## target: `amaru::consensus::perf::fork`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `switch` | `TRACE` | public | Event recorded when a fork switch ends. \`duration_micros\` measures the time from the detection of the fork to its application (or abandonment). | header_hash | outcome, duration_micros |

<details><summary>span: `switch`</summary>

| field | type | required |
| --- | --- | --- |
| `header_hash` | `string` | ✓ |
| `outcome` | `string` |  |
| `duration_micros` | `integer` |  |

</details>

## target: `amaru::consensus::perf::header`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `lifecycle` | `TRACE` | public | Event recorded once per header, when its processing reaches a terminal state. It covers the four network-health processing points of a header's lifecycle: reception of the header, request of its block, reception of its block and local adoption of the block. \`outcome\` describes the terminal state (including headers rejected on reception, which carry no durations). The optional durations are the intervals between those points: - \`block_fetch_wait_micros\`: reception of the header to the request of its block - \`block_fetch_micros\`: request of the block to its reception - \`forward_micros\`: reception of the header to the adoption of its block |  | peer, header_hash, outcome, error, slot_start_to_header_micros, block_fetch_wait_micros, block_fetch_micros, forward_micros |

<details><summary>span: `lifecycle`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` |  |
| `header_hash` | `string` |  |
| `outcome` | `string` |  |
| `error` | `string` |  |
| `slot_start_to_header_micros` | `integer` |  |
| `block_fetch_wait_micros` | `integer` |  |
| `block_fetch_micros` | `integer` |  |
| `forward_micros` | `integer` |  |

</details>

## target: `amaru::consensus::performance`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `queue_lagging` | `TRACE` | public | The performance operation queue is growing faster than the worker drains it | queue_depth |  |
| `queue_overflow` | `TRACE` | public | The performance operation queue exceeded its hard limit; the node aborts | queue_depth, threshold |  |
| `worker_panicked` | `TRACE` | public | The performance worker thread stopped because it panicked |  |  |

<details><summary>span: `queue_lagging`</summary>

| field | type | required |
| --- | --- | --- |
| `queue_depth` | `integer` | ✓ |

</details>

<details><summary>span: `queue_overflow`</summary>

| field | type | required |
| --- | --- | --- |
| `queue_depth` | `integer` | ✓ |
| `threshold` | `integer` | ✓ |

</details>

## target: `amaru::consensus::tip`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `adopt` | `TRACE` | public | Adopt a tip as the next tip in the best chain | slot, header_hash, block_height, max_block_height, suppressed |  |

<details><summary>span: `adopt`</summary>

| field | type | required |
| --- | --- | --- |
| `slot` | `integer` | ✓ |
| `header_hash` | `string` | ✓ |
| `block_height` | `integer` | ✓ |
| `max_block_height` | `integer` | ✓ |
| `suppressed` | `integer` | ✓ |

</details>

## target: `amaru::ledger::account`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `pay_or_refund` | `TRACE` | public | Pay withdrawals to an account, or refund its deposit | credential_type, account, deposit |  |

<details><summary>span: `pay_or_refund`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` | ✓ |
| `account` | `string` | ✓ |
| `deposit` | `integer` | ✓ |

</details>

## target: `amaru::ledger::block`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `apply` | `TRACE` | public | Apply a block to stable state | point_slot |  |
| `prepare` | `TRACE` | public | Prepare block for validation |  |  |

<details><summary>span: `apply`</summary>

| field | type | required |
| --- | --- | --- |
| `point_slot` | `integer` | ✓ |

</details>

## target: `amaru::ledger::block_validation_context`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `create` | `TRACE` | public | Create validation context for a block | block_id, block_number, block_body_size | total_inputs |

<details><summary>span: `create`</summary>

| field | type | required |
| --- | --- | --- |
| `block_id` | `string` | ✓ |
| `block_number` | `integer` | ✓ |
| `block_body_size` | `integer` | ✓ |
| `total_inputs` | `integer` |  |

</details>

## target: `amaru::ledger::chain_growth`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `violate` | `TRACE` | public | Fewer than k blocks were seen within the stability window | unstable_tail_length, reason |  |

<details><summary>span: `violate`</summary>

| field | type | required |
| --- | --- | --- |
| `unstable_tail_length` | `integer` | ✓ |
| `reason` | `string` | ✓ |

</details>

## target: `amaru::ledger::constitutional_committee`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `ignore` | `TRACE` | public | The constitutional committee votes were ignored during ratification | active_members, min_committee_size, reason |  |

<details><summary>span: `ignore`</summary>

| field | type | required |
| --- | --- | --- |
| `active_members` | `integer` | ✓ |
| `min_committee_size` | `integer` | ✓ |
| `reason` | `string` | ✓ |

</details>

## target: `amaru::ledger::epoch_transition`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `apply` | `TRACE` | public | Flushing the epoch transition overlay to disk | epoch | should_end_epoch, should_snapshot, should_begin_epoch |
| `compute` | `TRACE` | public | Epoch transition processing | from, into | skipped, resuming_from |
| `new_governance_updates` | `TRACE` | public | Create governance updates (i.e. ratify proposals) at an epoch boundary. | proposals_count |  |
| `new_pools_updates` | `TRACE` | public | Create pools updates |  |  |
| `record` | `TRACE` | public | Record an in-flight epoch transition | from, to |  |
| `retire_pool` | `TRACE` | public | Retire a pool at an epoch boundary | id |  |
| `rollback` | `TRACE` | public | Rollback an in-flight epoch transition | from, to |  |
| `tick_pool` | `TRACE` | public | Update a pool's parameters at an epoch boundary; only changed parameters are recorded | id | vrf, pledge, cost, margin, reward_account, owners, relays, metadata |

<details><summary>span: `apply`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `should_end_epoch` | `boolean` |  |
| `should_snapshot` | `boolean` |  |
| `should_begin_epoch` | `boolean` |  |

</details>

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `integer` | ✓ |
| `into` | `integer` | ✓ |
| `skipped` | `boolean` |  |
| `resuming_from` | `string` |  |

</details>

<details><summary>span: `new_governance_updates`</summary>

| field | type | required |
| --- | --- | --- |
| `proposals_count` | `integer` | ✓ |

</details>

<details><summary>span: `record`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `integer` | ✓ |
| `to` | `integer` | ✓ |

</details>

<details><summary>span: `retire_pool`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |

</details>

<details><summary>span: `rollback`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `integer` | ✓ |
| `to` | `integer` | ✓ |

</details>

<details><summary>span: `tick_pool`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `vrf` | `string` |  |
| `pledge` | `string` |  |
| `cost` | `string` |  |
| `margin` | `string` |  |
| `reward_account` | `string` |  |
| `owners` | `string` |  |
| `relays` | `string` |  |
| `metadata` | `string` |  |

</details>

## target: `amaru::ledger::governance`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `enacting` | `TRACE` | public | Computing enactment of a ratified proposal | proposal_id, proposal_kind | pruned_relatives |
| `new_ratification_context` | `TRACE` | public | Create ratification context | ratifying_epoch | treasury, votes |
| `ratify_proposals` | `TRACE` | public | Ratify proposals at epoch boundary | epoch | roots_protocol_parameters, roots_hard_fork, roots_constitutional_committee, roots_constitution |
| `ratifying` | `TRACE` | public | Ratify a proposal while traversing the governance forest | proposal_id, proposal_kind | approved_by_constitutional_committee, committee_approval_threshold, approved_by_pools, pools_approval_threshold, approved_by_dreps, dreps_approval_threshold |

<details><summary>span: `enacting`</summary>

| field | type | required |
| --- | --- | --- |
| `proposal_id` | `string` | ✓ |
| `proposal_kind` | `string` | ✓ |
| `pruned_relatives` | `string` |  |

</details>

<details><summary>span: `new_ratification_context`</summary>

| field | type | required |
| --- | --- | --- |
| `ratifying_epoch` | `integer` | ✓ |
| `treasury` | `integer` |  |
| `votes` | `integer` |  |

</details>

<details><summary>span: `ratify_proposals`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `roots_protocol_parameters` | `string` |  |
| `roots_hard_fork` | `string` |  |
| `roots_constitutional_committee` | `string` |  |
| `roots_constitution` | `string` |  |

</details>

<details><summary>span: `ratifying`</summary>

| field | type | required |
| --- | --- | --- |
| `proposal_id` | `string` | ✓ |
| `proposal_kind` | `string` | ✓ |
| `approved_by_constitutional_committee` | `boolean` |  |
| `committee_approval_threshold` | `string` |  |
| `approved_by_pools` | `boolean` |  |
| `pools_approval_threshold` | `string` |  |
| `approved_by_dreps` | `boolean` |  |
| `dreps_approval_threshold` | `string` |  |

</details>

## target: `amaru::ledger::governance_activity`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `update` | `TRACE` | public | Update the number of consecutive dormant epochs | consecutive_dormant_epochs |  |

<details><summary>span: `update`</summary>

| field | type | required |
| --- | --- | --- |
| `consecutive_dormant_epochs` | `integer` | ✓ |

</details>

## target: `amaru::ledger::overlay`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `no_governance_updates` | `TRACE` | public | No governance updates found in the epoch transition overlay |  |  |
| `no_pools_updates` | `TRACE` | public | No pools updates found in the epoch transition overlay |  |  |

## target: `amaru::ledger::pots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `load` | `TRACE` | public | Load the current ledger pots | treasury, reserves, fees, donations |  |

<details><summary>span: `load`</summary>

| field | type | required |
| --- | --- | --- |
| `treasury` | `integer` | ✓ |
| `reserves` | `integer` | ✓ |
| `fees` | `integer` | ✓ |
| `donations` | `integer` | ✓ |

</details>

## target: `amaru::ledger::proposal`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `active` | `TRACE` | public | Observe a governance proposal that is currently active | id, proposal_kind, proposed_in, valid_until | detail |
| `drop` | `TRACE` | public | Drop an expired or ratified governance proposal | id, expired, ratified_or_evicted |  |
| `skip` | `TRACE` | public | Skip a governance proposal during ratification | id, reason | proposed_in, ratifying_epoch, withdrawal, treasury, invalid_members |

<details><summary>span: `active`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `proposal_kind` | `string` | ✓ |
| `proposed_in` | `integer` | ✓ |
| `valid_until` | `integer` | ✓ |
| `detail` | `string` |  |

</details>

<details><summary>span: `drop`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `expired` | `boolean` | ✓ |
| `ratified_or_evicted` | `boolean` | ✓ |

</details>

<details><summary>span: `skip`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `reason` | `string` | ✓ |
| `proposed_in` | `integer` |  |
| `ratifying_epoch` | `integer` |  |
| `withdrawal` | `integer` |  |
| `treasury` | `integer` |  |
| `invalid_members` | `string` |  |

</details>

## target: `amaru::ledger::proposal_roots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `summarize` | `TRACE` | public | Summary of the governance proposal roots after ratification |  | constitution, constitutional_committee, hard_fork, protocol_parameters |

<details><summary>span: `summarize`</summary>

| field | type | required |
| --- | --- | --- |
| `constitution` | `string` |  |
| `constitutional_committee` | `string` |  |
| `hard_fork` | `string` |  |
| `protocol_parameters` | `string` |  |

</details>

## target: `amaru::ledger::protocol`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `upgrade` | `TRACE` | public | Upgrade to a new protocol version | old_version, new_version |  |

<details><summary>span: `upgrade`</summary>

| field | type | required |
| --- | --- | --- |
| `old_version` | `integer` | ✓ |
| `new_version` | `integer` | ✓ |

</details>

## target: `amaru::ledger::protocol_parameters`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `load` | `TRACE` | public | Load the current protocol parameters |  | protocol_version, max_block_body_size, max_transaction_size, max_block_header_size, max_tx_ex_units, max_block_ex_units, max_value_size, max_collateral_inputs, min_fee_a, min_fee_b, stake_credential_deposit, stake_pool_deposit, monetary_expansion_rate, treasury_expansion_rate, min_pool_cost, lovelace_per_utxo_byte, prices, min_fee_ref_script_lovelace_per_byte, max_ref_script_size_per_tx, max_ref_script_size_per_block, ref_script_cost_stride, ref_script_cost_multiplier, stake_pool_max_retirement_epoch, optimal_stake_pools_count, pledge_influence, collateral_percentage, cost_models, pool_voting_thresholds, drep_voting_thresholds, min_committee_size, max_committee_term_length, gov_action_lifetime, gov_action_deposit, drep_deposit, drep_expiry |
| `ratify` | `TRACE` | public | Ratify a protocol parameters update; only changed parameters are recorded |  | protocol_version, max_block_body_size, max_transaction_size, max_block_header_size, max_tx_ex_units, max_block_ex_units, max_value_size, max_collateral_inputs, min_fee_a, min_fee_b, stake_credential_deposit, stake_pool_deposit, monetary_expansion_rate, treasury_expansion_rate, min_pool_cost, lovelace_per_utxo_byte, prices, min_fee_ref_script_lovelace_per_byte, max_ref_script_size_per_tx, max_ref_script_size_per_block, ref_script_cost_stride, ref_script_cost_multiplier, stake_pool_max_retirement_epoch, optimal_stake_pools_count, pledge_influence, collateral_percentage, cost_models, pool_voting_thresholds, drep_voting_thresholds, min_committee_size, max_committee_term_length, gov_action_lifetime, gov_action_deposit, drep_deposit, drep_expiry |

<details><summary>span: `load`</summary>

| field | type | required |
| --- | --- | --- |
| `protocol_version` | `string` |  |
| `max_block_body_size` | `string` |  |
| `max_transaction_size` | `string` |  |
| `max_block_header_size` | `string` |  |
| `max_tx_ex_units` | `string` |  |
| `max_block_ex_units` | `string` |  |
| `max_value_size` | `string` |  |
| `max_collateral_inputs` | `string` |  |
| `min_fee_a` | `string` |  |
| `min_fee_b` | `string` |  |
| `stake_credential_deposit` | `string` |  |
| `stake_pool_deposit` | `string` |  |
| `monetary_expansion_rate` | `string` |  |
| `treasury_expansion_rate` | `string` |  |
| `min_pool_cost` | `string` |  |
| `lovelace_per_utxo_byte` | `string` |  |
| `prices` | `string` |  |
| `min_fee_ref_script_lovelace_per_byte` | `string` |  |
| `max_ref_script_size_per_tx` | `string` |  |
| `max_ref_script_size_per_block` | `string` |  |
| `ref_script_cost_stride` | `string` |  |
| `ref_script_cost_multiplier` | `string` |  |
| `stake_pool_max_retirement_epoch` | `string` |  |
| `optimal_stake_pools_count` | `string` |  |
| `pledge_influence` | `string` |  |
| `collateral_percentage` | `string` |  |
| `cost_models` | `string` |  |
| `pool_voting_thresholds` | `string` |  |
| `drep_voting_thresholds` | `string` |  |
| `min_committee_size` | `string` |  |
| `max_committee_term_length` | `string` |  |
| `gov_action_lifetime` | `string` |  |
| `gov_action_deposit` | `string` |  |
| `drep_deposit` | `string` |  |
| `drep_expiry` | `string` |  |

</details>

<details><summary>span: `ratify`</summary>

| field | type | required |
| --- | --- | --- |
| `protocol_version` | `string` |  |
| `max_block_body_size` | `string` |  |
| `max_transaction_size` | `string` |  |
| `max_block_header_size` | `string` |  |
| `max_tx_ex_units` | `string` |  |
| `max_block_ex_units` | `string` |  |
| `max_value_size` | `string` |  |
| `max_collateral_inputs` | `string` |  |
| `min_fee_a` | `string` |  |
| `min_fee_b` | `string` |  |
| `stake_credential_deposit` | `string` |  |
| `stake_pool_deposit` | `string` |  |
| `monetary_expansion_rate` | `string` |  |
| `treasury_expansion_rate` | `string` |  |
| `min_pool_cost` | `string` |  |
| `lovelace_per_utxo_byte` | `string` |  |
| `prices` | `string` |  |
| `min_fee_ref_script_lovelace_per_byte` | `string` |  |
| `max_ref_script_size_per_tx` | `string` |  |
| `max_ref_script_size_per_block` | `string` |  |
| `ref_script_cost_stride` | `string` |  |
| `ref_script_cost_multiplier` | `string` |  |
| `stake_pool_max_retirement_epoch` | `string` |  |
| `optimal_stake_pools_count` | `string` |  |
| `pledge_influence` | `string` |  |
| `collateral_percentage` | `string` |  |
| `cost_models` | `string` |  |
| `pool_voting_thresholds` | `string` |  |
| `drep_voting_thresholds` | `string` |  |
| `min_committee_size` | `string` |  |
| `max_committee_term_length` | `string` |  |
| `gov_action_lifetime` | `string` |  |
| `gov_action_deposit` | `string` |  |
| `drep_deposit` | `string` |  |
| `drep_expiry` | `string` |  |

</details>

## target: `amaru::ledger::ratification`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `skip` | `TRACE` | public | Skip the remaining proposals for this epoch | reason |  |
| `summarize` | `TRACE` | public | Summary of the outcome of a ratification round | is_dormant_epoch | pruned_proposals, refunds, withdrawals, new_constitution, constitutional_committee_update |

<details><summary>span: `skip`</summary>

| field | type | required |
| --- | --- | --- |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `summarize`</summary>

| field | type | required |
| --- | --- | --- |
| `is_dormant_epoch` | `boolean` | ✓ |
| `pruned_proposals` | `string` |  |
| `refunds` | `string` |  |
| `withdrawals` | `string` |  |
| `new_constitution` | `string` |  |
| `constitutional_committee_update` | `string` |  |

</details>

## target: `amaru::ledger::relays`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `collect` | `TRACE` | public | Fetch candidate relays from the immutable store |  | count |

<details><summary>span: `collect`</summary>

| field | type | required |
| --- | --- | --- |
| `count` | `string` |  |

</details>

## target: `amaru::ledger::rewards`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `compute` | `TRACE` | public | Compute rewards for epoch | for_epoch, using_stake_distribution_from_epoch |  |
| `summarize` | `TRACE` | public | Summary of the rewards calculation for an epoch | efficiency, incentives, treasury_tax, total_rewards, available_rewards, effective_rewards, pots_reserves, pots_treasury, pots_fees |  |

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `for_epoch` | `integer` | ✓ |
| `using_stake_distribution_from_epoch` | `integer` | ✓ |

</details>

<details><summary>span: `summarize`</summary>

| field | type | required |
| --- | --- | --- |
| `efficiency` | `string` | ✓ |
| `incentives` | `integer` | ✓ |
| `treasury_tax` | `integer` | ✓ |
| `total_rewards` | `integer` | ✓ |
| `available_rewards` | `integer` | ✓ |
| `effective_rewards` | `integer` | ✓ |
| `pots_reserves` | `integer` | ✓ |
| `pots_treasury` | `integer` | ✓ |
| `pots_fees` | `integer` | ✓ |

</details>

## target: `amaru::ledger::rules`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `block` | `TRACE` | public | Block-related rules and other preflight checks |  |  |
| `phase_one` | `TRACE` | public | All phase one validations |  | preflight_micros, certificates_micros, collateral_micros, collateral_return_micros, donation_micros, fees_micros, inputs_micros, metadata_micros, mint_micros, outputs_micros, proposals_micros, scripts_micros, signatures_micros, validity_interval_micros, votes_micros, withdrawals_micros |
| `phase_two` | `TRACE` | public | Initialize script context and cost models for phase-2 validations, common to all scripts |  | script_context_micros |

<details><summary>span: `phase_one`</summary>

| field | type | required |
| --- | --- | --- |
| `preflight_micros` | `integer` |  |
| `certificates_micros` | `integer` |  |
| `collateral_micros` | `integer` |  |
| `collateral_return_micros` | `integer` |  |
| `donation_micros` | `integer` |  |
| `fees_micros` | `integer` |  |
| `inputs_micros` | `integer` |  |
| `metadata_micros` | `integer` |  |
| `mint_micros` | `integer` |  |
| `outputs_micros` | `integer` |  |
| `proposals_micros` | `integer` |  |
| `scripts_micros` | `integer` |  |
| `signatures_micros` | `integer` |  |
| `validity_interval_micros` | `integer` |  |
| `votes_micros` | `integer` |  |
| `withdrawals_micros` | `integer` |  |

</details>

<details><summary>span: `phase_two`</summary>

| field | type | required |
| --- | --- | --- |
| `script_context_micros` | `integer` |  |

</details>

## target: `amaru::ledger::stake_distribution`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `compute` | `TRACE` | public | Compute stake distribution for epoch | epoch |  |
| `initial_begin` | `TRACE` | public | Start computing one of the initial stake distributions loaded on startup | epoch |  |
| `initial_progress` | `TRACE` | public | Report progress for one of the initial stake distributions loaded on startup | epoch, progress |  |
| `initial_ready` | `TRACE` | public | Finished computing all initial stake distributions loaded on startup | epochs |  |
| `rotate` | `TRACE` | public | Rotate stake distributions at an epoch boundary | available_stake_distributions |  |
| `snapshot` | `TRACE` | public | Snapshot of the stake distribution taken at an epoch boundary | accounts, dreps, pools, active_stake, pools_voting_stake, dreps_voting_stake |  |

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `initial_begin`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `initial_progress`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `progress` | `number` | ✓ |

</details>

<details><summary>span: `initial_ready`</summary>

| field | type | required |
| --- | --- | --- |
| `epochs` | `string` | ✓ |

</details>

<details><summary>span: `rotate`</summary>

| field | type | required |
| --- | --- | --- |
| `available_stake_distributions` | `string` | ✓ |

</details>

<details><summary>span: `snapshot`</summary>

| field | type | required |
| --- | --- | --- |
| `accounts` | `integer` | ✓ |
| `dreps` | `integer` | ✓ |
| `pools` | `integer` | ✓ |
| `active_stake` | `integer` | ✓ |
| `pools_voting_stake` | `integer` | ✓ |
| `dreps_voting_stake` | `integer` | ✓ |

</details>

## target: `amaru::ledger::state`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `push` | `TRACE` | public | Forward ledger state with new volatile state |  |  |
| `roll_backward` | `TRACE` | public | Roll backward to a specific point |  |  |
| `roll_forward` | `TRACE` | public | Roll forward with a new block |  |  |
| `switch_to_fork` | `TRACE` | public | Switching to an alternative chain fork | fork_point, fork_length, rollback_length | outcome, stable_modified |

<details><summary>span: `switch_to_fork`</summary>

| field | type | required |
| --- | --- | --- |
| `fork_point` | `array` | ✓ |
| `fork_length` | `integer` | ✓ |
| `rollback_length` | `integer` | ✓ |
| `outcome` | `string` |  |
| `stable_modified` | `boolean` |  |

</details>

## target: `amaru::ledger::tip`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `update` | `TRACE` | public | Updated view of the locally adopted chain tip and its derived ledger health. | slot, header_hash, block_height, tx_count, epoch, slot_in_epoch, density, current_kes_period, remaining_kes_periods |  |

<details><summary>span: `update`</summary>

| field | type | required |
| --- | --- | --- |
| `slot` | `integer` | ✓ |
| `header_hash` | `string` | ✓ |
| `block_height` | `integer` | ✓ |
| `tx_count` | `integer` | ✓ |
| `epoch` | `integer` | ✓ |
| `slot_in_epoch` | `integer` | ✓ |
| `density` | `number` | ✓ |
| `current_kes_period` | `integer` | ✓ |
| `remaining_kes_periods` | `integer` | ✓ |

</details>

## target: `amaru::ledger::transaction`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `validate` | `TRACE` | public | Validate a single transaction | id |  |

<details><summary>span: `validate`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |

</details>

## target: `amaru::ledger::transaction::script`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `execute` | `TRACE` | public | A single script execution, with the associated redeemer qualifiers | purpose, index | acquire_arena_micros, decode_script_micros, build_uplc_program_micros, evaluate_uplc_program_micros |

<details><summary>span: `execute`</summary>

| field | type | required |
| --- | --- | --- |
| `purpose` | `string` | ✓ |
| `index` | `integer` | ✓ |
| `acquire_arena_micros` | `integer` |  |
| `decode_script_micros` | `integer` |  |
| `build_uplc_program_micros` | `integer` |  |
| `evaluate_uplc_program_micros` | `integer` |  |

</details>

## target: `amaru::ledger::transaction_validation_context`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `create` | `TRACE` | public | Create validation context for a transaction | id |  |

<details><summary>span: `create`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |

</details>

## target: `amaru::ledger::validation_context::accounts`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve accounts from the volatile db or the stable one |  | from_volatile, from_db |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::committee`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve committee members from the volatile db or the stable one |  |  |

## target: `amaru::ledger::validation_context::dreps`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve dreps from the volatile db or the stable one |  | from_volatile, from_db |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::inputs`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve transaction inputs from the volatile db or the stable one |  | from_volatile, from_db |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::pools`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve pools from the volatile db or the stable one |  | from_volatile, from_db |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::proposals`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve proposals from the volatile db or the stable one |  | from_volatile, from_db |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::volatile`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `aggregate` | `TRACE` | public | Recompute the volatile aggregate |  |  |
| `warm_up` | `TRACE` | public | The volatile db is still warming up and hasn't reached a stable point yet | size |  |

<details><summary>span: `warm_up`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::mempool::state`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `update` | `TRACE` | public | Compact view of the mempool occupancy for terminal dashboards. | tx_count, size_bytes |  |

<details><summary>span: `update`</summary>

| field | type | required |
| --- | --- | --- |
| `tx_count` | `integer` | ✓ |
| `size_bytes` | `integer` | ✓ |

</details>

## target: `amaru::mempool::transaction`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `accepted` | `TRACE` | public | Transaction validated and inserted into the mempool. | id, seq_no, origin |  |
| `evicted` | `TRACE` | public | Transaction removed from the mempool. Reason ∈ {included_in_adopted_block, evicted_after_new_tip}. | id, tip, reason |  |
| `received` | `TRACE` | public | Transaction received by the mempool stage, before validation. | id, origin |  |
| `rejected` | `TRACE` | public | Transaction rejected at insertion. Reason ∈ {invalid, duplicate, mempool_full}. | id, reason | validation_error |

<details><summary>span: `accepted`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `seq_no` | `integer` | ✓ |
| `origin` | `string` | ✓ |

</details>

<details><summary>span: `evicted`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `tip` | `array` | ✓ |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `received`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `origin` | `string` | ✓ |

</details>

<details><summary>span: `rejected`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `reason` | `string` | ✓ |
| `validation_error` | `string` |  |

</details>

## target: `amaru::mithril::snapshot`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `download` | `TRACE` | public | Download and unpack immutable files from a Mithril snapshot | target_dir, from_chunk |  |
| `fetch` | `TRACE` | public | Fetch and verify a Mithril snapshot | hash, from_chunk |  |
| `ready` | `TRACE` | public | Mithril cardano-node database is ready | target_dir |  |
| `verify_database` | `TRACE` | public | Verify the local cardano-node database against a Mithril certificate | target_dir |  |
| `verify_digests` | `TRACE` | public | Download and verify the digests for a Mithril snapshot | target_dir |  |

<details><summary>span: `download`</summary>

| field | type | required |
| --- | --- | --- |
| `target_dir` | `string` | ✓ |
| `from_chunk` | `integer` | ✓ |

</details>

<details><summary>span: `fetch`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `from_chunk` | `integer` | ✓ |

</details>

<details><summary>span: `ready`</summary>

| field | type | required |
| --- | --- | --- |
| `target_dir` | `string` | ✓ |

</details>

<details><summary>span: `verify_database`</summary>

| field | type | required |
| --- | --- | --- |
| `target_dir` | `string` | ✓ |

</details>

<details><summary>span: `verify_digests`</summary>

| field | type | required |
| --- | --- | --- |
| `target_dir` | `string` | ✓ |

</details>

## target: `amaru::network::connection`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `accept_loop_stopped` | `TRACE` | public | The accept loop terminated because the listener or channel closed | local |  |
| `listener_restart` | `TRACE` | public | Aborted an existing listener task so the address can be rebound on restart | address |  |

<details><summary>span: `accept_loop_stopped`</summary>

| field | type | required |
| --- | --- | --- |
| `local` | `string` | ✓ |

</details>

<details><summary>span: `listener_restart`</summary>

| field | type | required |
| --- | --- | --- |
| `address` | `string` | ✓ |

</details>

## target: `amaru::node::build`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `ledger_opened` | `TRACE` | public | Opened the ledger state; reports the ledger tip at startup | tip |  |
| `stake_dist_notify_failed` | `TRACE` | public | Failed to notify the peer tracker of a stake distribution update |  |  |

<details><summary>span: `ledger_opened`</summary>

| field | type | required |
| --- | --- | --- |
| `tip` | `array` | ✓ |

</details>

## target: `amaru::node::metrics`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `process_not_found` | `TRACE` | public | The metrics collector could not find Amaru's own process | pid |  |

<details><summary>span: `process_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `pid` | `integer` | ✓ |

</details>

## target: `amaru::node::submit_api`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `mempool_unreachable` | `TRACE` | public | A submitted transaction could not reach the mempool. Reason ∈ {send_failed, response_dropped, deserialize_failed}. | reason |  |
| `started` | `TRACE` | public | The transaction submission HTTP server is listening | local_addr |  |
| `stopped` | `TRACE` | public | The transaction submission HTTP server stopped with an error | error |  |

<details><summary>span: `mempool_unreachable`</summary>

| field | type | required |
| --- | --- | --- |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `started`</summary>

| field | type | required |
| --- | --- | --- |
| `local_addr` | `string` | ✓ |

</details>

<details><summary>span: `stopped`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |

</details>

## target: `amaru::protocols::blockfetch::initiator`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `protocol_violation` | `TRACE` | public | The peer broke the block-fetch protocol and the connection is terminated. Reason ∈ {too_many_blocks, no_pending_request, invalid_cbor}. | reason | max_blocks, bytes |

<details><summary>span: `protocol_violation`</summary>

| field | type | required |
| --- | --- | --- |
| `reason` | `string` | ✓ |
| `max_blocks` | `integer` |  |
| `bytes` | `integer` |  |

</details>

## target: `amaru::protocols::chainsync::initiator`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `rollback_point_not_found` | `TRACE` | public | A rollback target announced by the peer is not in the chain store | header_hash |  |

<details><summary>span: `rollback_point_not_found`</summary>

| field | type | required |
| --- | --- | --- |
| `header_hash` | `string` | ✓ |

</details>

## target: `amaru::protocols::chainsync::responder`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `stopped` | `TRACE` | public | The peer ended the chainsync session |  |  |

## target: `amaru::protocols::connection`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `accept_failed` | `TRACE` | public | An inbound connection could not be accepted. Reason ∈ {aborted, error}. | reason | error |
| `child_died` | `TRACE` | public | A mini-protocol stage running on a connection died | peer, conn_id, child |  |
| `handshake_query_reply` | `TRACE` | public | The peer answered a version query instead of negotiating | version_table |  |
| `handshake_refused` | `TRACE` | public | The peer refused our proposed protocol versions | reason |  |

<details><summary>span: `accept_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `reason` | `string` | ✓ |
| `error` | `string` |  |

</details>

<details><summary>span: `child_died`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `child` | `string` | ✓ |

</details>

<details><summary>span: `handshake_query_reply`</summary>

| field | type | required |
| --- | --- | --- |
| `version_table` | `string` | ✓ |

</details>

<details><summary>span: `handshake_refused`</summary>

| field | type | required |
| --- | --- | --- |
| `reason` | `string` | ✓ |

</details>

## target: `amaru::protocols::keepalive::peer`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `round_trip` | `TRACE` | public | Measured round-trip time for a keepalive exchange on an established peer connection. | peer, conn_id, round_trip_micros |  |

<details><summary>span: `round_trip`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `round_trip_micros` | `integer` | ✓ |

</details>

## target: `amaru::protocols::manager::blocks`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `fetch_no_peers` | `TRACE` | public | No connection was available to serve a block-fetch request | id |  |

<details><summary>span: `fetch_no_peers`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `integer` | ✓ |

</details>

## target: `amaru::protocols::manager::listen`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `failed` | `TRACE` | public | The node could not listen on the configured address | listen_addr, error |  |
| `started` | `TRACE` | public | The node is accepting inbound connections on an address | listen_addr |  |

<details><summary>span: `failed`</summary>

| field | type | required |
| --- | --- | --- |
| `listen_addr` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `started`</summary>

| field | type | required |
| --- | --- | --- |
| `listen_addr` | `string` | ✓ |

</details>

## target: `amaru::protocols::manager::message`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `process` | `TRACE` | public | Handle manager stage messages | message_type |  |

<details><summary>span: `process`</summary>

| field | type | required |
| --- | --- | --- |
| `message_type` | `string` | ✓ |

</details>

## target: `amaru::protocols::manager::peer`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `accepted` | `TRACE` | public | An inbound connection was accepted from a peer | peer, conn_id |  |
| `add` | `TRACE` | public | A new peer was added to the manager | peer |  |
| `close_failed` | `TRACE` | public | Closing the socket of a dead connection failed | peer, error |  |
| `connect` | `TRACE` | public | Initiating an outbound connection to a peer | peer |  |
| `connect_discarded` | `TRACE` | public | A connection request for a peer was discarded. Reason ∈ {already_connected_or_scheduled, already_connected, not_added}. | peer, reason |  |
| `connect_exhausted` | `TRACE` | public | A peer is dropped after exhausting its connection attempts | peer |  |
| `connect_failed` | `TRACE` | public | An outbound connection attempt failed | peer, error |  |
| `connected` | `TRACE` | public | An outbound connection to a peer was established | peer, conn_id |  |
| `connection_died` | `TRACE` | public | A peer connection has died | peer, conn_id, role |  |
| `connection_died_handled` | `TRACE` | public | A dead connection was reconciled with the peer's remaining state. Outcome ∈ {peer_removed, kept_for_outbound, retries_suppressed, reconnect_scheduled}. | peer, outcome |  |
| `disconnect_ignored` | `TRACE` | public | A disconnect request could not be carried out. Reason ∈ {not_connected, connection_not_found, peer_already_removed, before_handshake}. | peer, reason | conn_id |
| `disconnecting` | `TRACE` | public | A connection is being closed on request. Direction ∈ {inbound, outbound}. | peer, conn_id, direction |  |
| `duplicate_terminated` | `TRACE` | public | A duplicate connection is terminated after its handshake completed | peer, conn_id |  |
| `handshake_completed` | `TRACE` | public | The handshake completed on a connection | peer, conn_id, full_duplex_capable, full_duplex, advertisable |  |
| `remove` | `TRACE` | public | A peer was removed from the manager | peer |  |

<details><summary>span: `accepted`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |

</details>

<details><summary>span: `add`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `close_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `connect`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `connect_discarded`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `connect_exhausted`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `connect_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `connected`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |

</details>

<details><summary>span: `connection_died`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `role` | `string` | ✓ |

</details>

<details><summary>span: `connection_died_handled`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `outcome` | `string` | ✓ |

</details>

<details><summary>span: `disconnect_ignored`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `reason` | `string` | ✓ |
| `conn_id` | `integer` |  |

</details>

<details><summary>span: `disconnecting`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `direction` | `string` | ✓ |

</details>

<details><summary>span: `duplicate_terminated`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |

</details>

<details><summary>span: `handshake_completed`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `full_duplex_capable` | `boolean` | ✓ |
| `full_duplex` | `boolean` | ✓ |
| `advertisable` | `boolean` | ✓ |

</details>

<details><summary>span: `remove`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

## target: `amaru::protocols::mux`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `empty_segment` | `TRACE` | public | A segment header announcing an empty payload was received | role, peer |  |
| `failed` | `TRACE` | public | The muxer failed while moving data between a protocol and the network. Operation ∈ {send, recv_header, decode_header, recv_data, muxing}. | role, peer, operation, error |  |

<details><summary>span: `empty_segment`</summary>

| field | type | required |
| --- | --- | --- |
| `role` | `string` | ✓ |
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `failed`</summary>

| field | type | required |
| --- | --- | --- |
| `role` | `string` | ✓ |
| `peer` | `string` | ✓ |
| `operation` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

## target: `amaru::protocols::mux::protocol`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `buffer_exceeded` | `TRACE` | public | A protocol message does not fit in the buffer allotted to it | buffered, max_buffer |  |
| `buffer_overflow` | `TRACE` | public | Reducing a protocol buffer was not enough and the connection was killed | buffer, limit |  |

<details><summary>span: `buffer_exceeded`</summary>

| field | type | required |
| --- | --- | --- |
| `buffered` | `integer` | ✓ |
| `max_buffer` | `integer` | ✓ |

</details>

<details><summary>span: `buffer_overflow`</summary>

| field | type | required |
| --- | --- | --- |
| `buffer` | `integer` | ✓ |
| `limit` | `integer` | ✓ |

</details>

## target: `amaru::protocols::peer_selection`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `connect_initial` | `TRACE` | public | Connect to the initial set of peers at startup | static_peers, snapshot_peers |  |

<details><summary>span: `connect_initial`</summary>

| field | type | required |
| --- | --- | --- |
| `static_peers` | `integer` | ✓ |
| `snapshot_peers` | `integer` | ✓ |

</details>

## target: `amaru::protocols::peer_selection::ledger`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `candidates_failed` | `TRACE` | public | Failed to read registered relay addresses from the ledger | error |  |

<details><summary>span: `candidates_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |

</details>

## target: `amaru::protocols::peer_selection::peer`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add_skipped` | `TRACE` | public | A peer was not added to the outbound set. Reason ∈ {already_added, too_many_inbound}. | peer, reason |  |
| `added` | `TRACE` | public | A peer was added to the outbound set | peer, was_banned |  |
| `connected` | `TRACE` | public | A connection has been established and the handshake completed successfully. | peer, conn_id, direction, full_duplex_capable, full_duplex |  |
| `disconnected` | `TRACE` | public | A connection has been terminated (graceful disconnect, error, handshake refusal, or network error). | peer, conn_id, direction | reason |
| `reconnected` | `TRACE` | public | A peer reconnected while a previous connection was still registered; the older connection is dropped. Direction ∈ {inbound, outbound}. | peer, direction, conn_id |  |
| `removed` | `TRACE` | public | A peer was removed after behaving adversarially | peer, direction, peer_state, is_static |  |

<details><summary>span: `add_skipped`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `added`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `was_banned` | `boolean` | ✓ |

</details>

<details><summary>span: `connected`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `direction` | `string` | ✓ |
| `full_duplex_capable` | `boolean` | ✓ |
| `full_duplex` | `boolean` | ✓ |

</details>

<details><summary>span: `disconnected`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |
| `direction` | `string` | ✓ |
| `reason` | `string` |  |

</details>

<details><summary>span: `reconnected`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `direction` | `string` | ✓ |
| `conn_id` | `integer` | ✓ |

</details>

<details><summary>span: `removed`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `direction` | `string` | ✓ |
| `peer_state` | `string` | ✓ |
| `is_static` | `boolean` | ✓ |

</details>

## target: `amaru::protocols::peer_selection::sharing`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `received` | `TRACE` | public | Peer-sharing address list received from peer. | peer, peers, added, total |  |
| `sent` | `TRACE` | public | Peer-sharing request served for peer. | peer, peers, requested, count |  |

<details><summary>span: `received`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `peers` | `string` | ✓ |
| `added` | `integer` | ✓ |
| `total` | `integer` | ✓ |

</details>

<details><summary>span: `sent`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `peers` | `string` | ✓ |
| `requested` | `integer` | ✓ |
| `count` | `integer` | ✓ |

</details>

## target: `amaru::protocols::peer_sharing::initiator`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `protocol_violation` | `TRACE` | public | The peer broke the peer-sharing protocol and the connection is terminated. Reason ∈ {no_request_in_flight, too_many_addresses}. | reason | requested, received |

<details><summary>span: `protocol_violation`</summary>

| field | type | required |
| --- | --- | --- |
| `reason` | `string` | ✓ |
| `requested` | `integer` |  |
| `received` | `integer` |  |

</details>

## target: `amaru::protocols::tx_submission`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `terminating` | `TRACE` | public | The tx-submission protocol is being torn down; the cause names the rule broken | cause |  |

<details><summary>span: `terminating`</summary>

| field | type | required |
| --- | --- | --- |
| `cause` | `string` | ✓ |

</details>

## target: `amaru::protocols::tx_submission::initiator`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `over_acknowledged` | `TRACE` | public | The peer acknowledged more transaction ids than are outstanding | ack, window |  |
| `unavailable_txs` | `TRACE` | public | The peer asked for transactions that are not in our outstanding window | unavailable |  |

<details><summary>span: `over_acknowledged`</summary>

| field | type | required |
| --- | --- | --- |
| `ack` | `integer` | ✓ |
| `window` | `integer` | ✓ |

</details>

<details><summary>span: `unavailable_txs`</summary>

| field | type | required |
| --- | --- | --- |
| `unavailable` | `string` | ✓ |

</details>

## target: `amaru::protocols::tx_submission::responder`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `mempool_timeout` | `TRACE` | public | The mempool did not answer an insertion batch before the timeout |  |  |
| `over_replied` | `TRACE` | public | The peer replied with more transaction ids than were requested | requested, received, max_window |  |
| `received_tx` | `TRACE` | public | A transaction received from a peer was handed to the mempool. Outcome ∈ {inserted, invalid, mempool_full, duplicate}. | id, outcome | error |
| `unsolicited_txs` | `TRACE` | public | The peer sent transaction bodies that were never requested | not_requested |  |

<details><summary>span: `over_replied`</summary>

| field | type | required |
| --- | --- | --- |
| `requested` | `integer` | ✓ |
| `received` | `integer` | ✓ |
| `max_window` | `integer` | ✓ |

</details>

<details><summary>span: `received_tx`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |
| `outcome` | `string` | ✓ |
| `error` | `string` |  |

</details>

<details><summary>span: `unsolicited_txs`</summary>

| field | type | required |
| --- | --- | --- |
| `not_requested` | `string` | ✓ |

</details>

## target: `amaru::setup::build`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `version` | `TRACE` | public | Running binary build/version identity (package version, git commit, target). | version, git_commit, git_dirty, os, arch |  |

<details><summary>span: `version`</summary>

| field | type | required |
| --- | --- | --- |
| `version` | `string` | ✓ |
| `git_commit` | `string` | ✓ |
| `git_dirty` | `boolean` | ✓ |
| `os` | `string` | ✓ |
| `arch` | `string` | ✓ |

</details>

## target: `amaru::setup::file_descriptors`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `too_low` | `TRACE` | public | The soft limit on open files is below what Amaru needs | current_soft_fd_limit, current_hard_fd_limit, expected_min, hint |  |
| `unknown` | `TRACE` | public | The open-file limit could not be queried | expected_min |  |

<details><summary>span: `too_low`</summary>

| field | type | required |
| --- | --- | --- |
| `current_soft_fd_limit` | `integer` | ✓ |
| `current_hard_fd_limit` | `integer` | ✓ |
| `expected_min` | `integer` | ✓ |
| `hint` | `string` | ✓ |

</details>

<details><summary>span: `unknown`</summary>

| field | type | required |
| --- | --- | --- |
| `expected_min` | `integer` | ✓ |

</details>

## target: `amaru::setup::lifecycle`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `consensus_died` | `TRACE` | public | The consensus pipeline stopped while the node was still running |  |  |
| `termination_signal` | `TRACE` | public | A termination signal was received; the node is shutting down |  |  |

## target: `amaru::setup::observability`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `init` | `TRACE` | public | Observability stack initialization | with_open_telemetry, with_json_traces, with_colors |  |

<details><summary>span: `init`</summary>

| field | type | required |
| --- | --- | --- |
| `with_open_telemetry` | `boolean` | ✓ |
| `with_json_traces` | `boolean` | ✓ |
| `with_colors` | `boolean` | ✓ |

</details>

## target: `amaru::setup::peer_snapshot`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `empty` | `TRACE` | public | A peer snapshot was loaded but holds no relay addresses | path, point, pools |  |
| `loaded` | `TRACE` | public | A peer snapshot was loaded at startup | path, point, pools, relays, node_to_client_version, configs_commit |  |
| `missing` | `TRACE` | public | No embedded peer snapshot exists for the selected network | network |  |

<details><summary>span: `empty`</summary>

| field | type | required |
| --- | --- | --- |
| `path` | `string` | ✓ |
| `point` | `string` | ✓ |
| `pools` | `integer` | ✓ |

</details>

<details><summary>span: `loaded`</summary>

| field | type | required |
| --- | --- | --- |
| `path` | `string` | ✓ |
| `point` | `string` | ✓ |
| `pools` | `integer` | ✓ |
| `relays` | `integer` | ✓ |
| `node_to_client_version` | `integer` | ✓ |
| `configs_commit` | `string` | ✓ |

</details>

<details><summary>span: `missing`</summary>

| field | type | required |
| --- | --- | --- |
| `network` | `string` | ✓ |

</details>

## target: `amaru::setup::pid`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `write_failed` | `TRACE` | public | The PID file could not be created or written | error |  |

<details><summary>span: `write_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `error` | `string` | ✓ |

</details>

## target: `amaru::setup::trace`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `filter` | `TRACE` | public | Resolution of a trace filter from the environment | var, value, provided_by_user | provided_invalid, error |

<details><summary>span: `filter`</summary>

| field | type | required |
| --- | --- | --- |
| `var` | `string` | ✓ |
| `value` | `string` | ✓ |
| `provided_by_user` | `boolean` | ✓ |
| `provided_invalid` | `boolean` |  |
| `error` | `string` |  |

</details>

## target: `amaru::setup::trace_buffer`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `dump_failed` | `TRACE` | public | The stage trace buffer could not be written to disk | path, error |  |
| `dumped` | `TRACE` | public | The stage trace buffer was written to disk | path |  |

<details><summary>span: `dump_failed`</summary>

| field | type | required |
| --- | --- | --- |
| `path` | `string` | ✓ |
| `error` | `string` | ✓ |

</details>

<details><summary>span: `dumped`</summary>

| field | type | required |
| --- | --- | --- |
| `path` | `string` | ✓ |

</details>

## target: `amaru::stores::batch`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `commit` | `TRACE` | public | Commit a write batch |  |  |
| `dropped_without_close` | `TRACE` | public | A transaction was dropped without commit or rollback. Outcome ∈ {left_open, auto_rolled_back}. | outcome |  |
| `rollback` | `TRACE` | public | Rollback a write batch |  |  |

<details><summary>span: `dropped_without_close`</summary>

| field | type | required |
| --- | --- | --- |
| `outcome` | `string` | ✓ |

</details>

## target: `amaru::stores::consensus::block`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `store` | `TRACE` | public | Store a raw block | hash |  |

<details><summary>span: `store`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

## target: `amaru::stores::consensus::chain`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `roll_forward` | `TRACE` | public | Roll forward the chain to a point | hash, slot |  |
| `switch_to_fork` | `TRACE` | public | Switch the chain to a new fork | hash, slot |  |

<details><summary>span: `roll_forward`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |

</details>

<details><summary>span: `switch_to_fork`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |

</details>

## target: `amaru::stores::consensus::header`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `store` | `TRACE` | public | Store a block header | hash |  |

<details><summary>span: `store`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

## target: `amaru::stores::ledger`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `iter_scan` | `TRACE` | public | Full scan for a given collection | db_collection_name | rows_scanned, rows_written, rows_deleted |

<details><summary>span: `iter_scan`</summary>

| field | type | required |
| --- | --- | --- |
| `db_collection_name` | `string` | ✓ |
| `rows_scanned` | `integer` |  |
| `rows_written` | `integer` |  |
| `rows_deleted` | `integer` |  |

</details>

## target: `amaru::stores::ledger::accounts`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add` | `TRACE` | public | Batch-upsert account entries |  |  |
| `get` | `TRACE` | public | Point-read an account entry |  |  |
| `remove` | `TRACE` | public | Batch-delete account entries |  |  |
| `reset_many` | `TRACE` | public | Reset rewards counters for many accounts |  | credential, reason |
| `set` | `TRACE` | public | Update rewards balance for a single account |  | credential_type, account, reason |

<details><summary>span: `reset_many`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` |  |
| `reason` | `string` |  |

</details>

<details><summary>span: `set`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` |  |
| `account` | `string` |  |
| `reason` | `string` |  |

</details>

## target: `amaru::stores::ledger::cc_members`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `get` | `TRACE` | public | Read a constitutional committee member |  |  |
| `upsert` | `TRACE` | public | Upsert a constitutional committee member |  |  |

## target: `amaru::stores::ledger::dreps`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add` | `TRACE` | public | Batch-upsert DRep registrations |  | credential, reason |
| `get` | `TRACE` | public | Point-read a DRep entry |  |  |
| `remove` | `TRACE` | public | Record DRep de-registration |  | drep, reason |
| `set_valid_until` | `TRACE` | public | Refresh DRep expiry after a vote |  | credential, reason |

<details><summary>span: `add`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` |  |
| `reason` | `string` |  |

</details>

<details><summary>span: `remove`</summary>

| field | type | required |
| --- | --- | --- |
| `drep` | `string` |  |
| `reason` | `string` |  |

</details>

<details><summary>span: `set_valid_until`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` |  |
| `reason` | `string` |  |

</details>

## target: `amaru::stores::ledger::epoch`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `create_snapshot` | `TRACE` | public | Create ledger snapshot for epoch | epoch |  |
| `prune_old_snapshots` | `TRACE` | public | Prune old snapshots | functional_minimum, desired_minimum |  |
| `try_transition` | `TRACE` | public | Epoch transition tracking | from, to |  |

<details><summary>span: `create_snapshot`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `prune_old_snapshots`</summary>

| field | type | required |
| --- | --- | --- |
| `functional_minimum` | `integer` | ✓ |
| `desired_minimum` | `integer` | ✓ |

</details>

<details><summary>span: `try_transition`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `string` | ✓ |
| `to` | `string` | ✓ |

</details>

## target: `amaru::stores::ledger::overlay`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `apply_governance_updates` | `TRACE` | public | Enact all governance updates and flush their outcome to disk |  |  |
| `pay_or_refund_accounts` | `TRACE` | public | Pay withdrawals to accounts, or refund deposits |  | total_paid_or_refunded, treasury_leftovers |
| `pay_rewards` | `TRACE` | public | Pay rewards to all accounts before the epoch end |  | accounts_paid, rewards_paid, treasury_delta, reserves_delta |
| `record_pruned_proposals` | `TRACE` | public | Pruned proposals at an epoch boundary, recorded to facilitate future stake distribution calculations. |  |  |
| `reset_blocks_count` | `TRACE` | public | Reset blocks count to zero |  |  |
| `reset_fees` | `TRACE` | public | Reset fees to zero |  |  |
| `update_constitutional_committee` | `TRACE` | public | Add or remove CC members; or switch to a no-confidence state | no_confidence |  |
| `update_or_retire_pools` | `TRACE` | public | Updating pools metadata or retiring pools at an epoch boundary. | pools_updated, pools_retired |  |

<details><summary>span: `pay_or_refund_accounts`</summary>

| field | type | required |
| --- | --- | --- |
| `total_paid_or_refunded` | `integer` |  |
| `treasury_leftovers` | `integer` |  |

</details>

<details><summary>span: `pay_rewards`</summary>

| field | type | required |
| --- | --- | --- |
| `accounts_paid` | `integer` |  |
| `rewards_paid` | `integer` |  |
| `treasury_delta` | `integer` |  |
| `reserves_delta` | `integer` |  |

</details>

<details><summary>span: `update_constitutional_committee`</summary>

| field | type | required |
| --- | --- | --- |
| `no_confidence` | `boolean` | ✓ |

</details>

<details><summary>span: `update_or_retire_pools`</summary>

| field | type | required |
| --- | --- | --- |
| `pools_updated` | `integer` | ✓ |
| `pools_retired` | `integer` | ✓ |

</details>

## target: `amaru::stores::ledger::pools`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add` | `TRACE` | public | Batch-upsert pool entries |  |  |
| `get` | `TRACE` | public | Point-read a pool entry |  |  |
| `remove` | `TRACE` | public | Schedule pool retirement |  | pool, reason |

<details><summary>span: `remove`</summary>

| field | type | required |
| --- | --- | --- |
| `pool` | `string` |  |
| `reason` | `string` |  |

</details>

## target: `amaru::stores::ledger::pots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `get` | `TRACE` | public | Read treasury/reserve/fees pots |  |  |
| `put` | `TRACE` | public | Write treasury/reserve/fees pots |  |  |

## target: `amaru::stores::ledger::proposals`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add` | `TRACE` | public | Insert governance proposals |  |  |
| `get` | `TRACE` | public | Read governance proposals |  |  |
| `remove` | `TRACE` | public | Remove enacted or expired proposals |  |  |

## target: `amaru::stores::ledger::recently_pruned_proposals`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `replace_all` | `TRACE` | public | Inserting recently pruned proposals |  |  |

## target: `amaru::stores::ledger::recently_unregistered_accounts`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `insert` | `TRACE` | public | Insert a recently unregistered account |  |  |
| `prune` | `TRACE` | public | Prune recently unregistered accounts | epoch |  |
| `remove` | `TRACE` | public | Remove a recently unregistered account |  |  |

<details><summary>span: `prune`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

## target: `amaru::stores::ledger::slots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `get` | `TRACE` | public | Point-read a slot/block-issuer entry |  |  |
| `put` | `TRACE` | public | Write a slot/block-issuer entry |  |  |

## target: `amaru::stores::ledger::snapshots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `unexpected_file` | `TRACE` | public | Skipped an unexpected file found in the snapshots directory | filename |  |
| `validate` | `TRACE` | public | Validate sufficient snapshots exist |  | snapshot_count, continuous_ranges |

<details><summary>span: `unexpected_file`</summary>

| field | type | required |
| --- | --- | --- |
| `filename` | `string` | ✓ |

</details>

<details><summary>span: `validate`</summary>

| field | type | required |
| --- | --- | --- |
| `snapshot_count` | `integer` |  |
| `continuous_ranges` | `integer` |  |

</details>

## target: `amaru::stores::ledger::utxo`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add` | `TRACE` | public | Batch-insert UTxO entries |  |  |
| `get` | `TRACE` | public | Point-read a UTxO entry |  |  |
| `remove` | `TRACE` | public | Batch-delete UTxO entries |  |  |

## target: `amaru::stores::ledger::votes`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add` | `TRACE` | public | Record governance votes |  |  |
| `remove` | `TRACE` | public | Remove now-obsolete governance votes |  |  |

## Updating This Documentation

This file is auto-generated from the trace schema definitions in the code. To update it, run:

```bash
./scripts/generate-traces-doc
```

The schemas are defined using the `define_schemas!` macro in the codebase. Any changes to trace definitions will automatically be reflected in this documentation when the script is run.
