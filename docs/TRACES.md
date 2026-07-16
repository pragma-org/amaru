# Available Spans

This document lists all available spans in Amaru, auto-generated from the code.

For information on how to use and filter these spans, see [monitoring/README.md](../monitoring/README.md).


## target: `amaru::ledger::block`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `apply` | `TRACE` | public | Apply a block to stable state | point_slot |  |
| `prepare` | `TRACE` | public | Prepare block for validation |  |  |
| `validate` | `TRACE` | public | Validate block against rules |  |  |

<details><summary>span: `apply`</summary>

| field | type | required |
| --- | --- | --- |
| `point_slot` | `integer` | ✓ |

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

## target: `amaru::ledger::epoch_transition`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `apply` | `TRACE` | public | Flushing the epoch transition overlay to disk | epoch | should_end_epoch, should_snapshot, should_begin_epoch |
| `begin_epoch` | `TRACE` | public | Perform start-of-epoch epoch boundary computations |  |  |
| `compute` | `TRACE` | public | Epoch transition processing | from, into | skipped, resuming_from |
| `end_epoch` | `TRACE` | public | Perform end-of-epoch epoch boundary computations |  |  |
| `new_governance_updates` | `TRACE` | public | Create governance updates (i.e. ratify proposals) at an epoch boundary. | proposals_count |  |
| `new_pools_updates` | `TRACE` | public | Create pools updates |  |  |

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

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `for_epoch` | `integer` | ✓ |
| `using_stake_distribution_epoch_from` | `integer` |  |

</details>

## target: `amaru::ledger::stake_distribution`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `compute` | `TRACE` | public | Compute stake distribution for epoch | epoch |  |

<details><summary>span: `compute`</summary>

| field | type | required |
| --- | --- | --- |
| `epoch` | `integer` | ✓ |

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
| `epoch` | `integer` | ✓ |

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
| `reset_many` | `TRACE` | public | Reset rewards counters for many accounts |  |  |
| `set` | `TRACE` | public | Update rewards balance for a single account |  |  |

## target: `amaru::stores::ledger::cc_members`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `get` | `TRACE` | public | Read a constitutional committee member |  |  |
| `upsert` | `TRACE` | public | Upsert a constitutional committee member |  |  |

## target: `amaru::stores::ledger::dreps`

| name | level | public | description | required fields | optional fields |
| --- | --- | --- | --- | --- | --- |
| `add` | `TRACE` | public | Batch-upsert DRep registrations |  |  |
| `get` | `TRACE` | public | Point-read a DRep entry |  |  |
| `remove` | `TRACE` | public | Record DRep de-registration |  |  |
| `set_valid_until` | `TRACE` | public | Refresh DRep expiry after a vote |  |  |

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
| `remove` | `TRACE` | public | Schedule pool retirement |  |  |

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
