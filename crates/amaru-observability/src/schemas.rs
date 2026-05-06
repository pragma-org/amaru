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
//! This module defines schemas that can be used with the `trace_span!` macro to enable
//! compile-time validation of tracing fields. The schemas are organized by module
//! hierarchy matching the crate structure.
//!

use amaru_observability_macros::define_schemas;

pub const OPENING_CHAIN_DB: &str = "opening chain db";
pub const MIGRATING_DATABASE: &str = "migrating database";

// Certificate validation target
pub const CERTIFICATE_TARGET: &str = "amaru::ledger::context::default::validation";

define_schemas! {
    amaru {
        consensus {
            validate_header {

            /// Evolve the nonce based on header
            EVOLVE_NONCE {
                required hash: amaru_kernel::HeaderHash
            }

            /// Validate header cryptographic properties
            VALIDATE {
                required issuer_key: amaru_kernel::Bytes
            }
        }

        // Chain sync operations
        chain_sync {
            /// Decode header from raw bytes
            DECODE_HEADER {
                required peer: String
            }
        }
    }

    network {
        connection {
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

        chainsync_client {
            /// Find chain intersection point with peer
            FIND_INTERSECTION {
                required peer: String
                required intersection_slot: u64
            }
        }
    }

    ledger {
        state {
            /// Roll forward ledger state with a new block
            public ROLL_FORWARD {}

            /// Apply a block to stable state
            public APPLY_BLOCK {
                required point_slot: u64
            }

            /// Epoch transition processing
            public EPOCH_TRANSITION {
                required from: u64
                required into: u64
            }

            /// Resolve transaction inputs from various sources
            public RESOLVE_INPUTS {
                optional resolved_from_context: u64
                optional resolved_from_volatile: u64
                optional resolved_from_db: u64
            }

            /// Create validation context for a block
            public CREATE_VALIDATION_CONTEXT {
                required block_body_hash: amaru_kernel::HeaderHash
                required block_number: u64
                required block_body_size: u64
                optional total_inputs: u64
            }

            /// Compute stake distribution for epoch
            public COMPUTE_STAKE_DISTRIBUTION {
                required epoch: u64
            }

            /// Tick proposals for ratification
            public TICK_PROPOSALS {
                required proposals_count: u64
            }

            /// Prepare block for validation
            public PREPARE_BLOCK {}

            /// Validate block against rules
            public VALIDATE_BLOCK {}

            /// Tick pool operations
            public TICK_POOL {}

            /// Compute rewards for epoch
            public COMPUTE_REWARDS {}

            /// Forward ledger state with new volatile state
            public FORWARD {}

            /// Persist the oldest volatile block to stable storage once the security parameter is reached
            public VOLATILE_TO_STABLE {
                required persisted_point: String
                required volatile_len_before: u64
                required volatile_len_after: u64
                required k: u64
            }

            /// End epoch operations
            public END_EPOCH {}

            /// Begin epoch operations
            public BEGIN_EPOCH {}

            /// Reset fees to zero
            public RESET_FEES {}

            /// Reset blocks count to zero
            public RESET_BLOCKS_COUNT {}

            /// Roll backward to a specific point
            public ROLL_BACKWARD {
                required rollback_point: String
            }

            /// Create ratification context
            public RATIFICATION_CONTEXT_NEW {}

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

            default {
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
            }
        }

        governance {
            /// Ratify proposals at epoch boundary
            public RATIFY_PROPOSALS {
                optional roots_protocol_parameters: String
                optional roots_hard_fork: String
                optional roots_constitutional_committee: String
                optional roots_constitution: String
            }

            /// Ratify a proposal while traversing the governance forest
            RATIFYING {
                required proposal_id: String
                required proposal_kind: String
            }
        }
    }

    stores {
        ledger {
            /// Create ledger snapshot for epoch
            public SNAPSHOT {
                required epoch: u64
                required db_system_name: String
                required db_operation_name: String
            }

            /// Prune old snapshots
            public PRUNE {
                required functional_minimum: u64
                required db_system_name: String
                required db_operation_name: String
            }

            /// Epoch transition tracking
            public TRY_EPOCH_TRANSITION {
                optional has_from: bool
                optional has_to: bool
                optional point: String
                optional snapshots: String
                required db_system_name: String
                required db_operation_name: String
            }

            /// Remove DRep delegations
            public DREPS_DELEGATION_REMOVE {
                required drep_hash: amaru_kernel::Hash<28>
                required drep_type: amaru_kernel::StakeCredentialKind
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
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

                /// Clear DRep delegation for accounts (protocol v9 bug compat)
                public ACCOUNTS_RESET_DELEGATION {
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

                /// Remove enacted or expired proposals
                public PROPOSALS_REMOVE {
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
            /// Save point to RocksDB store
            public SAVE_POINT {
                required slot: u64
                optional epoch: u64
                required db_system_name: String
                required db_operation_name: String
                optional db_operation_batch_size: u64
            }

            /// Validate sufficient snapshots exist
            public VALIDATE_SNAPSHOTS {
                optional snapshot_count: u64
                optional continuous_ranges: u64
                required db_system_name: String
                required db_operation_name: String
            }

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

        consensus {
            /// Store a block header
            public STORE_HEADER {
                required hash: amaru_kernel::HeaderHash
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Store a raw block
            public STORE_BLOCK {
                required hash: amaru_kernel::HeaderHash
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Roll forward the chain to a point
            public ROLL_FORWARD_CHAIN {
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

    protocols {
        connection {
            /// Handle connection stage messages
            CONNECTION_STAGE {
                required message_type: String
                required conn_id: String
                required peer: String
                required role: String
            }
        }

        manager {
            /// Handle manager stage messages
            public MANAGER_STAGE {
                required message_type: String
            }

            /// A new peer was added to the manager
            public ADD_PEER {
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
            public REMOVE_PEER {
                required peer: String
            }

            /// A peer connection has died
            public CONNECTION_DIED {
                required peer: String
                required conn_id: String
                required role: String
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

    stage {
        tokio {
            /// Poll stage operation
            POLL {
                required stage: Name
            }
        }
    }
    }
}
