# Available Spans

This document lists all available spans in Amaru, auto-generated from the code.

For information on how to use and filter these spans, see [monitoring/README.md](../monitoring/README.md).


## target: `amaru::consensus::chain_sync`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `decode_header` | `TRACE` | Decode header from raw bytes |  |  |
| `pull` | `TRACE` | Chain sync pull operation |  |  |
| `receive_header` | `TRACE` | Pull chain updates from peer |  |  |
| `receive_header_decode_failed` | `TRACE` | Header decode failed from received data |  |  |
| `select_chain` | `TRACE` | Select best chain from available headers |  |  |
| `validate_block` | `TRACE` | Validate block properties |  |  |
| `validate_header` | `TRACE` | Validate header properties |  |  |

## target: `amaru::consensus::diffusion`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `fetch_block` | `TRACE` | Fetch a block from the network |  |  |
| `forward_chain` | `TRACE` | Forward chain operations |  |  |

## target: `amaru::consensus::validate_header`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `evolve_nonce` | `TRACE` | Evolve the nonce based on header | hash |  |
| `validate` | `TRACE` | Validate header cryptographic properties | issuer_key |  |

<details><summary>span: `evolve_nonce`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `validate`</summary>

| field | type | required |
| --- | --- | --- |
| `issuer_key` | `string` | ✓ |

</details>

## target: `amaru::ledger::context`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `add_fees` | `TRACE` | Add transaction fees to pots | fee |  |
| `require_bootstrap_witness` | `TRACE` | Require a bootstrap witness | bootstrap_witness_hash |  |
| `require_script_witness` | `TRACE` | Require a script witness | hash |  |
| `require_vkey_witness` | `TRACE` | Require a verification key witness | hash |  |
| `vote` | `TRACE` | Record a governance vote | voter_type, credential_type, credential_hash |  |
| `withdraw_from` | `TRACE` | Withdraw from stake credential | credential_type, credential_hash |  |

<details><summary>span: `add_fees`</summary>

| field | type | required |
| --- | --- | --- |
| `fee` | `integer` | ✓ |

</details>

<details><summary>span: `require_bootstrap_witness`</summary>

| field | type | required |
| --- | --- | --- |
| `bootstrap_witness_hash` | `string` | ✓ |

</details>

<details><summary>span: `require_script_witness`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `require_vkey_witness`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `vote`</summary>

| field | type | required |
| --- | --- | --- |
| `voter_type` | `string` | ✓ |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |

</details>

<details><summary>span: `withdraw_from`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |

</details>

## target: `amaru::ledger::context::default::validation`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `certificate_committee_delegate` | `TRACE` | Delegate cold key to committee | cc_member_type, cc_member_hash, delegate_type, delegate_hash |  |
| `certificate_committee_resign` | `TRACE` | Resign from committee | cc_member_type, cc_member_hash |  |
| `certificate_drep_registration` | `TRACE` | Register a DRep | drep_type, drep_hash, deposit |  |
| `certificate_drep_retirement` | `TRACE` | Unregister a DRep | drep_type, drep_hash, refund |  |
| `certificate_drep_update` | `TRACE` | Update DRep anchor | drep_type, drep_hash |  |
| `certificate_pool_registration` | `TRACE` | Register a pool | pool_id |  |
| `certificate_pool_retirement` | `TRACE` | Retire a pool | pool_id, epoch |  |
| `certificate_stake_delegation` | `TRACE` | Delegate stake to a pool | credential_type, credential_hash, pool_id |  |
| `certificate_stake_deregistration` | `TRACE` | Unregister a stake credential | credential_type, credential_hash |  |
| `certificate_stake_registration` | `TRACE` | Register a stake credential | credential_type, credential_hash |  |
| `certificate_vote_delegation` | `TRACE` | Delegate vote to DRep | credential_type, credential_hash, drep_type, drep_hash |  |

<details><summary>span: `certificate_committee_delegate`</summary>

| field | type | required |
| --- | --- | --- |
| `cc_member_type` | `string` | ✓ |
| `cc_member_hash` | `string` | ✓ |
| `delegate_type` | `string` | ✓ |
| `delegate_hash` | `string` | ✓ |

</details>

<details><summary>span: `certificate_committee_resign`</summary>

| field | type | required |
| --- | --- | --- |
| `cc_member_type` | `string` | ✓ |
| `cc_member_hash` | `string` | ✓ |

</details>

<details><summary>span: `certificate_drep_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `drep_type` | `string` | ✓ |
| `drep_hash` | `string` | ✓ |
| `deposit` | `integer` | ✓ |

</details>

<details><summary>span: `certificate_drep_retirement`</summary>

| field | type | required |
| --- | --- | --- |
| `drep_type` | `string` | ✓ |
| `drep_hash` | `string` | ✓ |
| `refund` | `integer` | ✓ |

</details>

<details><summary>span: `certificate_drep_update`</summary>

| field | type | required |
| --- | --- | --- |
| `drep_type` | `string` | ✓ |
| `drep_hash` | `string` | ✓ |

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
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `certificate_stake_delegation`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |
| `pool_id` | `string` | ✓ |

</details>

<details><summary>span: `certificate_stake_deregistration`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |

</details>

<details><summary>span: `certificate_stake_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |

