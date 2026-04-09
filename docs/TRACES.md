# Available Spans

This document lists all available spans in Amaru, auto-generated from the code.

For information on how to use and filter these spans, see [monitoring/README.md](../monitoring/README.md).


## target: `amaru::ledger::context`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add_fees` | `TRACE` | public | Add transaction fees to pots | fee |  |
| `require_bootstrap_witness` | `TRACE` | public | Require a bootstrap witness | bootstrap_witness_hash |  |
| `require_script_witness` | `TRACE` | public | Require a script witness | hash |  |
| `require_vkey_witness` | `TRACE` | public | Require a verification key witness | hash |  |
| `vote` | `TRACE` | public | Record a governance vote | voter_type, credential_type, credential_hash |  |
| `withdraw_from` | `TRACE` | public | Withdraw from stake credential | credential_type, credential_hash |  |

## target: `amaru::ledger::context`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add_fees` | `TRACE` | public | Add transaction fees to pots | fee |  |
| `require_bootstrap_witness` | `TRACE` | public | Require a bootstrap witness | bootstrap_witness_hash |  |
| `require_script_witness` | `TRACE` | public | Require a script witness | hash |  |
| `require_vkey_witness` | `TRACE` | public | Require a verification key witness | hash |  |
| `vote` | `TRACE` | public | Record a governance vote | voter_type, credential_type, credential_hash |  |
| `withdraw_from` | `TRACE` | public | Withdraw from stake credential | credential_type, credential_hash |  |

<details><summary>span: `add_fees`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `add_fees`</summary>

| field | type | required |
| --- | --- | --- |
| `fee` | `integer` | ✓ |

</details>
| `fee` | `integer` | ✓ |

</details>

<details><summary>span: `require_bootstrap_witness`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `require_bootstrap_witness`</summary>

| field | type | required |
| --- | --- | --- |
| `bootstrap_witness_hash` | `string` | ✓ |

</details>
| `bootstrap_witness_hash` | `string` | ✓ |

</details>

<details><summary>span: `require_script_witness`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `require_script_witness`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `require_vkey_witness`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `require_vkey_witness`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |

</details>
| `hash` | `string` | ✓ |

</details>

<details><summary>span: `vote`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `vote`</summary>

| field | type | required |
| --- | --- | --- |
| `voter_type` | `string` | ✓ |
| `voter_type` | `string` | ✓ |
| `credential_type` | `string` | ✓ |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |

</details>
| `credential_hash` | `string` | ✓ |

</details>

<details><summary>span: `withdraw_from`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `withdraw_from`</summary>

| field | type | required |
| --- | --- | --- |
| `credential_type` | `string` | ✓ |
| `credential_type` | `string` | ✓ |
| `credential_hash` | `string` | ✓ |

</details>

## target: `amaru::ledger::context::default::validation`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `credential_hash` | `string` | ✓ |

</details>
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

## target: `amaru::ledger::context::default::validation`

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

<details><summary>span: `certificate_committee_delegate`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_committee_delegate`</summary>

| field | type | required |
| --- | --- | --- |
| `cc_member` | `string` | ✓ |
| `cc_member` | `string` | ✓ |
| `delegate` | `string` | ✓ |

</details>
| `delegate` | `string` | ✓ |

</details>

<details><summary>span: `certificate_committee_resign`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_committee_resign`</summary>

| field | type | required |
| --- | --- | --- |
| `cc_member` | `string` | ✓ |
| `cc_member` | `string` | ✓ |
| `anchor_url` | `string` |  |

</details>
| `anchor_url` | `string` |  |

</details>

<details><summary>span: `certificate_drep_registration`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_drep_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `drep` | `string` | ✓ |
| `drep` | `string` | ✓ |
| `deposit` | `integer` | ✓ |
| `deposit` | `integer` | ✓ |
| `anchor_url` | `string` |  |

</details>
| `anchor_url` | `string` |  |

</details>

<details><summary>span: `certificate_drep_retirement`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_drep_retirement`</summary>

| field | type | required |
| --- | --- | --- |
| `drep` | `string` | ✓ |
| `drep` | `string` | ✓ |
| `refund` | `integer` | ✓ |

</details>
| `refund` | `integer` | ✓ |

</details>

<details><summary>span: `certificate_drep_update`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_drep_update`</summary>

