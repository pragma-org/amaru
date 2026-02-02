//! Tracing schemas for compile-time validation of observability instrumentation.
//!
//! This module defines schemas that can be used with the `#[trace]` macro to enable
//! compile-time validation of tracing fields. The schemas are organized by module
//! hierarchy matching the crate structure.
//!
//! ## Usage with `#[trace]` macro
//!
//! The `#[trace]` macro automatically generates a `#[tracing::instrument]` attribute with:
//! - `level = Level::TRACE` - all spans are at TRACE level
//! - `skip_all` - function parameters are not recorded by instrument directly
//! - `target = "module::submodule"` - target derived from schema path
//! - `fields(...)` - all schema fields (required and optional) pre-declared as Empty
//! - **Auto-recording**: `record()` calls are generated for all function parameters
//!
//! ```ignore
//! use amaru_observability::schemas::*;\n//!
//! // The macro validates required fields and auto-records parameter values
//! #[trace(amaru::ledger::state::FORWARD)]\n//! fn forward(point_slot: u64, point_hash: String) {
//!     // point_slot and point_hash are automatically recorded!
//!     // Optional fields can still be set manually:
//!     tracing::Span::current().record("extra_info", "some value");
//! }
//! ```
//!
//! The generated code is equivalent to:
//!
//! ```ignore
//! #[tracing::instrument(
//!     level = tracing::Level::TRACE,
//!     skip_all,
//!     target = "ledger::state",
//!     fields(point_slot = tracing::field::Empty, point_hash = tracing::field::Empty)
//! )]
//! fn forward(point_slot: u64, point_hash: String) {
//!     // Auto-generated record() calls:
//!     tracing::Span::current().record("point_slot", tracing::field::display(&point_slot));
//!     tracing::Span::current().record("point_hash", tracing::field::display(&point_hash));
//!     // ... rest of function body
//! }
//! ```

use amaru_observability_macros::define_schemas;