</details>

<details><summary>span: `certificate_vote_delegation`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |
| `drep_type` | `string` | ✓ |
| `drep_hash` | `string` | ✓ |

</details>

## target: `amaru::ledger::governance`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `ratify_proposals` | `TRACE` | Ratify proposals at epoch boundary | roots_protocol_parameters, roots_hard_fork, roots_constitutional_committee, roots_constitution |  |

<details><summary>span: `ratify_proposals`</summary>

| field | type | required |
| --- | --- | --- |
| `roots_protocol_parameters` | `string` |  |
| `roots_hard_fork` | `string` |  |
| `roots_constitutional_committee` | `string` |  |
| `roots_constitution` | `string` |  |

</details>

## target: `amaru::ledger::rules`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `parse_block` | `TRACE` | Parse raw block bytes | block_size |  |

<details><summary>span: `parse_block`</summary>

| field | type | required |
| --- | --- | --- |
| `block_size` | `integer` | ✓ |

</details>

## target: `amaru::ledger::state`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `apply_block` | `TRACE` | Apply a block to stable state | point_slot |  |
| `begin_epoch` | `TRACE` | Begin epoch operations |  |  |
| `cleanup_expired_proposals` | `TRACE` | Cleanup expired proposals |  |  |
| `cleanup_old_epochs` | `TRACE` | Cleanup old epochs |  |  |
| `compute_rewards` | `TRACE` | Compute rewards for epoch |  |  |
| `compute_stake_distribution` | `TRACE` | Compute stake distribution for epoch | epoch |  |
| `compute_stake_distribution_named` | `TRACE` | Compute stake distribution for epoch |  |  |
| `create_validation_context` | `TRACE` | Create validation context for a block | block_body_hash, block_number, block_body_size | total_inputs |
| `end_epoch` | `TRACE` | End epoch operations |  |  |
| `epoch_transition` | `TRACE` | Epoch transition processing | from, into |  |
| `forward` | `TRACE` | Forward ledger state with new volatile state |  |  |
| `manage_transaction_outputs` | `TRACE` | Manage transaction outputs |  |  |
| `prepare_block` | `TRACE` | Prepare block for validation |  |  |
| `ratification_context_new` | `TRACE` | Create ratification context |  |  |
| `reset_blocks_count` | `TRACE` | Reset blocks count to zero |  |  |
| `reset_fees` | `TRACE` | Reset fees to zero |  |  |
| `resolve_inputs` | `TRACE` | Resolve transaction inputs from various sources | resolved_from_context, resolved_from_volatile, resolved_from_db |  |
| `roll_backward` | `TRACE` | Roll backward to a specific point |  |  |
| `roll_forward` | `TRACE` | Roll forward ledger state with a new block |  |  |
| `tick_pool` | `TRACE` | Tick pool operations |  |  |
| `tick_proposals` | `TRACE` | Tick proposals for ratification | proposals_count |  |
| `validate_block` | `TRACE` | Validate block against rules |  |  |

<details><summary>span: `apply_block`</summary>

| field | type | required |
| --- | --- | --- |
| `point_slot` | `integer` | ✓ |

</details>

<details><summary>span: `compute_stake_distribution`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `create_validation_context`</summary>

| field | type | required |
| --- | --- | --- |
| `block_body_hash` | `string` | ✓ |
| `block_number` | `integer` | ✓ |
| `block_body_size` | `integer` | ✓ |
| `total_inputs` | `integer` |  |

</details>