| field | type | required |
| --- | --- | --- |
| `drep` | `string` | ✓ |
| `drep` | `string` | ✓ |
| `anchor_url` | `string` |  |

</details>
| `anchor_url` | `string` |  |

</details>

<details><summary>span: `certificate_pool_registration`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_pool_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `pool_id` | `string` | ✓ |

</details>
| `pool_id` | `string` | ✓ |

</details>

<details><summary>span: `certificate_pool_retirement`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_pool_retirement`</summary>

| field | type | required |
| --- | --- | --- |
| `pool_id` | `string` | ✓ |
| `pool_id` | `string` | ✓ |
| `epoch` | `integer` | ✓ |

</details>
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `certificate_stake_delegation`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_stake_delegation`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |
| `credential` | `string` | ✓ |
| `pool_id` | `string` | ✓ |

</details>
| `pool_id` | `string` | ✓ |

</details>

<details><summary>span: `certificate_stake_deregistration`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_stake_deregistration`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |

</details>
| `credential` | `string` | ✓ |

</details>

<details><summary>span: `certificate_stake_registration`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_stake_registration`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |

</details>
| `credential` | `string` | ✓ |

</details>

<details><summary>span: `certificate_vote_delegation`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `certificate_vote_delegation`</summary>

| field | type | required |
| --- | --- | --- |
| `credential` | `string` | ✓ |
| `credential` | `string` | ✓ |
| `drep` | `string` |  |

</details>

## target: `amaru::ledger::governance`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `drep` | `string` |  |

</details>

## target: `amaru::ledger::governance`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `ratify_proposals` | `TRACE` | public | Ratify proposals at epoch boundary | roots_protocol_parameters, roots_hard_fork, roots_constitutional_committee, roots_constitution |  |
| `ratify_proposals` | `TRACE` | public | Ratify proposals at epoch boundary | roots_protocol_parameters, roots_hard_fork, roots_constitutional_committee, roots_constitution |  |

<details><summary>span: `ratify_proposals`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `ratify_proposals`</summary>

| field | type | required |
| --- | --- | --- |
| `roots_protocol_parameters` | `string` |  |
| `roots_protocol_parameters` | `string` |  |
| `roots_hard_fork` | `string` |  |
| `roots_hard_fork` | `string` |  |
| `roots_constitutional_committee` | `string` |  |
| `roots_constitutional_committee` | `string` |  |
| `roots_constitution` | `string` |  |

</details>

## target: `amaru::ledger::state`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `roots_constitution` | `string` |  |

</details>