define_schemas! {
    // =========================================================================
    // Consensus Schemas
    // =========================================================================
    consensus {
        // Header validation spans
        validate_header {
            /// Evolve the nonce based on header (validate_header.rs)
            /// Name: "validate_header.evolve_nonce"
            EVOLVE_NONCE {
                required hash: String
            }

            /// Validate header cryptographic properties (validate_header.rs)
            /// Name: "validate_header.validate"
            VALIDATE {
                required issuer_key: String
            }
        }

        // Chain sync operations
        chain_sync {
            /// Chain sync pull operation (stages/pull.rs)
            /// Name: "diffusion.chain_sync"
            PULL {}

            /// Receive header from peer (stages/receive_header.rs)
            /// Note: Uses function name as span name
            RECEIVE_HEADER {}

            /// Decode header from raw bytes (stages/receive_header.rs)
            /// Name: "consensus.decode_header"
            DECODE_HEADER {}
        }
    }

    // =========================================================================
    // Network Schemas
    // =========================================================================
    network {
        chainsync_client {
            /// Find chain intersection point with peer (chain_sync_client.rs)
            /// Name: "chainsync_client.find_intersection"
            FIND_INTERSECTION {
                required peer: String
                required intersection_slot: u64
            }
        }
    }

    // =========================================================================
    // Ledger Schemas
    // =========================================================================
    ledger {
        state {
            /// Roll forward ledger state with a new block (state.rs)
            /// Name: "ledger.roll_forward"
            ROLL_FORWARD {}

            /// Apply a block to stable state (state.rs)
            /// Note: Uses function name as span name
            APPLY_BLOCK {
                required point_slot: u64
            }

            /// Epoch transition processing (state.rs)
            /// Note: Uses function name as span name
            EPOCH_TRANSITION {
                required from: u64
                required into: u64
            }

            /// Resolve transaction inputs from various sources (state.rs)
            /// Name: "state.resolve_inputs"
            RESOLVE_INPUTS {
                optional resolved_from_context: u64
                optional resolved_from_volatile: u64
                optional resolved_from_db: u64
            }

            /// Create validation context for a block (state.rs)
            /// Name: "ledger.create_validation_context"
            CREATE_VALIDATION_CONTEXT {
                required block_body_hash: String
                required block_number: u64
                required block_body_size: u64
                optional total_inputs: u64
            }

            /// Compute stake distribution for epoch (state.rs)
            /// Note: Uses function name as span name
            COMPUTE_STAKE_DISTRIBUTION {
                required epoch: u64
            }

            /// Tick proposals for ratification (state.rs)
            /// Name: "tick.proposals"
            TICK_PROPOSALS {
                required proposals_count: u64
            }

            /// Prepare block for validation (rules.rs)
            /// Name: "prepare_block"
            PREPARE_BLOCK {}

            /// Validate block against rules (rules/block.rs)
            /// Name: "validate_block"
            VALIDATE_BLOCK {}

            /// Tick pool operations (state.rs)
            /// Name: "tick.pool"
            TICK_POOL {}

            /// Compute rewards for epoch (state.rs)
            /// Note: Uses function name as span name
            COMPUTE_REWARDS {}

            /// Forward ledger state with new volatile state (state.rs)
            /// Note: Uses function name as span name
            FORWARD {}

            /// End epoch operations (state.rs)
            /// Note: Uses function name as span name
            END_EPOCH {}

            /// Begin epoch operations (state.rs)
            /// Note: Uses function name as span name
            BEGIN_EPOCH {}

            /// Compute stake distribution for epoch (state.rs)
            /// Name: "compute_stake_distribution"
            COMPUTE_STAKE_DISTRIBUTION_NAMED {}

            /// Reset fees to zero (state.rs)
            /// Name: "reset.fees"
            RESET_FEES {}

            /// Reset blocks count to zero (state.rs)
            /// Name: "reset.blocks_count"
            RESET_BLOCKS_COUNT {}

            /// Roll backward to a specific point (state.rs)
            /// Name: "ledger.roll_backward"
            ROLL_BACKWARD {}

            /// Create ratification context (state.rs)
            /// Name: "ratification.context.new"
            RATIFICATION_CONTEXT_NEW {}

            /// Manage transaction outputs (state.rs)
            /// Note: Uses function name as span name
            MANAGE_TRANSACTION_OUTPUTS {}

            /// Cleanup old epochs (state.rs)
            /// Note: Uses function name as span name  
            CLEANUP_OLD_EPOCHS {}

            /// Cleanup expired proposals (state.rs)
            /// Note: Uses function name as span name
            CLEANUP_EXPIRED_PROPOSALS {}
        }

        rules {
            /// Parse raw block bytes (rules.rs)
            /// Name: "ledger.parse_block"
            PARSE_BLOCK {
                required block_size: u64
            }
        }

        context {
            /// Add transaction fees to pots (assert.rs)
            /// Name: "add_fees"
            ADD_FEES {
                required fee: u64
            }

            /// Withdraw from stake credential (assert.rs)
            /// Name: "withdraw_from"
            WITHDRAW_FROM {
                required credential_type: String
                required credential_hash: String
            }

            /// Record a governance vote (assert.rs)
            /// Name: "vote"
            VOTE {
                required voter_type: String
                required credential_type: String
                required credential_hash: String
            }

            /// Require a verification key witness (assert.rs)
            /// Name: "require_vkey_witness"
            REQUIRE_VKEY_WITNESS {
                required hash: String
            }

            /// Require a script witness (assert.rs)
            /// Name: "require_script_witness"
            REQUIRE_SCRIPT_WITNESS {
                required hash: String
            }

            /// Require a bootstrap witness (assert.rs)
            /// Name: "require_bootstrap_witness"
            REQUIRE_BOOTSTRAP_WITNESS {
                required bootstrap_witness_hash: String
            }
        }

        governance {
            /// Ratify proposals at epoch boundary (ratification.rs)
            /// Note: Uses function name as span name
            RATIFY_PROPOSALS {
                optional roots_protocol_parameters: String
                optional roots_hard_fork: String
                optional roots_constitutional_committee: String
                optional roots_constitution: String
            }
        }
    }

    // =========================================================================
    // Store Schemas
    // =========================================================================
    stores {
        ledger {
            /// Create ledger snapshot for epoch (rocksdb/mod.rs)
            /// Name: "snapshot"
            SNAPSHOT {
                required epoch: u64
            }

            /// Prune old snapshots (rocksdb/mod.rs)
            /// Note: Uses function name as span name
            PRUNE {
                required functional_minimum: u64
            }

            /// Epoch transition tracking (rocksdb/mod.rs)
            /// Name: "try_epoch_transition"
            TRY_EPOCH_TRANSITION {
                optional has_from: bool
                optional has_to: bool
                optional point: String
                optional snapshots: String
            }

            /// Remove DRep delegations (dreps_delegations.rs)
            /// Name: "dreps_delegations.remove"
            DREPS_DELEGATION_REMOVE {
                required drep_hash: String
                required drep_type: String
            }
        }

        consensus {
            /// Store a block header (consensus/mod.rs)
            /// Name: "consensus.store.store_header"
            STORE_HEADER {
                required hash: String
            }

            /// Store a raw block (consensus/mod.rs)
            /// Name: "consensus.store.store_block"
            STORE_BLOCK {
                required hash: String
            }

            /// Roll forward the chain to a point (consensus/mod.rs)
            /// Name: "consensus.store.roll_forward_chain"
            ROLL_FORWARD_CHAIN {
                required hash: String
                required slot: u64
            }

            /// Rollback the chain to a point (consensus/mod.rs)
            /// Name: "consensus.store.rollback_chain"
            ROLLBACK_CHAIN {
                required hash: String
                required slot: u64
            }

            /// Store block to tip operations (consensus/mod.rs)
            /// Note: Uses function name as span name
            STORE_BLOCK_TO_TIP {
                required hash: String
            }

            /// Rollback to tip operations (consensus/mod.rs)
            /// Note: Uses function name as span name
            ROLLBACK_TO_TIP {
                required hash: String
            }

            /// Read headers operations (consensus/mod.rs)
            /// Note: Uses function name as span name
            READ_HEADERS {
                required hash: String
            }

            /// Read blocks operations (consensus/mod.rs)
            /// Note: Uses function name as span name
            READ_BLOCKS {
                required hash: String
            }
        }
    }

    // =========================================================================
    // Protocols Schemas
    // =========================================================================
    protocols {
        mux {
            /// Register protocol with muxer (mux.rs)
            /// Note: Uses function name as span name
            REGISTER {}

            /// Buffer protocol messages (mux.rs)
            /// Note: Uses function name as span name
            BUFFER {}

            /// Handle outgoing protocol messages (mux.rs)
            /// Note: Uses function name as span name
            OUTGOING {
                optional proto_id: String
                optional bytes: u64
            }

            /// Get next segment to send (mux.rs)
            /// Note: Uses function name as span name
            NEXT_SEGMENT {}

            /// Handle received protocol data (mux.rs)
            /// Note: Uses function name as span name
            RECEIVED {
                optional bytes: u64
            }

            /// Want next message for protocol (mux.rs)
            /// Note: Uses function name as span name
            WANT_NEXT {}

            /// Demultiplex incoming bytes (mux.rs)
            /// Note: Uses function name as span name
            DEMUX {
                required proto_id: u16
                required bytes: u64
            }

            /// Multiplex outgoing bytes (mux.rs)
            /// Note: Uses function name as span name
            MUX {
                required bytes: u64
            }
        }
    }
}