<details><summary>span: `epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `integer` | ✓ |
| `into` | `integer` | ✓ |

</details>

<details><summary>span: `resolve_inputs`</summary>

| field | type | required |
| --- | --- | --- |
| `resolved_from_context` | `integer` |  |
| `resolved_from_volatile` | `integer` |  |
| `resolved_from_db` | `integer` |  |

</details>

<details><summary>span: `tick_proposals`</summary>

| field | type | required |
| --- | --- | --- |
| `proposals_count` | `integer` | ✓ |

</details>

## target: `amaru::network::chainsync_client`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `find_intersection` | `TRACE` | Find chain intersection point with peer | peer, intersection_slot |  |

<details><summary>span: `find_intersection`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `intersection_slot` | `integer` | ✓ |

</details>

## target: `amaru::network::connection`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `accept` | `TRACE` | Accept a connection |  |  |
| `accept_loop` | `TRACE` | Accept loop for incoming connections |  |  |
| `close` | `TRACE` | Close connection |  |  |
| `connect` | `TRACE` | Connect to addresses |  |  |
| `connect_addrs` | `TRACE` | Connect to multiple addresses |  |  |
| `listen` | `TRACE` | Listen on address |  |  |
| `recv` | `TRACE` | Receive data from connection |  |  |
| `send` | `TRACE` | Send data over connection |  |  |

## target: `amaru::protocols::mux`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `buffer` | `TRACE` | Buffer protocol messages |  |  |
| `demux` | `TRACE` | Demultiplex incoming bytes | proto_id, bytes |  |
| `mux` | `TRACE` | Multiplex outgoing bytes | bytes |  |
| `next_segment` | `TRACE` | Get next segment to send |  |  |
| `outgoing` | `TRACE` | Handle outgoing protocol messages | proto_id, bytes |  |
| `received` | `TRACE` | Handle received protocol data | bytes |  |
| `register` | `TRACE` | Register protocol with muxer |  |  |
| `want_next` | `TRACE` | Want next message for protocol |  |  |

<details><summary>span: `demux`</summary>

| field | type | required |
| --- | --- | --- |
| `proto_id` | `integer` | ✓ |
| `bytes` | `integer` | ✓ |

</details>

<details><summary>span: `mux`</summary>

| field | type | required |
| --- | --- | --- |
| `bytes` | `integer` | ✓ |

</details>

<details><summary>span: `outgoing`</summary>

| field | type | required |
| --- | --- | --- |
| `proto_id` | `string` |  |
| `bytes` | `integer` |  |

</details>

<details><summary>span: `received`</summary>

| field | type | required |
| --- | --- | --- |
| `bytes` | `integer` |  |

</details>

## target: `amaru::simulator::node`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `handle_msg` | `TRACE` | Handle message in simulator node |  |  |

## target: `amaru::stage::logging`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `test_span` | `TRACE` | Test span for logging |  |  |

## target: `amaru::stage::tokio`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `poll` | `TRACE` | Poll stage operation |  |  |

## target: `amaru::stores::consensus`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `read_blocks` | `TRACE` | Read blocks operations | hash |  |
| `read_headers` | `TRACE` | Read headers operations | hash |  |
| `roll_forward_chain` | `TRACE` | Roll forward the chain to a point | hash, slot |  |
| `rollback_chain` | `TRACE` | Rollback the chain to a point | hash, slot |  |
| `rollback_to_tip` | `TRACE` | Rollback to tip operations | hash |  |
| `store_block` | `TRACE` | Store a raw block | hash |  |
| `store_block_to_tip` | `TRACE` | Store block to tip operations | hash |  |
| `store_header` | `TRACE` | Store a block header | hash |  |

<details><summary>span: `read_blocks`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `read_headers`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `roll_forward_chain`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |

</details>

<details><summary>span: `rollback_chain`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |

</details>

<details><summary>span: `rollback_to_tip`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `store_block`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `store_block_to_tip`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `store_header`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

## target: `amaru::stores::ledger`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `dreps_delegation_remove` | `TRACE` | Remove DRep delegations | drep_hash, drep_type |  |
| `prune` | `TRACE` | Prune old snapshots | functional_minimum |  |
| `snapshot` | `TRACE` | Create ledger snapshot for epoch | epoch |  |
| `try_epoch_transition` | `TRACE` | Epoch transition tracking | has_from, has_to, point, snapshots |  |

<details><summary>span: `dreps_delegation_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `drep_hash` | `string` | ✓ |
| `drep_type` | `string` | ✓ |

</details>

<details><summary>span: `prune`</summary>

| field | type | required |
| --- | --- | --- |
| `functional_minimum` | `integer` | ✓ |

</details>

<details><summary>span: `snapshot`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `try_epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |
| `has_from` | `boolean` |  |
| `has_to` | `boolean` |  |
| `point` | `string` |  |
| `snapshots` | `string` |  |

</details>

## target: `amaru::stores::rocksdb`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `save_point` | `TRACE` | Save point to RocksDB store | slot | epoch |
| `validate_snapshots` | `TRACE` | Validate sufficient snapshots exist | snapshot_count, continuous_ranges |  |

<details><summary>span: `save_point`</summary>

| field | type | required |
| --- | --- | --- |
| `slot` | `integer` | ✓ |
| `epoch` | `integer` |  |

</details>

<details><summary>span: `validate_snapshots`</summary>

| field | type | required |
| --- | --- | --- |
| `snapshot_count` | `integer` |  |
| `continuous_ranges` | `integer` |  |

</details>

## Updating This Documentation

This file is auto-generated from the trace schema definitions in the code. To update it, run:

```bash
./scripts/generate-traces-doc
```

The schemas are defined using the `define_schemas!` macro in the codebase. Any changes to trace definitions will automatically be reflected in this documentation when the script is run.