## target: `amaru::ledger::state`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `apply_block` | `TRACE` | public | Apply a block to stable state | point_slot |  |
| `begin_epoch` | `TRACE` | public | Begin epoch operations |  |  |
| `compute_rewards` | `TRACE` | public | Compute rewards for epoch |  |  |
| `compute_stake_distribution` | `TRACE` | public | Compute stake distribution for epoch | epoch |  |
| `create_validation_context` | `TRACE` | public | Create validation context for a block | block_body_hash, block_number, block_body_size | total_inputs |
| `end_epoch` | `TRACE` | public | End epoch operations |  |  |
| `epoch_transition` | `TRACE` | public | Epoch transition processing | from, into |  |
| `forward` | `TRACE` | public | Forward ledger state with new volatile state |  |  |
| `prepare_block` | `TRACE` | public | Prepare block for validation |  |  |
| `ratification_context_new` | `TRACE` | public | Create ratification context |  |  |
| `reset_blocks_count` | `TRACE` | public | Reset blocks count to zero |  |  |
| `reset_fees` | `TRACE` | public | Reset fees to zero |  |  |
| `resolve_inputs` | `TRACE` | public | Resolve transaction inputs from various sources | resolved_from_context, resolved_from_volatile, resolved_from_db |  |
| `roll_backward` | `TRACE` | public | Roll backward to a specific point | rollback_point |  |
| `roll_forward` | `TRACE` | public | Roll forward ledger state with a new block |  |  |
| `tick_pool` | `TRACE` | public | Tick pool operations |  |  |
| `tick_proposals` | `TRACE` | public | Tick proposals for ratification | proposals_count |  |
| `validate_block` | `TRACE` | public | Validate block against rules |  |  |
| `volatile_to_stable` | `TRACE` | public | Persist the oldest volatile block to stable storage once the security parameter is reached | persisted_point, volatile_len_before, volatile_len_after, k |  |
| `apply_block` | `TRACE` | public | Apply a block to stable state | point_slot |  |
| `begin_epoch` | `TRACE` | public | Begin epoch operations |  |  |
| `compute_rewards` | `TRACE` | public | Compute rewards for epoch |  |  |
| `compute_stake_distribution` | `TRACE` | public | Compute stake distribution for epoch | epoch |  |
| `create_validation_context` | `TRACE` | public | Create validation context for a block | block_body_hash, block_number, block_body_size | total_inputs |
| `end_epoch` | `TRACE` | public | End epoch operations |  |  |
| `epoch_transition` | `TRACE` | public | Epoch transition processing | from, into |  |
| `forward` | `TRACE` | public | Forward ledger state with new volatile state |  |  |
| `prepare_block` | `TRACE` | public | Prepare block for validation |  |  |
| `ratification_context_new` | `TRACE` | public | Create ratification context |  |  |
| `reset_blocks_count` | `TRACE` | public | Reset blocks count to zero |  |  |
| `reset_fees` | `TRACE` | public | Reset fees to zero |  |  |
| `resolve_inputs` | `TRACE` | public | Resolve transaction inputs from various sources | resolved_from_context, resolved_from_volatile, resolved_from_db |  |
| `roll_backward` | `TRACE` | public | Roll backward to a specific point | rollback_point |  |
| `roll_forward` | `TRACE` | public | Roll forward ledger state with a new block |  |  |
| `tick_pool` | `TRACE` | public | Tick pool operations |  |  |
| `tick_proposals` | `TRACE` | public | Tick proposals for ratification | proposals_count |  |
| `validate_block` | `TRACE` | public | Validate block against rules |  |  |
| `volatile_to_stable` | `TRACE` | public | Persist the oldest volatile block to stable storage once the security parameter is reached | persisted_point, volatile_len_before, volatile_len_after, k |  |

<details><summary>span: `apply_block`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `apply_block`</summary>

| field | type | required |
| --- | --- | --- |
| `point_slot` | `integer` | ✓ |

</details>
| `point_slot` | `integer` | ✓ |

</details>

<details><summary>span: `compute_stake_distribution`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `compute_stake_distribution`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

</details>
| `epoch` | `integer` | ✓ |

</details>

<details><summary>span: `create_validation_context`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `create_validation_context`</summary>

| field | type | required |
| --- | --- | --- |
| `block_body_hash` | `string` | ✓ |
| `block_body_hash` | `string` | ✓ |
| `block_number` | `integer` | ✓ |
| `block_number` | `integer` | ✓ |
| `block_body_size` | `integer` | ✓ |
| `block_body_size` | `integer` | ✓ |
| `total_inputs` | `integer` |  |

</details>
| `total_inputs` | `integer` |  |

</details>

