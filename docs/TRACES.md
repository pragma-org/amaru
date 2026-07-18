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
| `tip` | `string` | ✓ |

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

<details><summary>span: `fetch`</summary>

| field | type | required |
| --- | --- | --- |
| `requested_point` | `string` | ✓ |
| `intersection` | `string` | ✓ |
| `headers_per_point` | `integer` | ✓ |

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

## target: `amaru::bootstrap::local_snapshots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `detect` | `TRACE` | public | Detect locally-created snapshots from create-snapshots | count |  |

<details><summary>span: `detect`</summary>

| field | type | required |
| --- | --- | --- |
| `count` | `integer` | ✓ |

</details>

## target: `amaru::bootstrap::nonces`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `import` | `TRACE` | public | Import initial nonces into the chain store | point |  |

<details><summary>span: `import`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `string` | ✓ |

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

## target: `amaru::bootstrap::snapshot`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `download` | `TRACE` | public | Download a snapshot archive | epoch, point |  |
| `extract` | `TRACE` | public | Extract a snapshot archive | snapshot |  |
| `import_dir` | `TRACE` | public | Import a snapshot directory | path |  |
| `import_file` | `TRACE` | public | Import a single snapshot | path |  |
| `import_tvar` | `TRACE` | public | Import from the tvar data | point, new_epoch_state_offset |  |
| `invalid` | `TRACE` | public | Existing snapshot files are invalid and will be removed | snapshot |  |
| `skip_download` | `TRACE` | public | Snapshot already downloaded; skipping download | snapshot |  |

<details><summary>span: `download`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `point` | `string` | ✓ |

</details>

<details><summary>span: `extract`</summary>

| field | type | required |
| --- | --- | --- |
| `snapshot` | `string` | ✓ |

</details>

<details><summary>span: `import_dir`</summary>

| field | type | required |
| --- | --- | --- |
| `path` | `string` | ✓ |

</details>

<details><summary>span: `import_file`</summary>

| field | type | required |
| --- | --- | --- |
| `path` | `string` | ✓ |

</details>

<details><summary>span: `import_tvar`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `string` | ✓ |
| `new_epoch_state_offset` | `integer` | ✓ |

</details>

<details><summary>span: `invalid`</summary>

| field | type | required |
| --- | --- | --- |
| `snapshot` | `string` | ✓ |

</details>

<details><summary>span: `skip_download`</summary>

| field | type | required |
| --- | --- | --- |
| `snapshot` | `string` | ✓ |

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

## target: `amaru::cli::cardano_node_config`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `download` | `TRACE` | public | Download the official cardano-node configuration bundle | config_dir, network |  |
| `use` | `TRACE` | public | Use an existing cardano-node configuration | config_dir, network |  |

<details><summary>span: `download`</summary>

| field | type | required |
| --- | --- | --- |
| `config_dir` | `string` | ✓ |
| `network` | `string` | ✓ |

</details>

<details><summary>span: `use`</summary>

| field | type | required |
| --- | --- | --- |
| `config_dir` | `string` | ✓ |
| `network` | `string` | ✓ |

</details>

## target: `amaru::cli::chain_db`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `exist` | `TRACE` | public | Chain database already exists | dir, hint |  |
| `forcefully_remove` | `TRACE` | public | Forcefully remove an existing chain database | dir |  |

<details><summary>span: `exist`</summary>

| field | type | required |
| --- | --- | --- |
| `dir` | `string` | ✓ |
| `hint` | `string` | ✓ |

</details>