// =============================================================================
// Additional Constants (not schema-based, used with standard tracing macros)
// =============================================================================

// These are span name constants used directly with tracing macros (info_span!, trace_span!, etc.)
// rather than with the #[trace] attribute macro.

/// Additional ledger-related span name constants
pub mod ledger {
    /// Enacting a governance proposal (ratification.rs)
    pub const ENACTING: &str = "enacting";
    /// Ratifying a governance proposal (ratification.rs)
    pub const RATIFYING: &str = "ratifying";
}

/// Additional consensus-related span name constants
pub mod consensus {
    pub mod diffusion {
        pub const FETCH_BLOCK: &str = "diffusion.fetch_block";
        pub const FORWARD_CHAIN: &str = "diffusion.forward_chain";
        pub const CHAIN_SYNC: &str = "diffusion.chain_sync";
        pub const CHAIN_SYNC_WAIT: &str = "diffusion.chain_sync.wait";
    }

    pub mod chain_sync {
        pub const DECODE_HEADER: &str = "chain_sync.decode_header";
        pub const RECEIVE_HEADER: &str = "chain_sync.receive_header";
        pub const RECEIVE_HEADER_DECODE_FAILED: &str = "chain_sync.receive_header.decode_failed";
        pub const SELECT_CHAIN: &str = "chain_sync.select_chain";
        pub const TRACK_PEERS: &str = "chain_sync.track_peers";
        pub const VALIDATE_BLOCK: &str = "chain_sync.validate_block";
        pub const VALIDATE_HEADER: &str = "chain_sync.validate_header";
    }
}