<details><summary>span: `epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |
| `from` | `integer` | ✓ |
| `from` | `integer` | ✓ |
| `into` | `integer` | ✓ |

</details>
| `into` | `integer` | ✓ |

</details>

<details><summary>span: `resolve_inputs`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `resolve_inputs`</summary>

| field | type | required |
| --- | --- | --- |
| `resolved_from_context` | `integer` |  |
| `resolved_from_context` | `integer` |  |
| `resolved_from_volatile` | `integer` |  |
| `resolved_from_volatile` | `integer` |  |
| `resolved_from_db` | `integer` |  |

</details>
| `resolved_from_db` | `integer` |  |

</details>

<details><summary>span: `roll_backward`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `roll_backward`</summary>

| field | type | required |
| --- | --- | --- |
| `rollback_point` | `string` | ✓ |

</details>
| `rollback_point` | `string` | ✓ |

</details>

<details><summary>span: `tick_proposals`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `tick_proposals`</summary>

| field | type | required |
| --- | --- | --- |
| `proposals_count` | `integer` | ✓ |

</details>
| `proposals_count` | `integer` | ✓ |

</details>

<details><summary>span: `volatile_to_stable`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `volatile_to_stable`</summary>

| field | type | required |
| --- | --- | --- |
| `persisted_point` | `string` | ✓ |
| `persisted_point` | `string` | ✓ |
| `volatile_len_before` | `integer` | ✓ |
| `volatile_len_before` | `integer` | ✓ |
| `volatile_len_after` | `integer` | ✓ |
| `volatile_len_after` | `integer` | ✓ |
| `k` | `integer` | ✓ |

</details>

## target: `amaru::protocols::manager`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `k` | `integer` | ✓ |

</details>
| `accepted` | `TRACE` | public | An inbound connection was accepted from a peer | peer, conn_id |  |
| `add_peer` | `TRACE` | public | A new peer was added to the manager | peer |  |

## target: `amaru::protocols::manager`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `connect` | `TRACE` | public | Initiating an outbound connection to a peer | peer |  |
| `connection_died` | `TRACE` | public | A peer connection has died | peer, conn_id, role |  |
| `manager_stage` | `TRACE` | public | Handle manager stage messages | message_type |  |
| `remove_peer` | `TRACE` | public | A peer was removed from the manager | peer |  |
| `accepted` | `TRACE` | public | An inbound connection was accepted from a peer | peer, conn_id |  |
| `add_peer` | `TRACE` | public | A new peer was added to the manager | peer |  |
| `connect` | `TRACE` | public | Initiating an outbound connection to a peer | peer |  |
| `connection_died` | `TRACE` | public | A peer connection has died | peer, conn_id, role |  |
| `manager_stage` | `TRACE` | public | Handle manager stage messages | message_type |  |
| `remove_peer` | `TRACE` | public | A peer was removed from the manager | peer |  |

<details><summary>span: `accepted`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `accepted`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `peer` | `string` | ✓ |
| `conn_id` | `string` | ✓ |

</details>
| `conn_id` | `string` | ✓ |

</details>

<details><summary>span: `add_peer`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `add_peer`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `connect`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `connect`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>
| `peer` | `string` | ✓ |

</details>

<details><summary>span: `connection_died`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `connection_died`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |
| `peer` | `string` | ✓ |
| `conn_id` | `string` | ✓ |
| `conn_id` | `string` | ✓ |
| `role` | `string` | ✓ |

</details>
| `role` | `string` | ✓ |

</details>

<details><summary>span: `manager_stage`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `manager_stage`</summary>

| field | type | required |
| --- | --- | --- |
| `message_type` | `string` | ✓ |

</details>
| `message_type` | `string` | ✓ |

</details>

<details><summary>span: `remove_peer`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `remove_peer`</summary>

| field | type | required |
| --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

## target: `amaru::stores::consensus`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `peer` | `string` | ✓ |

</details>

## target: `amaru::stores::consensus`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `roll_forward_chain` | `TRACE` | public | Roll forward the chain to a point | hash, slot, db_system_name, db_operation_name, db_collection_name |  |
| `rollback_chain` | `TRACE` | public | Rollback the chain to a point | hash, slot, db_system_name, db_operation_name, db_collection_name |  |
| `store_block` | `TRACE` | public | Store a raw block | hash, db_system_name, db_operation_name, db_collection_name |  |
| `store_header` | `TRACE` | public | Store a block header | hash, db_system_name, db_operation_name, db_collection_name |  |
| `roll_forward_chain` | `TRACE` | public | Roll forward the chain to a point | hash, slot, db_system_name, db_operation_name, db_collection_name |  |
| `rollback_chain` | `TRACE` | public | Rollback the chain to a point | hash, slot, db_system_name, db_operation_name, db_collection_name |  |
| `store_block` | `TRACE` | public | Store a raw block | hash, db_system_name, db_operation_name, db_collection_name |  |
| `store_header` | `TRACE` | public | Store a block header | hash, db_system_name, db_operation_name, db_collection_name |  |

<details><summary>span: `roll_forward_chain`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `roll_forward_chain`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `rollback_chain`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `rollback_chain`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `hash` | `string` | ✓ |
| `slot` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `store_block`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `store_block`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `hash` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `store_header`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `store_header`</summary>

| field | type | required |
| --- | --- | --- |
| `hash` | `string` | ✓ |
| `hash` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>

## target: `amaru::stores::ledger`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `db_collection_name` | `string` | ✓ |

</details>

## target: `amaru::stores::ledger`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `dreps_delegation_remove` | `TRACE` | public | Remove DRep delegations | drep_hash, drep_type, db_system_name, db_operation_name, db_collection_name |  |
| `prune` | `TRACE` | public | Prune old snapshots | functional_minimum, db_system_name, db_operation_name |  |
| `snapshot` | `TRACE` | public | Create ledger snapshot for epoch | epoch, db_system_name, db_operation_name |  |
| `try_epoch_transition` | `TRACE` | public | Epoch transition tracking | db_system_name, db_operation_name | has_from, has_to, point, snapshots |
| `dreps_delegation_remove` | `TRACE` | public | Remove DRep delegations | drep_hash, drep_type, db_system_name, db_operation_name, db_collection_name |  |
| `prune` | `TRACE` | public | Prune old snapshots | functional_minimum, db_system_name, db_operation_name |  |
| `snapshot` | `TRACE` | public | Create ledger snapshot for epoch | epoch, db_system_name, db_operation_name |  |
| `try_epoch_transition` | `TRACE` | public | Epoch transition tracking | db_system_name, db_operation_name | has_from, has_to, point, snapshots |

<details><summary>span: `dreps_delegation_remove`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `dreps_delegation_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `drep_hash` | `string` | ✓ |
| `drep_hash` | `string` | ✓ |
| `drep_type` | `string` | ✓ |
| `drep_type` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `prune`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `prune`</summary>