<details><summary>span: `forcefully_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `dir` | `string` | ✓ |

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
| `log` | `TRACE` | public | Output line from an external db-analyser command | step, line |  |
| `progress` | `TRACE` | public | Progress reported by an external db-analyser command | step, detail |  |
| `reuse_ledger_snapshot` | `TRACE` | public | Reuse an existing db-analyser ledger snapshot | epoch, slot |  |
| `run` | `TRACE` | public | Run db-analyser to produce a ledger snapshot | epoch, slot | analyse_from |

<details><summary>span: `log`</summary>

| field | type | required |
| --- | --- | --- |
| `step` | `string` | ✓ |
| `line` | `string` | ✓ |

</details>

<details><summary>span: `progress`</summary>

| field | type | required |
| --- | --- | --- |
| `step` | `string` | ✓ |
| `detail` | `string` | ✓ |

</details>

<details><summary>span: `reuse_ledger_snapshot`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `slot` | `string` | ✓ |

</details>

<details><summary>span: `run`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `slot` | `string` | ✓ |
| `analyse_from` | `string` |  |

</details>

## target: `amaru::cli::epoch_metadata`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `write` | `TRACE` | public | Write the epoch metadata file for a snapshot | epoch, path |  |

<details><summary>span: `write`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `path` | `string` | ✓ |

</details>

## target: `amaru::cli::last_block`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `resolve` | `TRACE` | public | Resolve the last produced block for an epoch | epoch, point |  |

<details><summary>span: `resolve`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `point` | `string` | ✓ |

</details>

## target: `amaru::cli::ledger_db`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `exist` | `TRACE` | public | Ledger database already exists | dir, hint |  |
| `forcefully_remove` | `TRACE` | public | Forcefully remove an existing ledger database | dir |  |

<details><summary>span: `exist`</summary>

| field | type | required |
| --- | --- | --- |
| `dir` | `string` | ✓ |
| `hint` | `string` | ✓ |

</details>

<details><summary>span: `forcefully_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `dir` | `string` | ✓ |

</details>

## target: `amaru::cli::mithril`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `download` | `TRACE` | public | Synchronize the cardano-node database from Mithril | from_chunk, target_dir |  |
| `skip_download` | `TRACE` | public | Local cardano-node database is recent enough; skipping Mithril download | from_chunk, required_chunk, target_dir, reason |  |

<details><summary>span: `download`</summary>

| field | type | required |
| --- | --- | --- |
| `from_chunk` | `integer` | ✓ |
| `target_dir` | `string` | ✓ |

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
| `bootstrap` | `TRACE` | public | Bootstrap a node from published snapshots | force, chain_dir, ledger_dir, network | epoch |

<details><summary>span: `bootstrap`</summary>

| field | type | required |
| --- | --- | --- |
| `force` | `boolean` | ✓ |
| `chain_dir` | `string` | ✓ |
| `ledger_dir` | `string` | ✓ |
| `network` | `string` | ✓ |
| `epoch` | `string` |  |

</details>

## target: `amaru::cli::snapshot`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `create` | `TRACE` | public | Create snapshots for the given network | network, snapshot_output_dir, config_dir, cardano_node_db, dist_dir | epoch, snapshots |
| `materialize` | `TRACE` | public | Materialize a bootstrap snapshot directory | epoch, snapshot |  |
| `package` | `TRACE` | public | Package a snapshot archive | epoch, archive |  |
| `skip_materialize` | `TRACE` | public | Snapshot already materialized; skipping | epoch, reason |  |
| `skip_package` | `TRACE` | public | Snapshot archive already packaged; skipping | epoch, reason |  |

<details><summary>span: `create`</summary>

| field | type | required |
| --- | --- | --- |
| `network` | `string` | ✓ |
| `snapshot_output_dir` | `string` | ✓ |
| `config_dir` | `string` | ✓ |
| `cardano_node_db` | `string` | ✓ |
| `dist_dir` | `string` | ✓ |
| `epoch` | `string` |  |
| `snapshots` | `string` |  |

</details>

<details><summary>span: `materialize`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `snapshot` | `string` | ✓ |

</details>

<details><summary>span: `package`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `archive` | `string` | ✓ |

</details>

<details><summary>span: `skip_materialize`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `skip_package`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `reason` | `string` | ✓ |

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
| `point_slot` | `string` | ✓ |

</details>

## target: `amaru::ledger::block_validation_context`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `create` | `TRACE` | public | Create validation context for a block | block_body_hash, block_number, block_body_size | total_inputs |

<details><summary>span: `create`</summary>

| field | type | required |
| --- | --- | --- |
| `block_body_hash` | `string` | ✓ |
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
| `begin_epoch` | `TRACE` | public | Perform start-of-epoch epoch boundary computations |  |  |
| `compute` | `TRACE` | public | Epoch transition processing | from, into | skipped, resuming_from |
| `end_epoch` | `TRACE` | public | Perform end-of-epoch epoch boundary computations |  |  |
| `new_governance_updates` | `TRACE` | public | Create governance updates (i.e. ratify proposals) at an epoch boundary. | proposals_count |  |
| `new_pools_updates` | `TRACE` | public | Create pools updates |  |  |
| `record` | `TRACE` | public | Record an in-flight epoch transition | from, to |  |
| `retire_pool` | `TRACE` | public | Retire a pool at an epoch boundary | id |  |
| `rollback` | `TRACE` | public | Rollback an in-flight epoch transition | from, to |  |
| `tick_pool` | `TRACE` | public | Update a pool's parameters at an epoch boundary; only changed parameters are recorded | id | vrf, pledge, cost, margin, reward_account, owners, relays, metadata |

