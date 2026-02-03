# Available Spans

This document lists all available spans in Amaru, auto-generated from the code.

For information on how to use and filter these spans, see [monitoring/README.md](../monitoring/README.md).


### target: `consensus::chain_sync`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `receive_header` | `TRACE` | Receive header from peer |  |  |
| `decode_header` | `TRACE` | Decode header from raw bytes |  |  |
| `pull` | `TRACE` | Chain sync pull operation |  |  |

### target: `consensus::validate_header`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `validate` | `TRACE` | Validate header cryptographic properties | issuer_key |  |
| `evolve_nonce` | `TRACE` | Evolve the nonce based on header | hash |  |

<details><summary>span: `validate`</summary>

| field | type | required |
| --- | --- | --- |
| `issuer_key` | `string` | ✓ |

</details>

<details><summary>span: `evolve_nonce`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

### target: `ledger::context`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `require_bootstrap_witness` | `TRACE` | Require a bootstrap witness | bootstrap_witness_hash |  |
| `require_script_witness` | `TRACE` | Require a script witness | hash |  |
| `require_vkey_witness` | `TRACE` | Require a verification key witness | hash |  |
| `vote` | `TRACE` | Record a governance vote | voter_type, credential_type, credential_hash |  |
| `withdraw_from` | `TRACE` | Withdraw from stake credential | credential_type, credential_hash |  |
| `add_fees` | `TRACE` | Add transaction fees to pots | fee |  |

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

<details><summary>span: `add_fees`</summary>

| field | type | required |
| --- | --- | --- |
| `fee` | `integer` | ✓ |

</details>

### target: `ledger::governance`

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

### target: `ledger::rules`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `parse_block` | `TRACE` | Parse raw block bytes | block_size |  |

<details><summary>span: `parse_block`</summary>

| field | type | required |
| --- | --- | --- |
| `block_size` | `integer` | ✓ |

</details>

### target: `ledger::state`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `cleanup_expired_proposals` | `TRACE` | Cleanup expired proposals |  |  |
| `cleanup_old_epochs` | `TRACE` | Cleanup old epochs |  |  |
| `manage_transaction_outputs` | `TRACE` | Manage transaction outputs |  |  |
| `ratification_context_new` | `TRACE` | Create ratification context |  |  |
| `roll_backward` | `TRACE` | Roll backward to a specific point |  |  |
| `reset_blocks_count` | `TRACE` | Reset blocks count to zero |  |  |
| `reset_fees` | `TRACE` | Reset fees to zero |  |  |
| `compute_stake_distribution_named` | `TRACE` | Compute stake distribution for epoch |  |  |
| `begin_epoch` | `TRACE` | Begin epoch operations |  |  |
| `end_epoch` | `TRACE` | End epoch operations |  |  |
| `forward` | `TRACE` | Forward ledger state with new volatile state |  |  |
| `compute_rewards` | `TRACE` | Compute rewards for epoch |  |  |
| `tick_pool` | `TRACE` | Tick pool operations |  |  |
| `validate_block` | `TRACE` | Validate block against rules |  |  |
| `prepare_block` | `TRACE` | Prepare block for validation |  |  |
| `tick_proposals` | `TRACE` | Tick proposals for ratification | proposals_count |  |
| `compute_stake_distribution` | `TRACE` | Compute stake distribution for epoch | epoch |  |
| `create_validation_context` | `TRACE` | Create validation context for a block | block_body_hash, block_number, block_body_size | total_inputs |
| `resolve_inputs` | `TRACE` | Resolve transaction inputs from various sources | resolved_from_context, resolved_from_volatile, resolved_from_db |  |
| `epoch_transition` | `TRACE` | Epoch transition processing | from, into |  |
| `apply_block` | `TRACE` | Apply a block to stable state | point_slot |  |
| `roll_forward` | `TRACE` | Roll forward ledger state with a new block |  |  |

<details><summary>span: `tick_proposals`</summary>

| field | type | required |
| --- | --- | --- |
| `proposals_count` | `integer` | ✓ |

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