| field | type | required |
| --- | --- | --- |
| `functional_minimum` | `integer` | ✓ |
| `functional_minimum` | `integer` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |

</details>
| `db_operation_name` | `string` | ✓ |

</details>

<details><summary>span: `snapshot`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `snapshot`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |
| `epoch` | `integer` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |

</details>
| `db_operation_name` | `string` | ✓ |

</details>

<details><summary>span: `try_epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `try_epoch_transition`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `has_from` | `boolean` |  |
| `has_from` | `boolean` |  |
| `has_to` | `boolean` |  |
| `has_to` | `boolean` |  |
| `point` | `string` |  |
| `point` | `string` |  |
| `snapshots` | `string` |  |

</details>

## target: `amaru::stores::ledger::columns`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `snapshots` | `string` |  |

</details>

## target: `amaru::stores::ledger::columns`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `accounts_add` | `TRACE` | public | Batch-upsert account entries | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_get` | `TRACE` | public | Point-read an account entry | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_remove` | `TRACE` | public | Batch-delete account entries | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_reset_delegation` | `TRACE` | public | Clear DRep delegation for accounts (protocol v9 bug compat) | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_reset_many` | `TRACE` | public | Reset rewards counters for many accounts | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_set` | `TRACE` | public | Update rewards balance for a single account | db_system_name, db_operation_name, db_collection_name |  |
| `cc_members_upsert` | `TRACE` | public | Upsert a constitutional committee member | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_add` | `TRACE` | public | Batch-upsert DRep registrations | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_get` | `TRACE` | public | Point-read a DRep entry | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_remove` | `TRACE` | public | Record DRep de-registration | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_set_valid_until` | `TRACE` | public | Refresh DRep expiry after a vote | db_system_name, db_operation_name, db_collection_name |  |
| `iter_scan` | `TRACE` | public | Full-table scan via IterBorrow (tick/epoch operations) | db_system_name, db_operation_name, db_collection_name | rows_scanned, rows_written, rows_deleted |
| `pools_add` | `TRACE` | public | Batch-upsert pool entries | db_system_name, db_operation_name, db_collection_name |  |
| `pools_get` | `TRACE` | public | Point-read a pool entry | db_system_name, db_operation_name, db_collection_name |  |
| `pools_remove` | `TRACE` | public | Schedule pool retirement | db_system_name, db_operation_name, db_collection_name |  |
| `pots_get` | `TRACE` | public | Read treasury/reserve/fees pots | db_system_name, db_operation_name, db_collection_name |  |
| `pots_put` | `TRACE` | public | Write treasury/reserve/fees pots | db_system_name, db_operation_name, db_collection_name |  |
| `proposals_add` | `TRACE` | public | Insert governance proposals | db_system_name, db_operation_name, db_collection_name |  |
| `proposals_remove` | `TRACE` | public | Remove enacted or expired proposals | db_system_name, db_operation_name, db_collection_name |  |
| `slots_get` | `TRACE` | public | Point-read a slot/block-issuer entry | db_system_name, db_operation_name, db_collection_name |  |
| `slots_put` | `TRACE` | public | Write a slot/block-issuer entry | db_system_name, db_operation_name, db_collection_name |  |
| `utxo_add` | `TRACE` | public | Batch-insert UTxO entries | db_system_name, db_operation_name, db_collection_name |  |
| `utxo_get` | `TRACE` | public | Point-read a UTxO entry | db_system_name, db_operation_name, db_collection_name |  |
| `utxo_remove` | `TRACE` | public | Batch-delete UTxO entries | db_system_name, db_operation_name, db_collection_name |  |
| `votes_add` | `TRACE` | public | Record governance votes | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_add` | `TRACE` | public | Batch-upsert account entries | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_get` | `TRACE` | public | Point-read an account entry | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_remove` | `TRACE` | public | Batch-delete account entries | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_reset_delegation` | `TRACE` | public | Clear DRep delegation for accounts (protocol v9 bug compat) | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_reset_many` | `TRACE` | public | Reset rewards counters for many accounts | db_system_name, db_operation_name, db_collection_name |  |
| `accounts_set` | `TRACE` | public | Update rewards balance for a single account | db_system_name, db_operation_name, db_collection_name |  |
| `cc_members_upsert` | `TRACE` | public | Upsert a constitutional committee member | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_add` | `TRACE` | public | Batch-upsert DRep registrations | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_get` | `TRACE` | public | Point-read a DRep entry | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_remove` | `TRACE` | public | Record DRep de-registration | db_system_name, db_operation_name, db_collection_name |  |
| `dreps_set_valid_until` | `TRACE` | public | Refresh DRep expiry after a vote | db_system_name, db_operation_name, db_collection_name |  |
| `iter_scan` | `TRACE` | public | Full-table scan via IterBorrow (tick/epoch operations) | db_system_name, db_operation_name, db_collection_name | rows_scanned, rows_written, rows_deleted |
| `pools_add` | `TRACE` | public | Batch-upsert pool entries | db_system_name, db_operation_name, db_collection_name |  |
| `pools_get` | `TRACE` | public | Point-read a pool entry | db_system_name, db_operation_name, db_collection_name |  |
| `pools_remove` | `TRACE` | public | Schedule pool retirement | db_system_name, db_operation_name, db_collection_name |  |
| `pots_get` | `TRACE` | public | Read treasury/reserve/fees pots | db_system_name, db_operation_name, db_collection_name |  |
| `pots_put` | `TRACE` | public | Write treasury/reserve/fees pots | db_system_name, db_operation_name, db_collection_name |  |
| `proposals_add` | `TRACE` | public | Insert governance proposals | db_system_name, db_operation_name, db_collection_name |  |
| `proposals_remove` | `TRACE` | public | Remove enacted or expired proposals | db_system_name, db_operation_name, db_collection_name |  |
| `slots_get` | `TRACE` | public | Point-read a slot/block-issuer entry | db_system_name, db_operation_name, db_collection_name |  |
| `slots_put` | `TRACE` | public | Write a slot/block-issuer entry | db_system_name, db_operation_name, db_collection_name |  |
| `utxo_add` | `TRACE` | public | Batch-insert UTxO entries | db_system_name, db_operation_name, db_collection_name |  |
| `utxo_get` | `TRACE` | public | Point-read a UTxO entry | db_system_name, db_operation_name, db_collection_name |  |
| `utxo_remove` | `TRACE` | public | Batch-delete UTxO entries | db_system_name, db_operation_name, db_collection_name |  |
| `votes_add` | `TRACE` | public | Record governance votes | db_system_name, db_operation_name, db_collection_name |  |