<details><summary>span: `apply`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
| `should_end_epoch` | `boolean` |  |
| `should_snapshot` | `boolean` |  |
| `should_begin_epoch` | `boolean` |  |

</details>

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `string` | ✓ |
| `into` | `string` | ✓ |
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
| `from` | `string` | ✓ |
| `to` | `string` | ✓ |

</details>

<details><summary>span: `retire_pool`</summary>

| field | type | required |
| --- | --- | --- |
| `id` | `string` | ✓ |

</details>

<details><summary>span: `rollback`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `string` | ✓ |
| `to` | `string` | ✓ |

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
| `ratifying_epoch` | `string` | ✓ |
| `treasury` | `integer` |  |
| `votes` | `integer` |  |

</details>

<details><summary>span: `ratify_proposals`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |
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

## target: `amaru::ledger::non_empty_block`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `found` | `TRACE` | public | Found a non-empty block while applying it to the ledger | point, block_height, tx_count |  |

<details><summary>span: `found`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `string` | ✓ |
| `block_height` | `integer` | ✓ |
| `tx_count` | `integer` | ✓ |

</details>

## target: `amaru::ledger::overlay`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `no_governance_updates` | `TRACE` | public | No governance updates found in the epoch transition overlay |  |  |
| `no_pools_updates` | `TRACE` | public | No pools updates found in the epoch transition overlay |  |  |

## target: `amaru::ledger::proposal`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `drop` | `TRACE` | public | Drop an expired or ratified governance proposal | id, expired, ratified_or_evicted |  |
| `skip` | `TRACE` | public | Skip a governance proposal during ratification | id, reason | proposed_in, ratifying_epoch, withdrawal, treasury, invalid_members |

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
| `proposed_in` | `string` |  |
| `ratifying_epoch` | `string` |  |
| `withdrawal` | `integer` |  |
| `treasury` | `integer` |  |
| `invalid_members` | `string` |  |

</details>

## target: `amaru::ledger::proposal_roots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `summarize` | `TRACE` | public | Summary of the governance proposal roots after ratification | constitution, constitutional_committee, hard_fork, protocol_parameters |  |

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
| `ratify` | `TRACE` | public | Ratify a protocol parameters update; only changed parameters are recorded | protocol_version, max_block_body_size, max_transaction_size, max_block_header_size, max_tx_ex_units, max_block_ex_units, max_value_size, max_collateral_inputs, min_fee_a, min_fee_b, stake_credential_deposit, stake_pool_deposit, monetary_expansion_rate, treasury_expansion_rate, min_pool_cost, lovelace_per_utxo_byte, prices, min_fee_ref_script_lovelace_per_byte, max_ref_script_size_per_tx, max_ref_script_size_per_block, ref_script_cost_stride, ref_script_cost_multiplier, stake_pool_max_retirement_epoch, optimal_stake_pools_count, pledge_influence, collateral_percentage, cost_models, pool_voting_thresholds, drep_voting_thresholds, min_committee_size, max_committee_term_length, gov_action_lifetime, gov_action_deposit, drep_deposit, drep_expiry |  |

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
| `summarize` | `TRACE` | public | Summary of the outcome of a ratification round | is_dormant_epoch | pruned_proposals, payouts, new_constitution, constitutional_committee_update |

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
| `payouts` | `string` |  |
| `new_constitution` | `string` |  |
| `constitutional_committee_update` | `string` |  |

</details>

## target: `amaru::ledger::relays`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `collect` | `TRACE` | public | Fetch candidate relays from the immutable store | count |  |

<details><summary>span: `collect`</summary>

| field | type | required |
| --- | --- | --- |
| `count` | `string` |  |

</details>

## target: `amaru::ledger::rewards`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `compute` | `TRACE` | public | Compute rewards for epoch | for_epoch | using_stake_distribution_epoch_from |
| `summarize` | `TRACE` | public | Summary of the rewards calculation for an epoch | efficiency, incentives, treasury_tax, total_rewards, available_rewards, effective_rewards, pots_reserves, pots_treasury, pots_fees |  |

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `for_epoch` | `string` | ✓ |
| `using_stake_distribution_epoch_from` | `string` |  |

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
| `execute` | `TRACE` | public | Validate block against ledger rules |  |  |

