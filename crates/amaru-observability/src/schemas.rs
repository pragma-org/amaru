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

//! Amaru tracing schemas declared with the `define_schemas!` embedded DSL.
//!
//! Each schema is a compile-time contract for a tracing span or event: its path, required
//! and optional fields, types, visibility, and functional tags. Call sites use
//! [`trace_span!`](crate::trace_span), [`trace_event!`](crate::trace_event), and
//! [`trace_record!`](crate::trace_record) against these schemas; missing required fields,
//! unknown fields, and type mismatches fail at compile time.
//!
//! # Embedded DSL
//!
//! Schemas are written inside [`define_schemas!`](amaru_observability_macros::define_schemas)
//! as a nested category tree. Category names become Rust modules; schema names become unit
//! structs with associated constants and typed field accessors. Identifiers and types in
//! this file are re-emitted by the proc macro with their original source spans, so
//! go-to-definition from a generated item navigates back to the definition here.
//!
//! ```text
//! define_schemas! {
//!     <category> {
//!         tags: <tag>, <tag>, ...          // optional; inherited by nested schemas
//!         <category> { ... }               // nested category
//!         /// Description of the event     // required on every schema
//!         [public] <SCHEMA> {
//!             tags: <tag>, ...             // optional; overrides inherited tags
//!             required <field>: <Type> [,]
//!             optional <field>: <Type> [,]
//!         }
//!     }
//! }
//! ```
//!
//! ## Categories and paths
//!
//! Categories are lowercase identifiers; they nest arbitrarily. Schema names start with an
//! uppercase letter (conventionally `SCREAMING_SNAKE_CASE`). The category path determines:
//!
//! - the Rust path of the generated marker type (`amaru::ledger::state::ROLL_FORWARD`);
//! - the tracing `target` (first two segments, e.g. `amaru::ledger`);
//! - the span/event `name` (remaining segments plus the schema name, lowercased and joined
//!   with `.`, e.g. `state.roll_forward`).
//!
//! ## Schemas
//!
//! Every schema **must** have a `///` doc comment (multi-line docs are joined for the
//! runtime registry). Schemas are **private by default**; mark with `public` to always emit
//! and to include the schema in the runtime dump used by documentation tooling. Private
//! schemas emit only when `AMARU_TRACE_EMIT_PRIVATE` is set. Empty field lists are valid.
//!
//! ## Fields
//!
//! Fields use a prefix keyword and a Rust type:
//!
//! - `required name: Type` — must be present at every `trace_span!` / `trace_event!` site;
//! - `optional name: Type` — may be omitted; may be filled later with `trace_record!`.
//!
//! Trailing commas after types are allowed. Field names must be Rust identifiers; `name`,
//! `schema`, and `message` are reserved. Types may be paths or generics
//! (`amaru_kernel::Hash<28>`). `String` accepts any `AsRef<str>`; primitives use typed
//! `tracing::Value` transport; other types must implement `Serialize` (CBOR via `record_bytes`)
//! when recorded without an explicit `%` / `?` formatter.
//!
//! ## Tags
//!
//! `tags: cpu, io` (module-level or schema-level) attaches boolean span attributes
//! `amaru.tag.<name>`. Module tags are inherited; a schema-level `tags:` replaces them.
//! Select tagged spans with e.g. `AMARU_LOG='[{amaru.tag.cpu=true}]=trace'`.
//!
//! ## Generated API (per schema)
//!
//! For each schema the expansion provides a unit struct with `NAME`, `TARGET`, `PATH`,
//! `VALIDATION`, `PUBLIC`, `FIELD_*` constants, `matches()`, and typed `field(record)`
//! accessors (for use with [`RecordFields`](crate::RecordFields)). Hidden declarative
//! macros implement the compile-time checks invoked by the instrumentation macros.
//!
//! See also the language reference on
//! [`define_schemas!`](amaru_observability_macros).

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
                /// Initialize the store
                INITIALIZE {
                    required ledger_tip: amaru_kernel::Point
                    optional best_chain_hash: amaru_kernel::HeaderHash
                }
                /// Remove the valid status of descendants of a given block to reapply those blocks.
                CLEAR_VALID_DESCENDANTS {
                    required count: usize
                }
            }
            blocks {
                /// Validate downloaded blocks that are not yet validated
                RECOVER_STORED {
                    tags: setup
                    required from: amaru_kernel::Point
                    required to: amaru_kernel::HeaderHash
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
                    required peer: amaru_kernel::Peer
                    required intersection_slot: amaru_kernel::Slot
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
                    required peer: amaru_kernel::Peer
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
                    required issuer_key: amaru_kernel::VerificationKey
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
            tip {
                /// Adopt a tip as the next tip in the best chain
                public ADOPT {
                    required slot: amaru_kernel::Slot
                    required header_hash: amaru_kernel::HeaderHash
                    required block_height: u64
                    required max_block_height: u64
                    required suppressed: u32
                }
            }
            peer {
                tags: cpu
                /// A peer behaves like an adversary, ban it
                BAN {
                    required peer: amaru_kernel::Peer
                }
            }
            perf {
                header {
                    /// Event recorded once per header, when its processing reaches a terminal state.
                    /// It covers the four network-health processing points of a header's lifecycle:
                    /// reception of the header, request of its block, reception of its block and
                    /// local adoption of the block. `outcome` describes the terminal state (including
                    /// headers rejected on reception, which carry no durations). The optional
                    /// durations are the intervals between those points:
                    /// - `block_fetch_wait_micros`: reception of the header to the request of its block
                    /// - `block_fetch_micros`: request of the block to its reception
                    /// - `forward_micros`: reception of the header to the adoption of its block
                    public LIFECYCLE {
                        optional peer: amaru_kernel::Peer
                        optional header_hash: amaru_kernel::HeaderHash
                        optional outcome: String
                        optional error: String
                        optional slot_start_to_header_micros: u64
                        optional block_fetch_wait_micros: u64
                        optional block_fetch_micros: u64
                        optional forward_micros: u64
                    }
                }
                fork {
                    /// Event recorded when a fork switch ends. `duration_micros` measures the time
                    /// from the detection of the fork to its application (or abandonment).
                    public SWITCH {
                        required header_hash: amaru_kernel::HeaderHash
                        optional outcome: String
                        optional duration_micros: u64
                    }
                }
            }
        }
        ledger {
            tags: cpu
            state {
                /// Roll forward with a new block
                public ROLL_FORWARD {}
                /// Roll backward to a specific point
                public ROLL_BACKWARD {}
                /// Switching to an alternative chain fork
                public SWITCH_TO_FORK {
                    required fork_point: amaru_kernel::Point
                    required fork_length: usize
                    required rollback_length: usize
                    optional outcome: String
                }
                /// Forward ledger state with new volatile state
                public PUSH {}
            }
            tip {
                /// Updated view of the locally adopted chain tip and its derived ledger health.
                public UPDATE {
                    required slot: amaru_kernel::Slot
                    required header_hash: amaru_kernel::HeaderHash
                    required block_height: u64
                    required tx_count: usize
                    required epoch: amaru_kernel::Epoch
                    required slot_in_epoch: amaru_kernel::Slot
                    required density: f64
                    required current_kes_period: u64
                    required remaining_kes_periods: u64
                }
            }
            stake_distribution {
                /// Start computing one of the initial stake distributions loaded on startup
                public INITIAL_BEGIN {
                    required epoch: amaru_kernel::Epoch
                }
                /// Report progress for one of the initial stake distributions loaded on startup
                public INITIAL_PROGRESS {
                    required epoch: amaru_kernel::Epoch
                    required progress: f64
                }
                /// Finished computing all initial stake distributions loaded on startup
                public INITIAL_READY {
                    required epochs: String
                }
                /// Compute stake distribution for epoch
                public COMPUTE {
                    required epoch: amaru_kernel::Epoch
                }
                /// Rotate stake distributions at an epoch boundary
                public ROTATE {
                    required available_stake_distributions: String
                }
                /// Snapshot of the stake distribution taken at an epoch boundary
                public SNAPSHOT {
                    required accounts: usize
                    required dreps: usize
                    required pools: usize
                    required active_stake: amaru_kernel::Lovelace
                    required pools_voting_stake: amaru_kernel::Lovelace
                    required dreps_voting_stake: amaru_kernel::Lovelace
                }
            }
            rules {
                /// Validate block against ledger rules
                public EXECUTE {}
                phase_one {
                    /// Ledger rules related to block metadata and 'global' preflight checks
                    public BLOCK {}
                    /// Ledger rules and state-transitions for certificates
                    public CERTIFICATES {}
                    /// Ledger rules and state-transitions for collateral
                    public COLLATERAL {}
                    /// Ledger rules and state-transitions for treasury donation
                    public DONATION {}
                    /// Ledger rules and state-transitions for fees
                    public FEES {}
                    /// Ledger rules and state-transitions for inputs
                    public INPUTS {}
                    /// Ledger rules and state-transitions for metadata
                    public METADATA {}
                    /// Ledger rules and state-transitions for minte/burned assets
                    public MINT {}
                    /// Ledger rules and state-transitions for outputs
                    public OUTPUTS {}
                    /// Ledger rules and state-transitions for governance proposals
                    public PROPOSALS {}
                    /// Ledger rules and state-transitions for script witnesses
                    public SCRIPTS {}
                    /// Ledger rules and state-transitions for key signatures
                    public SIGNATURES {}
                    /// Ledger rules and state-transitions for validity interval
                    public VALIDITY_INTERVAL {}
                    /// Ledger rules and state-transitions for governance votes
                    public VOTES {}
                    /// Ledger rules and state-transitions for withdrawas
                    public WITHDRAWALS {}
                }
                phase_two {
                    /// Initialize script context and cost models, common to all scripts
                    public BUILD_SCRIPT_CONTEXT {}
                    /// A span wrapping all script executions
                    public EXECUTE_SCRIPTS {}
                    /// A single script execution, with the associated redeemer qualifiers
                    public EXECUTE_ONE_SCRIPT {
                        required purpose: String
                        required index: u32
                    }
                    /// Acquiring the allocation arena for decoding and execution
                    public ACQUIRE_ARENA {}
                    /// Decoding the script from Cbor/Flat
                    public DECODE_SCRIPT {}
                    /// Construct the UPLC program from parameters, decoded script and context
                    public BUILD_UPLC_PROGRAM {}
                    /// Execute the fully-applied UPLC program
                    public EVALUATE_UPLC_PROGRAM {}
                }
            }
            rewards {
                /// Compute rewards for epoch
                public COMPUTE {
                    required for_epoch: amaru_kernel::Epoch
                    required using_stake_distribution_from_epoch: amaru_kernel::Epoch
                }
                /// Summary of the rewards calculation for an epoch
                public SUMMARIZE {
                    required efficiency: String
                    required incentives: amaru_kernel::Lovelace
                    required treasury_tax: amaru_kernel::Lovelace
                    required total_rewards: amaru_kernel::Lovelace
                    required available_rewards: amaru_kernel::Lovelace
                    required effective_rewards: amaru_kernel::Lovelace
                    required pots_reserves: amaru_kernel::Lovelace
                    required pots_treasury: amaru_kernel::Lovelace
                    required pots_fees: amaru_kernel::Lovelace
                }
            }
            block {
                /// Apply a block to stable state
                public APPLY {
                    required point_slot: amaru_kernel::Slot
                }
                /// Prepare block for validation
                public PREPARE {}
            }
            transaction {
                /// Validate a single transaction
                public VALIDATE {
                    required transaction_id: amaru_kernel::TransactionId,
                }
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
                    required deposit: amaru_kernel::Lovelace
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
                    required refund: amaru_kernel::Lovelace
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
                    required epoch: amaru_kernel::Epoch
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
                /// Found a transaction while applying a block
                public FOUND {
                    required point: amaru_kernel::Point
                    required block_height: u64
                    required tx_index: usize
                    required tx_id: amaru_kernel::TransactionId
                }
            }
            block_validation_context {
                /// Create validation context for a block
                public CREATE {
                    required block_body_hash: amaru_kernel::HeaderHash
                    required block_number: u64
                    required block_body_size: u64
                    optional total_inputs: u64
                }
            }
            transaction_validation_context {
                /// Create validation context for a transaction
                public CREATE {
                    required transaction_id: amaru_kernel::TransactionId
                }
            }
            validation_context {
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
            }
            relays {
                /// Fetch candidate relays from the immutable store
                public COLLECT {
                    optional count: String
                }
            }
            epoch_transition {
                /// Epoch transition processing
                public COMPUTE {
                    required from: amaru_kernel::Epoch
                    required into: amaru_kernel::Epoch
                    optional skipped: bool
                    optional resuming_from: String
                }
                /// Create pools updates
                public NEW_POOLS_UPDATES {}
                /// Create governance updates (i.e. ratify proposals) at an epoch boundary.
                public NEW_GOVERNANCE_UPDATES {
                    /// Total number of proposals in scope. This also includes proposals that have
                    /// *just* been submitted.
                    required proposals_count: u64
                }
                /// Flushing the epoch transition overlay to disk
                public APPLY {
                    /// Epoch for which this overlay is being flush; This is the *currently active*
                    /// epoch.
                    required epoch: amaru_kernel::Epoch
                    /// Whether to end the epoch; in case Amaru is restarting mid-update.
                    optional should_end_epoch: bool,
                    /// Whether to take an on-disk snapshot; in case Amaru is restarting mid-update.
                    optional should_snapshot: bool,
                    /// Whether to begin the epoch; in case Amaru is restarting mid-update.
                    optional should_begin_epoch: bool,
                }
                /// Update a pool's parameters at an epoch boundary; only changed parameters are recorded
                public TICK_POOL {
                    required id: amaru_kernel::PoolId
                    optional vrf: String
                    optional pledge: String
                    optional cost: String
                    optional margin: String
                    optional reward_account: String
                    optional owners: String
                    optional relays: String
                    optional metadata: String
                }
                /// Retire a pool at an epoch boundary
                public RETIRE_POOL {
                    required id: amaru_kernel::PoolId
                }
                /// Rollback an in-flight epoch transition
                public ROLLBACK {
                    required from: amaru_kernel::Epoch
                    required to: amaru_kernel::Epoch
                }
                /// Record an in-flight epoch transition
                public RECORD {
                    required from: amaru_kernel::Epoch
                    required to: amaru_kernel::Epoch
                }
            }
            governance {
                /// Create ratification context
                public NEW_RATIFICATION_CONTEXT {
                    /// Epoch to ratify; distinct from the actual epoch this calculation is happening.
                    required ratifying_epoch: amaru_kernel::Epoch
                    /// Value of the treasury considered for this ratification round.
                    optional treasury: amaru_kernel::Lovelace
                    /// Total number of votes to ratify.
                    optional votes: u64
                }
                /// Ratify proposals at epoch boundary
                public RATIFY_PROPOSALS {
                    required epoch: amaru_kernel::Epoch
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
            volatile {
                /// Recompute the volatile aggregate
                public AGGREGATE {}
                /// The volatile db is still warming up and hasn't reached a stable point yet
                public WARM_UP {
                    required size: usize
                }
            }
            account {
                /// Pay withdrawals to an account, or refund its deposit
                public PAY_OR_REFUND {
                    required credential_type: amaru_kernel::StakeCredentialKind
                    required account: amaru_kernel::Hash<28>
                    required deposit: amaru_kernel::Lovelace
                }
            }
            chain_growth {
                /// Fewer than k blocks were seen within the stability window
                public VIOLATE {
                    required unstable_tail_length: usize
                    required reason: String
                }
            }
            constitutional_committee {
                /// The constitutional committee votes were ignored during ratification
                public IGNORE {
                    required active_members: usize
                    required min_committee_size: u16
                    required reason: String
                }
            }
            governance_activity {
                /// Update the number of consecutive dormant epochs
                public UPDATE {
                    required consecutive_dormant_epochs: u32
                }
            }
            non_empty_block {
                /// Found a non-empty block while applying it to the ledger
                public FOUND {
                    required point: amaru_kernel::Point
                    required block_height: u64
                    required tx_count: usize
                }
            }
            pots {
                /// Load the current ledger pots
                public LOAD {
                    required treasury: amaru_kernel::Lovelace
                    required reserves: amaru_kernel::Lovelace
                    required fees: amaru_kernel::Lovelace
                    required donations: amaru_kernel::Lovelace
                }
            }
            overlay {
                /// No pools updates found in the epoch transition overlay
                public NO_POOLS_UPDATES {}
                /// No governance updates found in the epoch transition overlay
                public NO_GOVERNANCE_UPDATES {}
            }
            proposal {
                /// Observe a governance proposal that is currently active
                public ACTIVE {
                    required id: String
                    required proposal_kind: String
                    required proposed_in: amaru_kernel::Epoch
                    required valid_until: amaru_kernel::Epoch
                    optional detail: String
                }
                /// Drop an expired or ratified governance proposal
                public DROP {
                    required id: String
                    required expired: bool
                    required ratified_or_evicted: bool
                }
                /// Skip a governance proposal during ratification
                public SKIP {
                    required id: String
                    required reason: String
                    optional proposed_in: String
                    optional ratifying_epoch: amaru_kernel::Epoch
                    optional withdrawal: amaru_kernel::Lovelace
                    optional treasury: amaru_kernel::Lovelace
                    optional invalid_members: String
                }
            }
            proposal_roots {
                /// Summary of the governance proposal roots after ratification
                public SUMMARIZE {
                    optional constitution: String
                    optional constitutional_committee: String
                    optional hard_fork: String
                    optional protocol_parameters: String
                }
            }
            protocol {
                /// Upgrade to a new protocol version
                public UPGRADE {
                    required old_version: u64
                    required new_version: u64
                }
            }
            protocol_parameters {
                /// Load the current protocol parameters
                public LOAD {
                    optional protocol_version: String
                    optional max_block_body_size: String
                    optional max_transaction_size: String
                    optional max_block_header_size: String
                    optional max_tx_ex_units: String
                    optional max_block_ex_units: String
                    optional max_value_size: String
                    optional max_collateral_inputs: String
                    optional min_fee_a: String
                    optional min_fee_b: String
                    optional stake_credential_deposit: String
                    optional stake_pool_deposit: String
                    optional monetary_expansion_rate: String
                    optional treasury_expansion_rate: String
                    optional min_pool_cost: String
                    optional lovelace_per_utxo_byte: String
                    optional prices: String
                    optional min_fee_ref_script_lovelace_per_byte: String
                    optional max_ref_script_size_per_tx: String
                    optional max_ref_script_size_per_block: String
                    optional ref_script_cost_stride: String
                    optional ref_script_cost_multiplier: String
                    optional stake_pool_max_retirement_epoch: String
                    optional optimal_stake_pools_count: String
                    optional pledge_influence: String
                    optional collateral_percentage: String
                    optional cost_models: String
                    optional pool_voting_thresholds: String
                    optional drep_voting_thresholds: String
                    optional min_committee_size: String
                    optional max_committee_term_length: String
                    optional gov_action_lifetime: String
                    optional gov_action_deposit: String
                    optional drep_deposit: String
                    optional drep_expiry: String
                }
                /// Ratify a protocol parameters update; only changed parameters are recorded
                public RATIFY {
                    optional protocol_version: String
                    optional max_block_body_size: String
                    optional max_transaction_size: String
                    optional max_block_header_size: String
                    optional max_tx_ex_units: String
                    optional max_block_ex_units: String
                    optional max_value_size: String
                    optional max_collateral_inputs: String
                    optional min_fee_a: String
                    optional min_fee_b: String
                    optional stake_credential_deposit: String
                    optional stake_pool_deposit: String
                    optional monetary_expansion_rate: String
                    optional treasury_expansion_rate: String
                    optional min_pool_cost: String
                    optional lovelace_per_utxo_byte: String
                    optional prices: String
                    optional min_fee_ref_script_lovelace_per_byte: String
                    optional max_ref_script_size_per_tx: String
                    optional max_ref_script_size_per_block: String
                    optional ref_script_cost_stride: String
                    optional ref_script_cost_multiplier: String
                    optional stake_pool_max_retirement_epoch: String
                    optional optimal_stake_pools_count: String
                    optional pledge_influence: String
                    optional collateral_percentage: String
                    optional cost_models: String
                    optional pool_voting_thresholds: String
                    optional drep_voting_thresholds: String
                    optional min_committee_size: String
                    optional max_committee_term_length: String
                    optional gov_action_lifetime: String
                    optional gov_action_deposit: String
                    optional drep_deposit: String
                    optional drep_expiry: String
                }
            }
            ratification {
                /// Summary of the outcome of a ratification round
                public SUMMARIZE {
                    required is_dormant_epoch: bool
                    optional pruned_proposals: String
                    optional refunds: String
                    optional withdrawals: String
                    optional new_constitution: String
                    optional constitutional_committee_update: String
                }
                /// Skip the remaining proposals for this epoch
                public SKIP {
                    required reason: String
                }
            }
        }
        bootstrap {
            accounts {
                /// Existing accounts found in the store before import
                public IS_NOT_EMPTY {}
                /// Import accounts from a snapshot
                public IMPORT {
                    required size: usize
                }
            }
            block_issuers {
                /// Import block issuers from a snapshot
                public IMPORT {
                    required count: u64
                }
            }
            constitution {
                /// Import the constitution from a snapshot
                public IMPORT {
                    required anchor: String
                    required guardrails: String
                }
            }
            constitutional_committee {
                /// Import the constitutional committee from a snapshot
                public IMPORT {
                    required state: String
                    optional threshold: String
                    optional members: usize
                }
            }
            dreps {
                /// Import DReps from a snapshot
                public IMPORT {
                    required size: usize
                }
            }
            fetch {
                /// Received a rollback while fetching bootstrap headers
                public ROLLBACK {
                    required point: amaru_kernel::Point
                    required tip: amaru_kernel::Tip
                }
            }
            governance_activity {
                /// Import the governance activity from a snapshot
                public IMPORT {
                    required dormant_epochs: u32
                }
            }
            header {
                /// Import a single header into the chain store
                public IMPORT {
                    required header: amaru_kernel::HeaderHash
                }
            }
            headers {
                /// Fetch bootstrap headers from a peer
                public FETCH {
                    required requested_point: amaru_kernel::Point
                    required intersection: amaru_kernel::Point
                    required headers_per_point: usize
                }
            }
            import {
                /// Import UTxO entries from a snapshot
                public UTXO {
                    required size: usize
                }
            }
            nonces {
                /// Import initial nonces into the chain store
                public IMPORT {
                    required point: amaru_kernel::Point
                }
            }
            opcert_sequence_numbers {
                /// Import initial opcert sequence numbers into the chain store
                public IMPORT {
                    required point: amaru_kernel::Point
                }
            }
            peer {
                /// Failed to connect to a peer while bootstrapping
                public FAILED_TO_CONNECT {
                    required peer: String
                    required reason: String
                }
            }
            pots {
                /// Import treasury/reserves/fees pots from a snapshot
                public IMPORT {
                    required treasury: amaru_kernel::Lovelace
                    required reserves: amaru_kernel::Lovelace
                    required fees: amaru_kernel::Lovelace
                    required donations: amaru_kernel::Lovelace
                }
            }
            proposal_roots {
                /// Import governance proposal roots from a snapshot
                public IMPORT {
                    required constitution: String
                    required constitutional_committee: String
                    required hard_fork: String
                    required protocol_parameters: String
                }
            }
            proposals {
                /// Existing proposals found in the store before import
                public IS_NOT_EMPTY {}
                /// Import governance proposals from a snapshot
                public IMPORT {
                    required size: usize
                }
            }
            recently_pruned_proposals {
                /// Import proposals pruned at the snapshot's epoch boundary, from its ratify state
                public IMPORT {
                    required size: usize
                }
            }
            snapshot {
                /// Download a snapshot archive
                public DOWNLOAD {
                    required epoch: amaru_kernel::Epoch
                    required point: amaru_kernel::Point
                }
                /// Snapshot already downloaded; skipping download
                public SKIP_DOWNLOAD {
                    required snapshot: String
                }
                /// Import a compressed snapshot archive
                public IMPORT_ARCHIVE {
                    required path: String
                }
                /// Import from the tvar data
                public IMPORT_TVAR {
                    required point: amaru_kernel::Point
                    required new_epoch_state_offset: usize
                }
            }
            snapshots {
                /// Import all snapshots
                public IMPORT {
                    required count: usize
                }
            }
            stake_pools {
                /// Import stake pools from a snapshot
                public IMPORT {
                    required registered: usize
                    required retiring: usize
                }
            }
            votes {
                /// Import governance votes from a snapshot
                public IMPORT {
                    required size: usize
                }
            }
        }
        cli {
            /// Process terminated with an error.
            public ERROR {
                required description: String
                optional cause: String
            }
            cardano_node_config {
                /// Use an existing cardano-node configuration
                public USE {
                    required config_dir: String
                    required network: amaru_kernel::NetworkName
                }
                /// Download the official cardano-node configuration bundle
                public DOWNLOAD {
                    required config_dir: String
                    required network: amaru_kernel::NetworkName
                }
            }
            chain_db {
                /// Chain database already exists
                public EXIST {
                    required dir: String
                    required hint: String
                }
            }
            current_epoch {
                /// Resolve the current epoch from Koios
                public RESOLVE {
                    required epoch: u64
                }
            }
            db_analyser {
                /// Run db-analyser to produce a ledger snapshot
                public RUN {
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    optional analyse_from: amaru_kernel::Slot
                }
                /// Reuse an existing db-analyser ledger snapshot
                public REUSE_LEDGER_SNAPSHOT {
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required snapshot: String
                }
            }
            last_block {
                /// Resolve the last produced block for an epoch
                public RESOLVE {
                    required epoch: amaru_kernel::Epoch
                    required point: amaru_kernel::Point
                }
            }
            ledger_db {
                /// Ledger database already exists
                public EXIST {
                    required dir: String
                    required hint: String
                }
            }
            mithril {
                /// Synchronize the cardano-node database from Mithril
                public DOWNLOAD {
                    required from_chunk: u64
                    required target_dir: String
                }
                /// Local cardano-node database is recent enough; skipping Mithril download
                public SKIP_DOWNLOAD {
                    required from_chunk: u64
                    required required_chunk: u64
                    required target_dir: String
                    required reason: String
                }
            }
            node {
                /// Bootstrap a node from published snapshots
                public BOOTSTRAP {
                    required chain_dir: String
                    required ledger_dir: String
                    required network: amaru_kernel::NetworkName
                    optional epoch: amaru_kernel::Epoch
                }
                /// Remove ledger and chain database from disk
                public RM {
                    required chain_dir: String
                    required ledger_dir: String
                    required network: amaru_kernel::NetworkName
                }
                /// Roll the node databases back after a failure
                public ROLLBACK {
                    required chain_dir: String
                    required ledger_dir: String
                    required network: amaru_kernel::NetworkName
                    required mode: String
                    optional epoch: u64
                    optional ledger_tip: String
                    optional best_chain: String
                    optional anchor: String
                }
            }
            snapshot {
                /// Create snapshots for the given network
                public CREATE {
                    required network: amaru_kernel::NetworkName
                    optional epoch: amaru_kernel::Epoch
                    required snapshot_output_dir: String
                    required config_dir: String
                    required cardano_node_db: String
                    required dist_dir: String
                    optional snapshots: String
                }
                /// Finished creating a snapshot archive
                public CREATED {
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required archive: String
                }
                /// Package a snapshot archive
                public PACKAGE {
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required archive: String
                }
                /// Snapshot archive already packaged; skipping
                public SKIP_PACKAGE {
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required archive: String
                    required reason: String
                }
                /// Publish snapshot archives
                public PUBLISH {
                    required network: amaru_kernel::NetworkName
                    required local: usize
                    required remote: usize
                }
                /// Upload a snapshot archive
                public UPLOAD {
                    required archive: String
                }
                /// Finished uploading a snapshot archive
                public UPLOADED {
                    required archive: String
                }
                /// Snapshot archive already uploaded; skipping
                public SKIP_UPLOAD {
                    required archive: String
                }
                /// Update the published snapshot index
                public UPDATE_INDEX {
                    required network: amaru_kernel::NetworkName
                    required snapshots: usize
                }
            }
        }
        mithril {
            snapshot {
                /// Fetch and verify a Mithril snapshot
                public FETCH {
                    required hash: String
                    required from_chunk: u64
                }
                /// Download and unpack immutable files from a Mithril snapshot
                public DOWNLOAD {
                    required target_dir: String
                    required from_chunk: u64
                }
                /// Download and verify the digests for a Mithril snapshot
                public VERIFY_DIGESTS {
                    required target_dir: String
                }
                /// Verify the local cardano-node database against a Mithril certificate
                public VERIFY_DATABASE {
                    required target_dir: String
                }
                /// Mithril cardano-node database is ready
                public READY {
                    required target_dir: String
                }
            }
        }
        stores {
            tags: db
            batch {
                /// Commit a write batch
                public COMMIT {}
                /// Rollback a write batch
                public ROLLBACK {}
            }
            ledger {
                epoch {
                    /// Create ledger snapshot for epoch
                    public CREATE_SNAPSHOT {
                        required epoch: amaru_kernel::Epoch
                    }
                    /// Prune old snapshots
                    public PRUNE_OLD_SNAPSHOTS {
                        required functional_minimum: amaru_kernel::Epoch
                        required desired_minimum: amaru_kernel::Epoch
                    }
                    /// Epoch transition tracking
                    public TRY_TRANSITION {
                        required from: String
                        required to: String
                    }
                }
                overlay {
                    /// Reset fees to zero
                    public RESET_FEES {}
                    /// Reset blocks count to zero
                    public RESET_BLOCKS_COUNT {}
                    /// Pay rewards to all accounts before the epoch end
                    public PAY_REWARDS {
                        /// Total number of accounts that received non-zero rewards
                        optional accounts_paid: u64
                        /// Total rewards effectively paid to ALL accounts; does not include unassignable rewards
                        optional rewards_paid: amaru_kernel::Lovelace
                        /// Treasury increase; corresponding to both the treasury tax and the unpaid rewards
                        optional treasury_delta: amaru_kernel::Lovelace
                        /// Reserves depletion from incentives; always negative.
                        optional reserves_delta: i64
                    }
                    /// Pruned proposals at an epoch boundary, recorded to facilitate future stake
                    /// distribution calculations.
                    public RECORD_PRUNED_PROPOSALS {}
                    /// Pay withdrawals to accounts, or refund deposits
                    public PAY_OR_REFUND_ACCOUNTS {
                        /// Total quantity of ADA paid, excluding treasury leftovers
                        optional total_paid_or_refunded: amaru_kernel::Lovelace
                        /// Total amounts that couldn't be paid to accounts, going back to treasury instead.
                        optional treasury_leftovers: amaru_kernel::Lovelace
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
                utxo {
                    /// Point-read a UTxO entry
                    public GET {}
                    /// Batch-insert UTxO entries
                    public ADD {}
                    /// Batch-delete UTxO entries
                    public REMOVE {}
                }
                pools {
                    /// Point-read a pool entry
                    public GET {}
                    /// Batch-upsert pool entries
                    public ADD {}
                    /// Schedule pool retirement
                    public REMOVE {
                        optional pool: amaru_kernel::PoolId
                        optional reason: String
                    }
                }
                accounts {
                    /// Point-read an account entry
                    public GET {}
                    /// Batch-upsert account entries
                    public ADD {}
                    /// Batch-delete account entries
                    public REMOVE {}
                    /// Update rewards balance for a single account
                    public SET {
                        optional credential_type: amaru_kernel::StakeCredentialKind
                        optional account: amaru_kernel::Hash<28>
                        optional reason: String
                    }
                    /// Reset rewards counters for many accounts
                    public RESET_MANY {
                        optional credential: amaru_kernel::StakeCredential
                        optional reason: String
                    }
                }
                recently_unregistered_accounts {
                    /// Insert a recently unregistered account
                    public INSERT {}
                    /// Remove a recently unregistered account
                    public REMOVE {}
                    /// Prune recently unregistered accounts
                    public PRUNE {
                        required epoch: amaru_kernel::Epoch
                    }
                }
                dreps {
                    /// Point-read a DRep entry
                    public GET {}
                    /// Batch-upsert DRep registrations
                    public ADD {
                        optional credential: amaru_kernel::StakeCredential
                        optional reason: String
                    }
                    /// Record DRep de-registration
                    public REMOVE {
                        optional drep: amaru_kernel::StakeCredential
                        optional reason: String
                    }
                    /// Refresh DRep expiry after a vote
                    public SET_VALID_UNTIL {
                        optional credential: amaru_kernel::StakeCredential
                        optional reason: String
                    }
                }
                cc_members {
                    /// Read a constitutional committee member
                    public GET {}
                    /// Upsert a constitutional committee member
                    public UPSERT {}
                }
                proposals {
                    /// Insert governance proposals
                    public ADD {}
                    /// Read governance proposals
                    public GET { }
                    /// Remove enacted or expired proposals
                    public REMOVE {}
                }
                recently_pruned_proposals {
                    /// Inserting recently pruned proposals
                    public REPLACE_ALL {}
                }
                votes {
                    /// Record governance votes
                    public ADD {}
                }
                slots {
                    /// Point-read a slot/block-issuer entry
                    public GET {}
                    /// Write a slot/block-issuer entry
                    public PUT {}
                }
                pots {
                    /// Read treasury/reserve/fees pots
                    public GET {}
                    /// Write treasury/reserve/fees pots
                    public PUT {}
                }
                snapshots {
                    /// Validate sufficient snapshots exist
                    public VALIDATE {
                        optional snapshot_count: u64
                        optional continuous_ranges: u64
                    }
                }
                /// Full scan for a given collection
                public ITER_SCAN {
                    required db_collection_name: String
                    optional rows_scanned: u64
                    optional rows_written: u64
                    optional rows_deleted: u64
                }
            }
            consensus {
                header {
                    /// Store a block header
                    public STORE {
                        required hash: amaru_kernel::HeaderHash
                    }
                }
                block {
                    /// Store a raw block
                    public STORE {
                        required hash: amaru_kernel::HeaderHash
                    }
                }
                chain {
                    /// Roll forward the chain to a point
                    public ROLL_FORWARD {
                        required hash: amaru_kernel::HeaderHash
                        required slot: amaru_kernel::Slot
                    }
                    /// Switch the chain to a new fork
                    public SWITCH_TO_FORK {
                        required hash: amaru_kernel::HeaderHash
                        required slot: amaru_kernel::Slot
                    }
                }
            }
        }
        mempool {
            state {
                /// Compact view of the mempool occupancy for terminal dashboards.
                public UPDATE {
                    required tx_count: u64
                    required size_bytes: u64
                }
            }
            transaction {
                /// Transaction received by the mempool stage, before validation.
                public RECEIVED {
                    required tx_id: amaru_kernel::TransactionId
                    required origin: String
                }
                /// Transaction validated and inserted into the mempool.
                public ACCEPTED {
                    required tx_id: amaru_kernel::TransactionId
                    required seq_no: u64
                    required origin: String
                }
                /// Transaction rejected at insertion. Reason ∈ {invalid, duplicate, mempool_full}.
                public REJECTED {
                    required tx_id: amaru_kernel::TransactionId
                    required reason: String
                    optional validation_error: String
                }
                /// Transaction removed from the mempool. Reason ∈ {invalid_after_tip}.
                /// TODO: split the reason into invalid after tip + present in applied block
                public EVICTED {
                    required tx_id: amaru_kernel::TransactionId
                    required reason: String
                }
                /// Detail trace carrying upstream peer attribution for a received tx.
                RECEIVED_DETAIL {
                    required tx_id: amaru_kernel::TransactionId
                    required peer: amaru_kernel::Peer
                }
                /// Detail trace for a tip-driven revalidation pass.
                REVALIDATION_DETAIL {
                    required tip_slot: amaru_kernel::Slot
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
                        required peer: amaru_kernel::Peer
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
                        required peer: amaru_kernel::Peer
                    }
                    /// Initiating an outbound connection to a peer
                    public CONNECT {
                        required peer: amaru_kernel::Peer
                    }
                    /// An inbound connection was accepted from a peer
                    public ACCEPTED {
                        required peer: amaru_kernel::Peer
                        required conn_id: String
                    }
                    /// A peer was removed from the manager
                    public REMOVE {
                        required peer: amaru_kernel::Peer
                    }
                    /// A peer connection has died
                    public CONNECTION_DIED {
                        required peer: amaru_kernel::Peer
                        required conn_id: String
                        required role: String
                    }
                }
            }
            peer_selection {
                peer {
                    /// A connection has been established and the handshake completed successfully.
                    public CONNECTED {
                        required peer: amaru_kernel::Peer
                        required conn_id: u64
                        required direction: String
                        required full_duplex_capable: bool
                        required full_duplex: bool
                    }
                    /// A connection has been terminated (graceful disconnect, error, handshake refusal,
                    /// or network error).
                    public DISCONNECTED {
                        required peer: amaru_kernel::Peer
                        required conn_id: u64
                        required direction: String
                        optional reason: String
                    }
                }
                sharing {
                    /// Peer-sharing address list received from peer.
                    public RECEIVED {
                        /// Peer that answered (learn) or requested (advertise) the share.
                        required peer: amaru_kernel::Peer
                        /// Comma-separated list of shared listen addresses.
                        required peers: String
                        /// how many addresses were newly added to the shared pool.
                        required added: usize
                        /// size of the shared-peers pool after this reply.
                        required total: usize
                    }
                    /// Peer-sharing request served for peer.
                    public SENT {
                        /// Peer that answered (learn) or requested (advertise) the share.
                        required peer: amaru_kernel::Peer
                        /// Comma-separated list of shared listen addresses.
                        required peers: String
                        /// number of addresses requested.
                        required requested: u8
                        /// number of addresses returned.
                        required count: usize
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
                peer {
                    /// Measured round-trip time for a keepalive exchange on an established peer connection.
                    public ROUND_TRIP {
                        required peer: amaru_kernel::Peer
                        required conn_id: String
                        required round_trip_micros: u64
                    }
                }
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
            peer_sharing {
                initiator {
                    /// Handle peer-sharing initiator stage messages
                    PEER_SHARING_INITIATOR_STAGE {
                        required peer: String
                        required conn_id: String
                    }
                    /// Handle peer-sharing initiator protocol messages
                    PEER_SHARING_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                }
                responder {
                    /// Handle peer-sharing responder stage messages
                    PEER_SHARING_RESPONDER_STAGE {
                        required amount: u8
                    }
                    /// Handle peer-sharing responder protocol messages
                    PEER_SHARING_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            tx_submission {
                initiator {
                    /// Handle tx-submission initiator stage messages
                    TX_SUBMISSION_INITIATOR_STAGE {
                        required message_type: String
                        required peer: amaru_kernel::Peer
                    }
                    /// Handle tx-submission initiator protocol messages
                    TX_SUBMISSION_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                    /// Advertise transaction ids (and their sizes) to the peer in a ReplyTxIds.
                    REPLY_TX_IDS {
                        required peer: amaru_kernel::Peer
                        required count: usize
                        required tx_ids: &[amaru_kernel::TransactionId]
                    }
                    /// Send transaction bodies to the peer in a ReplyTxs. Advertised ids whose
                    /// tx was evicted before the fetch are listed in `omitted`.
                    REPLY_TXS {
                        required peer: amaru_kernel::Peer
                        required count: usize
                        optional omitted: String
                    }
                    /// The peer acknowledged the advertised ids.
                    ACKNOWLEDGED {
                        required peer: amaru_kernel::Peer
                        required ack: u16
                        required window: usize
                    }
                    /// A blocking RequestTxIds needs to wait until the mempool reaches `seq_no`.
                    WAIT_FOR_AT_LEAST {
                        required peer: amaru_kernel::Peer
                        required seq_no: u64
                        optional req: u16
                    }
                }
                responder {
                    /// Handle tx-submission responder stage messages
                    TX_SUBMISSION_RESPONDER_STAGE {
                        required message_type: String
                        required peer: amaru_kernel::Peer
                    }
                    /// Handle tx-submission responder protocol messages
                    TX_SUBMISSION_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                    /// The peer advertised transaction ids in a ReplyTxIds.
                    REPLY_TX_IDS_RECEIVED {
                        required peer: amaru_kernel::Peer
                        required count: usize
                    }
                    /// The peer delivered transaction bodies in a ReplyTxs.
                    REPLY_TXS_RECEIVED {
                        required peer: amaru_kernel::Peer
                        required count: usize
                    }
                    /// An advertised tx is already in our mempool: it will be acknowledged
                    /// without ever fetching its body.
                    SKIP_FETCH {
                        required peer: amaru_kernel::Peer
                        required tx_id: amaru_kernel::TransactionId
                    }
                    /// Request tx ids from the peer, acknowledging processed ones.
                    REQUEST_TX_IDS {
                        required peer: amaru_kernel::Peer
                        required ack: u16
                        required req: u16
                        required blocking: bool
                    }
                    /// Request tx bodies from the peer.
                    REQUEST_TXS {
                        required peer: amaru_kernel::Peer
                        required count: usize
                        required tx_ids: &[amaru_kernel::TransactionId]
                    }
                    /// Mempool near capacity: fetching is deferred until capacity frees up.
                    AWAITING_CAPACITY {
                        required peer: amaru_kernel::Peer
                        required pending: usize
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
        setup {
            observability {
                /// Observability stack initialization
                public INIT {
                    required with_open_telemetry: bool
                    required with_json_traces: bool
                    required with_colors: bool
                }
            }
            build {
                /// Running binary build/version identity (package version, git commit, target).
                public VERSION {
                    required version: String
                    required git_commit: String
                    required git_dirty: bool
                    required os: String
                    required arch: String
                }
            }
            trace {
                /// Resolution of a trace filter from the environment
                public FILTER {
                    required var: String
                    required value: String
                    required provided_by_user: bool
                    optional provided_invalid: bool
                    optional error: String
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