<details><summary>span: `accounts_add`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `accounts_add`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `accounts_get`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `accounts_get`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `accounts_remove`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `accounts_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `accounts_reset_delegation`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `accounts_reset_delegation`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `accounts_reset_many`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `accounts_reset_many`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `accounts_set`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `accounts_set`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `cc_members_upsert`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `cc_members_upsert`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `dreps_add`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `dreps_add`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `dreps_get`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `dreps_get`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `dreps_remove`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `dreps_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `dreps_set_valid_until`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `dreps_set_valid_until`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `iter_scan`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `iter_scan`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |
| `rows_scanned` | `integer` |  |
| `rows_scanned` | `integer` |  |
| `rows_written` | `integer` |  |
| `rows_written` | `integer` |  |
| `rows_deleted` | `integer` |  |

</details>
| `rows_deleted` | `integer` |  |

</details>

<details><summary>span: `pools_add`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `pools_add`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `pools_get`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `pools_get`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `pools_remove`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `pools_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `pots_get`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `pots_get`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `pots_put`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `pots_put`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `proposals_add`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `proposals_add`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `proposals_remove`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `proposals_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `slots_get`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `slots_get`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `slots_put`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `slots_put`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `utxo_add`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `utxo_add`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `utxo_get`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `utxo_get`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `utxo_remove`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `utxo_remove`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>
| `db_collection_name` | `string` | ✓ |