<details><summary>span: `resolve_inputs`</summary>

| field | type | required |
| --- | --- | --- |
| `resolved_from_context` | `integer` |  |
| `resolved_from_volatile` | `integer` |  |
| `resolved_from_db` | `integer` |  |

</details>

<details><summary>span: `epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `integer` | ✓ |
| `into` | `integer` | ✓ |

</details>

<details><summary>span: `apply_block`</summary>

| field | type | required |
| --- | --- | --- |
| `point_slot` | `integer` | ✓ |

</details>

### target: `network::chainsync_client`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `find_intersection` | `TRACE` | Find chain intersection point with peer | peer, intersection_slot |  |

<details><summary>span: `find_intersection`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `intersection_slot` | `integer` | ✓ |

</details>

### target: `protocols::mux`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `buffer` | `TRACE` | Buffer protocol messages |  |  |
| `register` | `TRACE` | Register protocol with muxer |  |  |
| `mux` | `TRACE` | Multiplex outgoing bytes | bytes |  |
| `demux` | `TRACE` | Demultiplex incoming bytes | proto_id, bytes |  |
| `want_next` | `TRACE` | Want next message for protocol |  |  |
| `received` | `TRACE` | Handle received protocol data | bytes |  |
| `next_segment` | `TRACE` | Get next segment to send |  |  |
| `outgoing` | `TRACE` | Handle outgoing protocol messages | proto_id, bytes |  |

<details><summary>span: `mux`</summary>

| field | type | required |
| --- | --- | --- |
| `bytes` | `integer` | ✓ |

</details>

<details><summary>span: `demux`</summary>

| field | type | required |
| --- | --- | --- |
| `proto_id` | `integer` | ✓ |
| `bytes` | `integer` | ✓ |

</details>

<details><summary>span: `received`</summary>

| field | type | required |
| --- | --- | --- |
| `bytes` | `integer` |  |

</details>

<details><summary>span: `outgoing`</summary>

| field | type | required |
| --- | --- | --- |
| `proto_id` | `string` |  |
| `bytes` | `integer` |  |

</details>

### target: `stores::consensus`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `read_blocks` | `TRACE` | Read blocks operations | hash |  |
| `read_headers` | `TRACE` | Read headers operations | hash |  |
| `rollback_to_tip` | `TRACE` | Rollback to tip operations | hash |  |
| `store_block_to_tip` | `TRACE` | Store block to tip operations | hash |  |
| `rollback_chain` | `TRACE` | Rollback the chain to a point | hash, slot |  |
| `roll_forward_chain` | `TRACE` | Roll forward the chain to a point | hash, slot |  |
| `store_block` | `TRACE` | Store a raw block | hash |  |
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

<details><summary>span: `rollback_to_tip`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `store_block_to_tip`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `rollback_chain`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |

</details>

<details><summary>span: `roll_forward_chain`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |

</details>

<details><summary>span: `store_block`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `store_header`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>

### target: `stores::ledger`

| name | level | description | required fields | optional fields |
| --- | --- | --- | --- | --- |
| `dreps_delegation_remove` | `TRACE` | Remove DRep delegations | drep_hash, drep_type |  |
| `try_epoch_transition` | `TRACE` | Epoch transition tracking | has_from, has_to, point, snapshots |  |
| `prune` | `TRACE` | Prune old snapshots | functional_minimum |  |
| `snapshot` | `TRACE` | Create ledger snapshot for epoch | epoch |  |

<details><summary>span: `dreps_delegation_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `drep_hash` | `string` | ✓ |
| `drep_type` | `string` | ✓ |

</details>

<details><summary>span: `try_epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |
| `has_from` | `boolean` |  |
| `has_to` | `boolean` |  |
| `point` | `string` |  |
| `snapshots` | `string` |  |

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

## Updating This Documentation

This file is auto-generated from the trace schema definitions in the code. To update it, run:

```bash
./scripts/generate-traces-doc
```

The schemas are defined using the `define_schemas!` macro in the codebase. Any changes to trace definitions will automatically be reflected in this documentation when the script is run.