## target: `amaru::ledger::rules::phase_one`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `block` | `TRACE` | public | Ledger rules related to block metadata and 'global' preflight checks |  |  |
| `certificates` | `TRACE` | public | Ledger rules and state-transitions for certificates |  |  |
| `collateral` | `TRACE` | public | Ledger rules and state-transitions for collateral |  |  |
| `donation` | `TRACE` | public | Ledger rules and state-transitions for treasury donation |  |  |
| `fees` | `TRACE` | public | Ledger rules and state-transitions for fees |  |  |
| `inputs` | `TRACE` | public | Ledger rules and state-transitions for inputs |  |  |
| `metadata` | `TRACE` | public | Ledger rules and state-transitions for metadata |  |  |
| `mint` | `TRACE` | public | Ledger rules and state-transitions for minte/burned assets |  |  |
| `outputs` | `TRACE` | public | Ledger rules and state-transitions for outputs |  |  |
| `proposals` | `TRACE` | public | Ledger rules and state-transitions for governance proposals |  |  |
| `scripts` | `TRACE` | public | Ledger rules and state-transitions for script witnesses |  |  |
| `signatures` | `TRACE` | public | Ledger rules and state-transitions for key signatures |  |  |
| `validity_interval` | `TRACE` | public | Ledger rules and state-transitions for validity interval |  |  |
| `votes` | `TRACE` | public | Ledger rules and state-transitions for governance votes |  |  |
| `withdrawals` | `TRACE` | public | Ledger rules and state-transitions for withdrawas |  |  |

## target: `amaru::ledger::rules::phase_two`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `acquire_arena` | `TRACE` | public | Acquiring the allocation arena for decoding and execution |  |  |
| `build_script_context` | `TRACE` | public | Initialize script context and cost models, common to all scripts |  |  |
| `build_uplc_program` | `TRACE` | public | Construct the UPLC program from parameters, decoded script and context |  |  |
| `decode_script` | `TRACE` | public | Decoding the script from Cbor/Flat |  |  |
| `evaluate_uplc_program` | `TRACE` | public | Execute the fully-applied UPLC program |  |  |
| `execute_one_script` | `TRACE` | public | A single script execution, with the associated redeemer qualifiers | purpose, index |  |
| `execute_scripts` | `TRACE` | public | A span wrapping all script executions |  |  |

<details><summary>span: `execute_one_script`</summary>

| field | type | required |
| --- | --- | --- |
| `purpose` | `string` | ✓ |
| `index` | `integer` | ✓ |

</details>

## target: `amaru::ledger::stake_distribution`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `compute` | `TRACE` | public | Compute stake distribution for epoch | epoch |  |
| `rotate` | `TRACE` | public | Rotate stake distributions at an epoch boundary | available_stake_distributions |  |
| `snapshot` | `TRACE` | public | Snapshot of the stake distribution taken at an epoch boundary | accounts, dreps, pools, active_stake, pools_voting_stake, dreps_voting_stake |  |

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `string` | ✓ |

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
| `roll_backward` | `TRACE` | public | Roll backward to a specific point | rollback_point |  |
| `roll_forward` | `TRACE` | public | Roll forward with a new block |  |  |

<details><summary>span: `roll_backward`</summary>

| field | type | required |
| --- | --- | --- |
| `rollback_point` | `string` | ✓ |

</details>

## target: `amaru::ledger::transaction`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `certificate_committee_delegate` | `TRACE` | public | Delegate cold key to committee | cc_member, delegate |  |
| `certificate_committee_resign` | `TRACE` | public | Resign from committee | cc_member | anchor_url |
| `certificate_drep_registration` | `TRACE` | public | Register a DRep | drep, deposit | anchor_url |
| `certificate_drep_retirement` | `TRACE` | public | Unregister a DRep | drep, refund |  |
| `certificate_drep_update` | `TRACE` | public | Update DRep anchor | drep | anchor_url |
| `certificate_pool_registration` | `TRACE` | public | Register a pool | pool_id |  |
| `certificate_pool_retirement` | `TRACE` | public | Retire a pool | pool_id, epoch |  |
| `certificate_stake_delegation` | `TRACE` | public | Delegate stake to a pool | credential, pool_id |  |
| `certificate_stake_deregistration` | `TRACE` | public | Unregister a stake credential | credential |  |
| `certificate_stake_registration` | `TRACE` | public | Register a stake credential | credential |  |
| `certificate_vote_delegation` | `TRACE` | public | Delegate vote to DRep | credential | drep |
| `found` | `TRACE` | public | Found a transaction while applying a block | point, block_height, tx_index, tx_id |  |
| `validate` | `TRACE` | public | Validate a single transaction | transaction_id |  |

