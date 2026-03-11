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
//! This module defines schemas that can be used with the `#[trace]` macro to enable
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
            diffusion {
                /// Fetch a block from the network
                FETCH_BLOCK {}

                /// Forward chain operations
                FORWARD_CHAIN {}
        }

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
            /// Chain sync pull operation
            PULL {}

            /// Decode header from raw bytes
            DECODE_HEADER {}

            /// Pull chain updates from peer
            RECEIVE_HEADER {}

            /// Header decode failed from received data
            RECEIVE_HEADER_DECODE_FAILED {}

            /// Select best chain from available headers
            SELECT_CHAIN {}

            /// Validate block properties
            VALIDATE_BLOCK {}

            /// Validate header properties
            VALIDATE_HEADER {}
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
            ROLL_FORWARD {}

            /// Apply a block to stable state
            APPLY_BLOCK {
                required point_slot: u64
            }

            /// Epoch transition processing
            EPOCH_TRANSITION {
                required from: u64
                required into: u64
            }

            /// Resolve transaction inputs from various sources
            RESOLVE_INPUTS {
                optional resolved_from_context: u64
                optional resolved_from_volatile: u64
                optional resolved_from_db: u64
            }

            /// Create validation context for a block
            CREATE_VALIDATION_CONTEXT {
                required block_body_hash: amaru_kernel::HeaderHash
                required block_number: u64
                required block_body_size: u64
                optional total_inputs: u64
            }

            /// Compute stake distribution for epoch
            COMPUTE_STAKE_DISTRIBUTION {
                required epoch: u64
            }

            /// Tick proposals for ratification
            TICK_PROPOSALS {
                required proposals_count: u64
            }

            /// Prepare block for validation
            PREPARE_BLOCK {}

            /// Validate block against rules
            VALIDATE_BLOCK {}

            /// Tick pool operations
            TICK_POOL {}

            /// Compute rewards for epoch
            COMPUTE_REWARDS {}

            /// Forward ledger state with new volatile state
            FORWARD {}

            /// End epoch operations
            END_EPOCH {}

            /// Begin epoch operations
            BEGIN_EPOCH {}

            /// Compute stake distribution for epoch
            COMPUTE_STAKE_DISTRIBUTION_NAMED {}

            /// Reset fees to zero
            RESET_FEES {}

            /// Reset blocks count to zero
            RESET_BLOCKS_COUNT {}

            /// Roll backward to a specific point
            ROLL_BACKWARD {}

            /// Create ratification context
            RATIFICATION_CONTEXT_NEW {}

            /// Manage transaction outputs
            MANAGE_TRANSACTION_OUTPUTS {}

            /// Cleanup old epochs
            CLEANUP_OLD_EPOCHS {}

            /// Cleanup expired proposals
            CLEANUP_EXPIRED_PROPOSALS {}
        }

        rules {
            /// Parse raw block bytes
            PARSE_BLOCK {
                required block_size: u64
            }
        }

        context {
            /// Add transaction fees to pots
            ADD_FEES {
                required fee: amaru_kernel::Lovelace
            }

            /// Withdraw from stake credential
            WITHDRAW_FROM {
                required credential_type: amaru_kernel::StakeCredentialKind
                required credential_hash: amaru_kernel::Hash<28>
            }

            /// Record a governance vote
            VOTE {
                required voter_type: amaru_kernel::VoterKind
                required credential_type: amaru_kernel::StakeCredentialKind
                required credential_hash: amaru_kernel::Hash<28>
            }

            /// Require a verification key witness
            REQUIRE_VKEY_WITNESS {
                required hash: String
            }

            /// Require a script witness
            REQUIRE_SCRIPT_WITNESS {
                required hash: String
            }

            /// Require a bootstrap witness
            REQUIRE_BOOTSTRAP_WITNESS {
                required bootstrap_witness_hash: String
            }

            default {
                validation {
                    /// Register a stake credential
                    CERTIFICATE_STAKE_REGISTRATION {
                        required credential_type: amaru_kernel::StakeCredentialKind
                        required credential_hash: amaru_kernel::Hash<28>
                    }

                    /// Delegate stake to a pool
                    CERTIFICATE_STAKE_DELEGATION {
                        required credential_type: amaru_kernel::StakeCredentialKind
                        required credential_hash: amaru_kernel::Hash<28>
                        required pool_id: String
                    }

                    /// Unregister a stake credential
                    CERTIFICATE_STAKE_DEREGISTRATION {
                        required credential_type: amaru_kernel::StakeCredentialKind
                        required credential_hash: amaru_kernel::Hash<28>
                    }

                    /// Register a DRep
                    CERTIFICATE_DREP_REGISTRATION {
                        required drep_type: amaru_kernel::StakeCredentialKind
                        required drep_hash: amaru_kernel::Hash<28>
                        required deposit: u64
                    }

                    /// Update DRep anchor
                    CERTIFICATE_DREP_UPDATE {
                        required drep_type: amaru_kernel::StakeCredentialKind
                        required drep_hash: amaru_kernel::Hash<28>
                    }

                    /// Unregister a DRep
                    CERTIFICATE_DREP_RETIREMENT {
                        required drep_type: amaru_kernel::StakeCredentialKind
                        required drep_hash: amaru_kernel::Hash<28>
                        required refund: u64
                    }

                    /// Delegate vote to DRep
                    CERTIFICATE_VOTE_DELEGATION {
                        required credential_type: amaru_kernel::StakeCredentialKind
                        required credential_hash: amaru_kernel::Hash<28>
                        required drep_type: amaru_kernel::StakeCredentialKind
                        required drep_hash: amaru_kernel::Hash<28>
                    }

                    /// Register a pool
                    CERTIFICATE_POOL_REGISTRATION {
                        required pool_id: String
                    }

                    /// Retire a pool
                    CERTIFICATE_POOL_RETIREMENT {
                        required pool_id: String
                        required epoch: u64
                    }

                    /// Delegate cold key to committee
                    CERTIFICATE_COMMITTEE_DELEGATE {
                        required cc_member_type: amaru_kernel::StakeCredentialKind
                        required cc_member_hash: amaru_kernel::Hash<28>
                        required delegate_type: amaru_kernel::StakeCredentialKind
                        required delegate_hash: amaru_kernel::Hash<28>
                    }

                    /// Resign from committee
                    CERTIFICATE_COMMITTEE_RESIGN {
                        required cc_member_type: amaru_kernel::StakeCredentialKind
                        required cc_member_hash: amaru_kernel::Hash<28>
                    }
                }
            }
        }

        governance {
            /// Ratify proposals at epoch boundary
            RATIFY_PROPOSALS {
                optional roots_protocol_parameters: String
                optional roots_hard_fork: String
                optional roots_constitutional_committee: String
                optional roots_constitution: String
            }
        }
    }

    stores {
        ledger {
            /// Create ledger snapshot for epoch
            SNAPSHOT {
                required epoch: u64
                required db_system_name: String
                required db_operation_name: String
            }

            /// Prune old snapshots
            PRUNE {
                required functional_minimum: u64
                required db_system_name: String
                required db_operation_name: String
            }

            /// Epoch transition tracking
            TRY_EPOCH_TRANSITION {
                optional has_from: bool
                optional has_to: bool
                optional point: String
                optional snapshots: String
                required db_system_name: String
                required db_operation_name: String
            }

            /// Remove DRep delegations
            DREPS_DELEGATION_REMOVE {
                required drep_hash: amaru_kernel::Hash<28>
                required drep_type: amaru_kernel::StakeCredentialKind
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            columns {
                /// Point-read a UTxO entry
                UTXO_GET {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Batch-insert UTxO entries
                UTXO_ADD {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Batch-delete UTxO entries
                UTXO_REMOVE {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Point-read a pool entry
                POOLS_GET {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Batch-upsert pool entries
                POOLS_ADD {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Schedule pool retirement
                POOLS_REMOVE {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Point-read an account entry
                ACCOUNTS_GET {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Batch-upsert account entries
                ACCOUNTS_ADD {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Batch-delete account entries
                ACCOUNTS_REMOVE {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Update rewards balance for a single account
                ACCOUNTS_SET {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Reset rewards counters for many accounts
                ACCOUNTS_RESET_MANY {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Clear DRep delegation for accounts (protocol v9 bug compat)
                ACCOUNTS_RESET_DELEGATION {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Point-read a DRep entry
                DREPS_GET {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Batch-upsert DRep registrations
                DREPS_ADD {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Record DRep de-registration
                DREPS_REMOVE {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Refresh DRep expiry after a vote
                DREPS_SET_VALID_UNTIL {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Upsert a constitutional committee member
                CC_MEMBERS_UPSERT {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Insert governance proposals
                PROPOSALS_ADD {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Remove enacted or expired proposals
                PROPOSALS_REMOVE {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Record governance votes
                VOTES_ADD {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Point-read a slot/block-issuer entry
                SLOTS_GET {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Write a slot/block-issuer entry
                SLOTS_PUT {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Read treasury/reserve/fees pots
                POTS_GET {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Write treasury/reserve/fees pots
                POTS_PUT {
                    required db_system_name: String
                    required db_operation_name: String
                    required db_collection_name: String
                }

                /// Full-table scan via IterBorrow (tick/epoch operations)
                ITER_SCAN {
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
            SAVE_POINT {
                required slot: u64
                optional epoch: u64
                required db_system_name: String
                required db_operation_name: String
                optional db_operation_batch_size: u64
            }

            /// Validate sufficient snapshots exist
            VALIDATE_SNAPSHOTS {
                optional snapshot_count: u64
                optional continuous_ranges: u64
                required db_system_name: String
                required db_operation_name: String
            }

            /// Commit a write transaction
            COMMIT {
                required db_system_name: String
                required db_operation_name: String
            }

            /// Rollback a write transaction
            ROLLBACK {
                required db_system_name: String
                required db_operation_name: String
            }
        }

        consensus {
            /// Store a block header
            STORE_HEADER {
                required hash: amaru_kernel::HeaderHash
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Store a raw block
            STORE_BLOCK {
                required hash: amaru_kernel::HeaderHash
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Roll forward the chain to a point
            ROLL_FORWARD_CHAIN {
                required hash: amaru_kernel::HeaderHash
                required slot: u64
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Rollback the chain to a point
            ROLLBACK_CHAIN {
                required hash: amaru_kernel::HeaderHash
                required slot: u64
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Store block to tip operations
            STORE_BLOCK_TO_TIP {
                required hash: String
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Rollback to tip operations
            ROLLBACK_TO_TIP {
                required hash: String
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Read headers operations
            READ_HEADERS {
                required hash: String
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }

            /// Read blocks operations
            READ_BLOCKS {
                required hash: String
                required db_system_name: String
                required db_operation_name: String
                required db_collection_name: String
            }
        }
    }

    protocols {
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

            /// Demultiplex incoming bytes
            DEMUX {
                required proto_id: u16
                required bytes: u64
            }

            /// Multiplex outgoing bytes
            MUX {
                required bytes: u64
            }
        }
    }

    simulator {
        node {
            /// Handle message in simulator node
            HANDLE_MSG {}
        }
    }

    stage {
        tokio {
            /// Poll stage operation
            POLL {
                required stage: Name
            }
        }

        logging {
            /// Test span for logging
            TEST_SPAN {}
        }
    }
    }
}