</details>

<details><summary>span: `votes_add`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `votes_add`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_collection_name` | `string` | ✓ |

</details>

## target: `amaru::stores::rocksdb`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `db_collection_name` | `string` | ✓ |

</details>
| `commit` | `TRACE` | public | Commit a write transaction | db_system_name, db_operation_name |  |
| `rollback` | `TRACE` | public | Rollback a write transaction | db_system_name, db_operation_name |  |

## target: `amaru::stores::rocksdb`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `save_point` | `TRACE` | public | Save point to RocksDB store | slot, db_system_name, db_operation_name | epoch, db_operation_batch_size |
| `validate_snapshots` | `TRACE` | public | Validate sufficient snapshots exist | db_system_name, db_operation_name | snapshot_count, continuous_ranges |
| `commit` | `TRACE` | public | Commit a write transaction | db_system_name, db_operation_name |  |
| `rollback` | `TRACE` | public | Rollback a write transaction | db_system_name, db_operation_name |  |
| `save_point` | `TRACE` | public | Save point to RocksDB store | slot, db_system_name, db_operation_name | epoch, db_operation_batch_size |
| `validate_snapshots` | `TRACE` | public | Validate sufficient snapshots exist | db_system_name, db_operation_name | snapshot_count, continuous_ranges |

<details><summary>span: `commit`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `commit`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |

</details>
| `db_operation_name` | `string` | ✓ |

</details>

<details><summary>span: `rollback`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `rollback`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |

</details>
| `db_operation_name` | `string` | ✓ |

</details>

<details><summary>span: `save_point`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `save_point`</summary>

| field | type | required |
| --- | --- | --- |
| `slot` | `integer` | ✓ |
| `slot` | `integer` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `epoch` | `integer` |  |
| `epoch` | `integer` |  |
| `db_operation_batch_size` | `integer` |  |

</details>
| `db_operation_batch_size` | `integer` |  |

</details>

<details><summary>span: `validate_snapshots`</summary>

| field | type | required |
| --- | --- | --- |

<details><summary>span: `validate_snapshots`</summary>

| field | type | required |
| --- | --- | --- |
| `db_system_name` | `string` | ✓ |
| `db_system_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `db_operation_name` | `string` | ✓ |
| `snapshot_count` | `integer` |  |
| `snapshot_count` | `integer` |  |
| `continuous_ranges` | `integer` |  |

</details>

## Updating This Documentation

This file is auto-generated from the trace schema definitions in the code. To update it, run:

```bash
./scripts/generate-traces-doc
```

The schemas are defined using the `define_schemas!` macro in the codebase. Any changes to trace definitions will automatically be reflected in this documentation when the script is run.
| `continuous_ranges` | `integer` |  |

</details>

## Updating This Documentation

This file is auto-generated from the trace schema definitions in the code. To update it, run:

```bash
./scripts/generate-traces-doc
```

The schemas are defined using the `define_schemas!` macro in the codebase. Any changes to trace definitions will automatically be reflected in this documentation when the script is run.