<details><summary>span: `certificate_committee_delegate`</summary>

| field | type | required |
| --- | --- | --- |
| `cc_member` | `string` | ✓ |
| `delegate` | `string` | ✓ |

</details>

<details><summary>span: `certificate_committee_resign`</summary>

| field | type | required |
| --- | --- | --- |
| `cc_member` | `string` | ✓ |
| `anchor_url` | `string` |  |

</details>

<details><summary>span: `certificate_drep_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `drep` | `string` | ✓ |
| `deposit` | `integer` | ✓ |
| `anchor_url` | `string` |  |

</details>

<details><summary>span: `certificate_drep_retirement`</summary>

| field | type | required |
| --- | --- | --- |
| `drep` | `string` | ✓ |
| `refund` | `integer` | ✓ |

</details>

<details><summary>span: `certificate_drep_update`</summary>

| field | type | required |
| --- | --- | --- |
| `drep` | `string` | ✓ |
| `anchor_url` | `string` |  |

</details>

<details><summary>span: `certificate_pool_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `pool_id` | `string` | ✓ |

</details>

<details><summary>span: `certificate_pool_retirement`</summary>

| field | type | required |
| --- | --- | --- |
| `pool_id` | `string` | ✓ |
| `epoch` | `string` | ✓ |

</details>

<details><summary>span: `certificate_stake_delegation`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |
| `pool_id` | `string` | ✓ |

</details>

<details><summary>span: `certificate_stake_deregistration`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |

</details>

