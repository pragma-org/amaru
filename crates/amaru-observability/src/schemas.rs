// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Tracing schemas for compile-time validation of observability instrumentation.
//!
//! This module defines schemas that can be used with the `debug_span!` macro to enable
//! compile-time validation of tracing fields. The schemas are organized by module
//! hierarchy matching the crate structure.
//!

use amaru_observability_macros::define_schemas;

define_schemas! {
    amaru {
        consensus {
            chain_db {
                tags: setup
                /// Open the database
                OPEN {
                    required path: String
                }
                /// Migrate the database if necessary
                MIGRATE {
                    required from: u16
                    required to: u16
                }
            }
            blocks {
                /// Validate downloaded blocks that are not yet validated
                RECOVER_STORED {
                    tags: setup
                    required best_hash: amaru_kernel::HeaderHash
                }
                /// Fetch a range of blocks starting from the specified tip
                FETCH {
                    tags: cpu
                    required tip: amaru_kernel::Tip
                    required header_hash: amaru_kernel::HeaderHash
                }
            }
            node {
                tags: setup
                /// Initialize the node
                INITIALIZE {}
            }
            chain {
                /// Find chain intersection point with peer
                FIND_INTERSECTION {
                    tags: bootstrap
                    required peer: String
                    required intersection_slot: u64
                }
                /// Received a new tip from an upstream peer
                SELECT_FROM_TIP {
                    tags: cpu
                    required tip: amaru_kernel::Tip
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Received a block validation result
                SELECT_FROM_BLOCK_VALIDATION {
                    tags: cpu
                    required point: amaru_kernel::Tip
                    required valid: bool
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Some blocks have been fetched for the current chain, decide what to do next
                FETCH_NEXT {
                    tags: cpu
                    required point: amaru_kernel::Point
                    required header_hash: amaru_kernel::HeaderHash
                }
            }
            roll_forward {
                tags: cpu
                /// Received a new tip to roll forward
                PROCESS {
                    required tip: amaru_kernel::Tip
                    required peer: amaru_kernel::Peer
                    optional header_hash: amaru_kernel::HeaderHash
                }
            }
            rollback {
                tags: cpu
                /// Received a header to rollback
                PROCESS {
                    required current: amaru_kernel::Point
                    required tip: amaru_kernel::Tip
                    required peer: amaru_kernel::Peer
                    required header_hash: amaru_kernel::HeaderHash
                }
            }
            header {
                tags: cpu
                /// Decode header from raw bytes
                DECODE {
                    required peer: String
                }
                /// Validate the whole header
                VALIDATE {
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Evolve the nonce based on header
                EVOLVE_NONCE {
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Check header cryptographic properties
                CHECK {
                    required issuer_key: amaru_kernel::Bytes
                }
                /// Forward to a downstream peer
                FORWARD {
                    required tip: amaru_kernel::Tip
                    required header_hash: amaru_kernel::HeaderHash
                    required peer: amaru_kernel::Peer
                }
            }
            block {
                tags: cpu
                /// Validate a block by applying it to the current ledger
                VALIDATE {
                    required tip: amaru_kernel::Tip
                    required header_hash: amaru_kernel::HeaderHash
                    optional valid: bool
                }
                /// Adopt a block as the next block in the best chain
                ADOPT {
                    required tip: amaru_kernel::Tip
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Mismatched body hash after download, the peer is adversarial
                MISMATCHED_HASH {
                    required peer: amaru_kernel::Peer
                    required header_hash: amaru_kernel::HeaderHash
                }
            }
            peer {
                tags: cpu
                /// A peer behaves like an adversary, ban it
                BAN {
                    required peer: amaru_kernel::Peer
                }
            }
        }
        ledger {
            tags: cpu
            block {
                /// Apply a block to stable state
                public APPLY {
                    required point_slot: u64
                }
                /// Create validation context for a block
                public CREATE_VALIDATION_CONTEXT {
                    required block_body_hash: amaru_kernel::HeaderHash
                    required block_number: u64
                    required block_body_size: u64
                    optional total_inputs: u64
                }
                /// Prepare block for validation
                public PREPARE {}

                /// Validate block against rules
                public VALIDATE {}
            }
            transaction {
                /// Create validation context for a transaction
                public CREATE_VALIDATION_CONTEXT {
                    required transaction_id: amaru_kernel::TransactionId
                }
            }
            inputs {
                /// Resolve transaction inputs from the volatile db or the stable one
                public HYDRATE {
                    optional from_volatile: u64
                    optional from_db: u64
                }
            }
            pools {
                /// Resolve pools from the volatile db or the stable one
                public HYDRATE {
                    optional from_volatile: u64
                    optional from_db: u64
                }
            }
            accounts {
                /// Resolve accounts from the volatile db or the stable one
                public HYDRATE {
                    optional from_volatile: u64
                    optional from_db: u64
                }
            }
            dreps {
                /// Resolve dreps from the volatile db or the stable one
                public HYDRATE {
                    optional from_volatile: u64
                    optional from_db: u64
                }
            }
            committee {
                /// Resolve committee members from the volatile db or the stable one
                public HYDRATE {
                    optional from_volatile: u64
                    optional from_db: u64
                }
            }
            proposals {
                /// Resolve proposals from the volatile db or the stable one
                public HYDRATE {
                    optional from_volatile: u64
                    optional from_db: u64
                }
            }
            epoch {
                /// Compute stake distribution for epoch
                public COMPUTE_STAKE_DISTRIBUTION {
                    required epoch: u64
                }
                /// Compute rewards for epoch
                public COMPUTE_REWARDS {
                    required for_epoch: u64
                    optional using_stake_distribution_epoch_from: u64
                }
            }
            epoch_transition {
                /// Epoch transition processing
                public EPOCH_TRANSITION {
                    required from: u64
                    required into: u64
                    optional skipped: bool
                    optional resuming_from: String
                }
                /// Perform end-of-epoch epoch boundary computations
                public END_EPOCH {}
                /// Perform start-of-epoch epoch boundary computations
                public BEGIN_EPOCH {}
                /// Flushing the epoch transition overlay to disk
                public APPLYING_OVERLAY {
                    /// Epoch for which this overlay is being flush; This is the *currently active*
                    /// epoch.
                    required epoch: u64
                    /// Whether to end the epoch; in case Amaru is restarting mid-update.
                    optional should_end_epoch: bool,
                    /// Whether to take an on-disk snapshot; in case Amaru is restarting mid-update.
                    optional should_snapshot: bool,
                    /// Whether to begin the epoch; in case Amaru is restarting mid-update.
                    optional should_begin_epoch: bool,
                }
                /// Create pools updates
                public NEW_POOLS_UPDATES {}
                /// Create governance updates (i.e. ratify proposals) at an epoch boundary.
                public NEW_GOVERNANCE_UPDATES {
                    /// Total number of proposals in scope. This also includes proposals that have
                    /// *just* been submitted.
                    required proposals_count: u64
                }
                /// Reset fees to zero
                public RESET_FEES {}
                /// Reset blocks count to zero
                public RESET_BLOCKS_COUNT {}
                /// Pay rewards to all accounts before the epoch end
                public PAY_REWARDS {
                    /// Total number of accounts that received non-zero rewards
                    optional accounts_paid: u64
                    /// Total rewards effectively paid to ALL accounts; does not include unassignable rewards
                    optional rewards_paid: u64
                    /// Treasury increase; corresponding to both the treasury tax and the unpaid rewards
                    optional treasury_delta: u64
                    /// Reserves depletion from incentives; always negative.
                    optional reserves_delta: i64
                }
                /// Pruned proposals at an epoch boundary, recorded to facilitate future stake
                /// distribution calculations.
                public RECORD_PRUNED_PROPOSALS {}
                /// Pay withdrawals to accounts, or refund deposits
                public PAY_OR_REFUND_ACCOUNTS {
                    /// Total quantity of ADA paid, excluding treasury leftovers
                    optional total_paid_or_refunded: u64
                    /// Total amounts that couldn't be paid to accounts, going back to treasury instead.
                    optional treasury_leftovers: u64
                }
                /// Updating pools metadata or retiring pools at an epoch boundary.
                public UPDATE_OR_RETIRE_POOLS {
                    /// Total number of pools updating metadata
                    required pools_updated: u64
                    /// Total number of pools retired
                    required pools_retired: u64
                }
                /// Enact all governance updates and flush their outcome to disk
                public APPLY_GOVERNANCE_UPDATES {}
                /// Add or remove CC members; or switch to a no-confidence state
                public UPDATE_CONSTITUTIONAL_COMMITTEE {
                    /// Whether or not updates switches the committee to a "no-confidence" state
                    required no_confidence: bool
                }
            }
            context {
                /// Add transaction fees to pots
                public ADD_FEES {
                    required fee: amaru_kernel::Lovelace
                }
                /// Withdraw from stake credential
                public WITHDRAW_FROM {
                    required credential_type: amaru_kernel::StakeCredentialKind
                    required credential_hash: amaru_kernel::Hash<28>
                }
                /// Record a governance vote
                public VOTE {
                    required voter_type: amaru_kernel::VoterKind
                    required credential_type: amaru_kernel::StakeCredentialKind
                    required credential_hash: amaru_kernel::Hash<28>
                }
                /// Require a verification key witness
                public REQUIRE_VKEY_WITNESS {
                    required hash: String
                }
                /// Require a script witness
                public REQUIRE_SCRIPT_WITNESS {
                    required hash: String
                }
                /// Require a bootstrap witness
                public REQUIRE_BOOTSTRAP_WITNESS {
                    required bootstrap_witness_hash: String
                }
            }
            validation {
                /// Register a stake credential
                public CERTIFICATE_STAKE_REGISTRATION {
                    required credential: String
                }
                /// Delegate stake to a pool
                public CERTIFICATE_STAKE_DELEGATION {
                    required credential: String
                    required pool_id: amaru_kernel::PoolId
                }
                /// Unregister a stake credential
                public CERTIFICATE_STAKE_DEREGISTRATION {
                    required credential: String
                }
                /// Register a DRep
                public CERTIFICATE_DREP_REGISTRATION {
                    required drep: String
                    required deposit: u64
                    optional anchor_url: String
                }
                /// Update DRep anchor
                public CERTIFICATE_DREP_UPDATE {
                    required drep: String
                    optional anchor_url: String
                }
                /// Unregister a DRep
                public CERTIFICATE_DREP_RETIREMENT {
                    required drep: String
                    required refund: u64
                }
                /// Delegate vote to DRep
                public CERTIFICATE_VOTE_DELEGATION {
                    required credential: String
                    optional drep: String
                }
                /// Register a pool
                public CERTIFICATE_POOL_REGISTRATION {
                    required pool_id: amaru_kernel::PoolId
                }
                /// Retire a pool
                public CERTIFICATE_POOL_RETIREMENT {
                    required pool_id: amaru_kernel::PoolId
                    required epoch: u64
                }
                /// Delegate cold key to committee
                public CERTIFICATE_COMMITTEE_DELEGATE {
                    required cc_member: String
                    required delegate: String
                }
                /// Resign from committee
                public CERTIFICATE_COMMITTEE_RESIGN {
                    required cc_member: String
                    optional anchor_url: String
                }
            }
            governance {
                /// Create ratification context
                public NEW_RATIFICATION_CONTEXT {
                    /// Epoch to ratify; distinct from the actual epoch this calculation is happening.
                    required ratifying_epoch: u64
                    /// Value of the treasury considered for this ratification round.
                    optional treasury: u64
                    /// Total number of votes to ratify.
                    optional votes: u64
                }
                /// Ratify proposals at epoch boundary
                public RATIFY_PROPOSALS {
                    required epoch: u64
                    optional roots_protocol_parameters: String
                    optional roots_hard_fork: String
                    optional roots_constitutional_committee: String
                    optional roots_constitution: String
                }
                /// Ratify a proposal while traversing the governance forest
                public RATIFYING {
                    required proposal_id: String
                    required proposal_kind: String
                    optional approved_by_constitutional_committee: bool
                    optional committee_approval_threshold: String
                    optional approved_by_pools: bool
                    optional pools_approval_threshold: String
                    optional approved_by_dreps: bool
                    optional dreps_approval_threshold: String
                }
                /// Computing enactment of a ratified proposal
                public ENACTING {
                    required proposal_id: String
                    required proposal_kind: String
                    optional pruned_relatives: String
                }
            }
            ledger_state {
                /// Roll forward with a new block
                public ROLL_FORWARD {}
                /// Forward ledger state with new volatile state
                public PUSH {}
                /// Roll backward to a specific point
                public ROLL_BACKWARD {
                    required rollback_point: String
                }
            }
            volatile {
                /// Recompute the volatile aggregate
                public AGGREGATE {}
            }
        }
        stores {
            tags: db
            ledger {
                epoch {
                    /// Create ledger snapshot for epoch
                    public CREATE_SNAPSHOT {
                        required epoch: u64
                        required db_system_name: String
                        required db_operation_name: String
                    }
                    /// Prune old snapshots
                    public PRUNE_OLD_SNAPSHOTS {
                        required functional_minimum: u64
                        required desired_minimum: u64
                        required db_system_name: String
                        required db_operation_name: String
                    }
                    /// Epoch transition tracking
                    public TRY_TRANSITION {
                        required from: String
                        required to: String
                        required db_system_name: String
                        required db_operation_name: String
                    }
                }
                columns {
                    /// Point-read a UTxO entry
                    public UTXO_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Batch-insert UTxO entries
                    public UTXO_ADD {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Batch-delete UTxO entries
                    public UTXO_REMOVE {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Point-read a pool entry
                    public POOLS_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Batch-upsert pool entries
                    public POOLS_ADD {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Schedule pool retirement
                    public POOLS_REMOVE {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Point-read an account entry
                    public ACCOUNTS_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Batch-upsert account entries
                    public ACCOUNTS_ADD {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Batch-delete account entries
                    public ACCOUNTS_REMOVE {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Update rewards balance for a single account
                    public ACCOUNTS_SET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Reset rewards counters for many accounts
                    public ACCOUNTS_RESET_MANY {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Point-read a DRep entry
                    public DREPS_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Batch-upsert DRep registrations
                    public DREPS_ADD {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Record DRep de-registration
                    public DREPS_REMOVE {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Refresh DRep expiry after a vote
                    public DREPS_SET_VALID_UNTIL {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Read a constitutional committee member
                    public CC_MEMBERS_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Upsert a constitutional committee member
                    public CC_MEMBERS_UPSERT {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Insert governance proposals
                    public PROPOSALS_ADD {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Read governance proposals
                    public PROPOSALS_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Remove enacted or expired proposals
                    public PROPOSALS_REMOVE {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Inserting recently pruned proposals
                    public RECENTLY_PRUNED_PROPOSALS_REPLACE_ALL {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Record governance votes
                    public VOTES_ADD {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Point-read a slot/block-issuer entry
                    public SLOTS_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Write a slot/block-issuer entry
                    public SLOTS_PUT {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Read treasury/reserve/fees pots
                    public POTS_GET {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Write treasury/reserve/fees pots
                    public POTS_PUT {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Full-table scan via IterBorrow (tick/epoch operations)
                    public ITER_SCAN {
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                        optional rows_scanned: u64
                        optional rows_written: u64
                        optional rows_deleted: u64
                    }
                }
            }
            rocksdb {
                point {
                    /// Save point to RocksDB store
                    public SAVE {
                        required slot: u64
                        optional epoch: u64
                        required db_system_name: String
                        required db_operation_name: String
                        optional db_operation_batch_size: u64
                    }
                }
                snapshots {
                    /// Validate sufficient snapshots exist
                    public VALIDATE {
                        optional snapshot_count: u64
                        optional continuous_ranges: u64
                        required db_system_name: String
                        required db_operation_name: String
                    }
                }
                transaction {
                    /// Commit a write transaction
                    public COMMIT {
                        required db_system_name: String
                        required db_operation_name: String
                    }
                    /// Rollback a write transaction
                    public ROLLBACK {
                        required db_system_name: String
                        required db_operation_name: String
                    }
                }
            }
            consensus {
                header {
                    /// Store a block header
                    public STORE {
                        required hash: amaru_kernel::HeaderHash
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                }
                block {
                    /// Store a raw block
                    public STORE {
                        required hash: amaru_kernel::HeaderHash
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                }
                chain {
                    /// Roll forward the chain to a point
                    public ROLL_FORWARD {
                        required hash: amaru_kernel::HeaderHash
                        required slot: u64
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                    /// Switch the chain to a new fork
                    public SWITCH_TO_FORK {
                        required hash: amaru_kernel::HeaderHash
                        required slot: u64
                        required db_system_name: String
                        required db_operation_name: String
                        required db_collection_name: String
                    }
                }
            }
        }
        mempool {
            transaction {
                /// Transaction received by the mempool stage, before validation.
                public RECEIVED {
                    required tx_id: String
                    required origin: String
                }
                /// Transaction validated and inserted into the mempool.
                public ACCEPTED {
                    required tx_id: String
                    required seq_no: u64
                    required origin: String
                }
                /// Transaction rejected at insertion. Reason ∈ {invalid, duplicate, mempool_full}.
                public REJECTED {
                    required tx_id: String
                    required reason: String
                    optional validation_error: String
                }
                /// Transaction removed from the mempool. Reason ∈ {invalid_after_tip}.
                /// TODO: split the reason into invalid after tip + present in applied block
                public EVICTED {
                    required tx_id: String
                    required reason: String
                }
                /// Detail trace carrying upstream peer attribution for a received tx.
                RECEIVED_DETAIL {
                    required tx_id: String
                    required peer: String
                }
                /// Detail trace for a tip-driven revalidation pass.
                REVALIDATION_DETAIL {
                    required tip_slot: u64
                    required total_before: u64
                    required evicted_count: u64
                    required duration_micros: u64
                }
            }
        }
        protocols {
            connection {
                message {
                    /// Handle connection stage messages
                    PROCESS {
                        required message_type: String
                        required conn_id: String
                        required peer: String
                        required role: String
                    }
                }
            }
            manager {
                message {
                    /// Handle manager stage messages
                    public PROCESS {
                        required message_type: String
                    }
                }
                peer {
                    /// A new peer was added to the manager
                    public ADD {
                        required peer: String
                    }
                    /// Initiating an outbound connection to a peer
                    public CONNECT {
                        required peer: String
                    }
                    /// An inbound connection was accepted from a peer
                    public ACCEPTED {
                        required peer: String
                        required conn_id: String
                    }
                    /// A peer was removed from the manager
                    public REMOVE {
                        required peer: String
                    }
                    /// A peer connection has died
                    public CONNECTION_DIED {
                        required peer: String
                        required conn_id: String
                        required role: String
                    }
                }
            }
            peer_selection {
                peer {
                    /// A connection has been established and the handshake completed successfully.
                    public CONNECTED {
                        required peer: String
                        required conn_id: u64
                        required direction: String
                        required full_duplex_capable: bool
                        required full_duplex: bool
                    }
                    /// A connection has been terminated (graceful disconnect, error, handshake refusal,
                    /// or network error).
                    public DISCONNECTED {
                        required peer: String
                        required conn_id: u64
                        required direction: String
                        optional reason: String
                    }
                }
            }
            chainsync {
                initiator {
                    /// Handle chain sync initiator stage messages
                    CHAINSYNC_INITIATOR_STAGE {
                        required message_type: String
                    }
                    /// Handle chain sync initiator protocol messages
                    CHAINSYNC_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                }
                responder {
                    /// Handle chain sync responder stage messages
                    CHAINSYNC_RESPONDER_STAGE {
                        required message_type: String
                    }
                    /// Handle chain sync responder protocol messages
                    CHAINSYNC_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            blockfetch {
                initiator {
                    /// Handle block fetch initiator stage messages
                    BLOCKFETCH_INITIATOR_STAGE {
                        required message_type: String
                    }
                    /// Handle block fetch initiator protocol messages
                    BLOCKFETCH_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                }
                responder {
                    /// Handle block fetch responder stage messages
                    BLOCKFETCH_RESPONDER_STAGE {
                        required message_type: String
                    }
                    /// Handle block fetch responder protocol messages
                    BLOCKFETCH_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            handshake {
                initiator {
                    /// Handle handshake initiator stage messages
                    HANDSHAKE_INITIATOR_STAGE {
                        required message_type: String
                    }
                    /// Handle handshake initiator protocol messages
                    HANDSHAKE_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                }
                responder {
                    /// Handle handshake responder stage messages
                    HANDSHAKE_RESPONDER_STAGE {
                        required version_table: String
                    }
                    /// Handle handshake responder protocol messages
                    HANDSHAKE_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            keepalive {
                initiator {
                    /// Handle keepalive initiator stage messages
                    KEEPALIVE_INITIATOR_STAGE {
                        required cookie: u16
                    }
                    /// Handle keepalive initiator protocol messages
                    KEEPALIVE_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                }
                responder {
                    /// Handle keepalive responder stage messages
                    KEEPALIVE_RESPONDER_STAGE {
                        required cookie: u16
                    }
                    /// Handle keepalive responder protocol messages
                    KEEPALIVE_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            tx_submission {
                initiator {
                    /// Handle tx-submission initiator stage messages
                    TX_SUBMISSION_INITIATOR_STAGE {
                        required message_type: String
                    }
                    /// Handle tx-submission initiator protocol messages
                    TX_SUBMISSION_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                }
                responder {
                    /// Handle tx-submission responder stage messages
                    TX_SUBMISSION_RESPONDER_STAGE {
                        required message_type: String
                    }
                    /// Handle tx-submission responder protocol messages
                    TX_SUBMISSION_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            mux {
                protocol {
                    /// Register protocol with muxer
                    REGISTER {}
                    /// Buffer protocol messages
                    BUFFER {}
                    /// Handle outgoing protocol messages
                    OUTGOING {
                        optional proto_id: String
                        optional bytes: u64
                    }
                    /// Get next segment to send
                    NEXT_SEGMENT {}
                    /// Handle received protocol data
                    RECEIVED {
                        optional bytes: u64
                    }
                    /// Want next message for protocol
                    WANT_NEXT {}
                }
            }
        }
        network {
            connection {
                tags: io
                /// Accept loop for incoming connections
                ACCEPT_LOOP {}
                /// Listen on address
                LISTEN {}
                /// Accept a connection
                ACCEPT {}
                /// Connect to addresses
                CONNECT {}
                /// Connect to multiple addresses
                CONNECT_ADDRS {}
                /// Send data over connection
                SEND {}
                /// Receive data from connection
                RECV {}
                /// Close connection
                CLOSE {}
            }
        }
    }
}