<details><summary>span: `certificate_stake_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |

</details>

<details><summary>span: `certificate_vote_delegation`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |
| `drep` | `string` |  |

</details>

<details><summary>span: `found`</summary>

| field | type | required |
| --- | --- | --- |
| `point` | `string` | ✓ |
| `block_height` | `integer` | ✓ |
| `tx_index` | `integer` | ✓ |
| `tx_id` | `string` | ✓ |

</details>

<details><summary>span: `validate`</summary>

| field | type | required |
| --- | --- | --- |
| `transaction_id` | `string` | ✓ |

</details>

## target: `amaru::ledger::transaction_validation_context`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `create` | `TRACE` | public | Create validation context for a transaction | transaction_id |  |

<details><summary>span: `create`</summary>

| field | type | required |
| --- | --- | --- |
| `transaction_id` | `string` | ✓ |

</details>

## target: `amaru::ledger::validation_context::accounts`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve accounts from the volatile db or the stable one | from_volatile, from_db |  |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::committee`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve committee members from the volatile db or the stable one | from_volatile, from_db |  |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::dreps`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve dreps from the volatile db or the stable one | from_volatile, from_db |  |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::inputs`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve transaction inputs from the volatile db or the stable one | from_volatile, from_db |  |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::pools`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve pools from the volatile db or the stable one | from_volatile, from_db |  |

<details><summary>span: `hydrate`</summary>

| field | type | required |
| --- | --- | --- |
| `from_volatile` | `integer` |  |
| `from_db` | `integer` |  |

</details>

## target: `amaru::ledger::validation_context::proposals`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `hydrate` | `TRACE` | public | Resolve proposals from the volatile db or the stable one | from_volatile, from_db |  |

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
| `rollback_to` | `TRACE` | public | Rollback the volatile state to a specific point | target_slot | last_slot, first_slot, warning, error |
| `warm_up` | `TRACE` | public | The volatile db is still warming up and hasn't reached a stable point yet | size |  |

<details><summary>span: `rollback_to`</summary>

| field | type | required |
| --- | --- | --- |
| `target_slot` | `string` | ✓ |
| `last_slot` | `string` |  |
| `first_slot` | `string` |  |
| `warning` | `string` |  |
| `error` | `string` |  |

</details>

<details><summary>span: `warm_up`</summary>

| field | type | required |
| --- | --- | --- |
| `size` | `integer` | ✓ |

</details>

## target: `amaru::mempool::transaction`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `accepted` | `TRACE` | public | Transaction validated and inserted into the mempool. | tx_id, seq_no, origin |  |
| `evicted` | `TRACE` | public | Transaction removed from the mempool. Reason ∈ {invalid_after_tip}. TODO: split the reason into invalid after tip + present in applied block | tx_id, reason |  |
| `received` | `TRACE` | public | Transaction received by the mempool stage, before validation. | tx_id, origin |  |
| `rejected` | `TRACE` | public | Transaction rejected at insertion. Reason ∈ {invalid, duplicate, mempool_full}. | tx_id, reason | validation_error |

<details><summary>span: `accepted`</summary>

| field | type | required |
| --- | --- | --- |
| `tx_id` | `string` | ✓ |
| `seq_no` | `integer` | ✓ |
| `origin` | `string` | ✓ |

</details>

<details><summary>span: `evicted`</summary>

| field | type | required |
| --- | --- | --- |
| `tx_id` | `string` | ✓ |
| `reason` | `string` | ✓ |

</details>

<details><summary>span: `received`</summary>

| field | type | required |
| --- | --- | --- |
| `tx_id` | `string` | ✓ |
| `origin` | `string` | ✓ |

</details>

<details><summary>span: `rejected`</summary>

| field | type | required |
| --- | --- | --- |
| `tx_id` | `string` | ✓ |
| `reason` | `string` | ✓ |
| `validation_error` | `string` |  |

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
| `connect` | `TRACE` | public | Initiating an outbound connection to a peer | peer |  |
| `connection_died` | `TRACE` | public | A peer connection has died | peer, conn_id, role |  |
| `remove` | `TRACE` | public | A peer was removed from the manager | peer |  |

<details><summary>span: `accepted`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `string` | ✓ |

</details>

<details><summary>span: `add`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `connect`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `connection_died`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `conn_id` | `string` | ✓ |
| `role` | `string` | ✓ |

</details>

<details><summary>span: `remove`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

## target: `amaru::protocols::peer_selection::peer`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `connected` | `TRACE` | public | A connection has been established and the handshake completed successfully. | peer, conn_id, direction, full_duplex_capable, full_duplex |  |
| `disconnected` | `TRACE` | public | A connection has been terminated (graceful disconnect, error, handshake refusal, or network error). | peer, conn_id, direction | reason |

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

## target: `amaru::stores::batch`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `commit` | `TRACE` | public | Commit a write batch |  |  |
| `rollback` | `TRACE` | public | Rollback a write batch |  |  |

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
| `slot` | `string` | ✓ |

</details>

<details><summary>span: `switch_to_fork`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `slot` | `string` | ✓ |

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
| `reset_many` | `TRACE` | public | Reset rewards counters for many accounts | credential, reason |  |
| `set` | `TRACE` | public | Update rewards balance for a single account | credential_type, account, reason |  |

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
| `add` | `TRACE` | public | Batch-upsert DRep registrations | credential, reason |  |
| `get` | `TRACE` | public | Point-read a DRep entry |  |  |
| `remove` | `TRACE` | public | Record DRep de-registration | drep, reason |  |
| `set_valid_until` | `TRACE` | public | Refresh DRep expiry after a vote | credential, reason |  |

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
| `epoch` | `string` | ✓ |

</details>

<details><summary>span: `prune_old_snapshots`</summary>

| field | type | required |
| --- | --- | --- |
| `functional_minimum` | `string` | ✓ |
| `desired_minimum` | `string` | ✓ |

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
| `pay_or_refund_accounts` | `TRACE` | public | Pay withdrawals to accounts, or refund deposits | total_paid_or_refunded, treasury_leftovers |  |
| `pay_rewards` | `TRACE` | public | Pay rewards to all accounts before the epoch end | accounts_paid, rewards_paid, treasury_delta, reserves_delta |  |
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
| `remove` | `TRACE` | public | Schedule pool retirement | pool, reason |  |

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

## target: `amaru::stores::ledger::slots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `get` | `TRACE` | public | Point-read a slot/block-issuer entry |  |  |
| `put` | `TRACE` | public | Write a slot/block-issuer entry |  |  |

## target: `amaru::stores::ledger::snapshots`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `validate` | `TRACE` | public | Validate sufficient snapshots exist | snapshot_count, continuous_ranges |  |

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

## Updating This Documentation

This file is auto-generated from the trace schema definitions in the code. To update it, run:

```bash
./scripts/generate-traces-doc
```

The schemas are defined using the `define_schemas!` macro in the codebase. Any changes to trace definitions will automatically be reflected in this documentation when the script is run.
