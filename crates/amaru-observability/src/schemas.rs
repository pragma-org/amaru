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
//!         [public] span <SCHEMA> {
//!             parents: <path>, ...         // optional; omitted means a root span
//!             root                         // optional; with parents, also allow `debug_span!(root, SPAN)`
//!             tags: <tag>, ...             // optional; overrides inherited tags
//!             required <field>: <Type> [,]
//!             optional <field>: <Type> [,]
//!         }
//!         /// Description of the event
//!         [public] event <SCHEMA> {
//!             levels: <level>, ...         // required; tracing levels this event may use
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
//! uppercase letter (conventionally `SCREAMING_SNAKE_CASE`) and are introduced by `span` or
//! `event`. The category path determines:
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
//! `span` schemas are opened with `trace_span!` / `debug_span!` / `info_span!` and updated
//! with `trace_record!`. `event` schemas are emitted with `trace_event!` / `trace!` /
//! `debug!` / `info!` / `warn!` / `error!`. An event must declare `levels:` (one or more of
//! `trace`, `debug`, `info`, `warn`, `error`); a span must not. Emitting an event at a level
//! outside that list, using an event as a span, or recording onto an event is a compile error.
//!
//! A span may list `parents: path::to::Marker, other::MARKER`. `debug_span!(parent_context: &ctx, SPAN)`
//! then compiles only when `ctx` is `TraceContext<S>` and `S` is one of those markers.
//! `TraceContext::none()` uses [`NoParent`](crate::NoParent) and does not satisfy a listed parent.
//! A span that lists parents cannot be opened bare, with `root`, or under a raw `parent:` span.
//! `root` in the schema body opts that span into `debug_span!(root, SPAN)` as well. A span with
//! no `parents:` is a root: `debug_span!(SPAN)` and `debug_span!(root, SPAN)` still compile.
//! Events reject `parents:` and `root`.
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
//! (`amaru_kernel::Hash<28>`). Prefix the type with `%` (`Display`) or `?` (`Debug`) to
//! render as a string. Unformatted `String` accepts any `AsRef<str>`; primitives use typed
//! `tracing::Value`; other types must implement `Serialize + JsonSchema` and are encoded as
//! CBOR via `record_bytes`.
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
            chain_db_migration {
                /// Migrate the database if necessary
                public event EXECUTE {
                    levels: info
                    required from: u16
                    required to: u16
                }
                /// Migrate the chain database from the stored version to the current one
                public span MIGRATE {
                    required from: u16
                    required to: u16
                }
                /// A database migration relies on an assumption that may not hold; see the reason
                public event WARN {
                    levels: warn
                    /// Version the database is being migrated to
                    required to: u16
                    required reason: String
                }
                /// Reset the best chain to the anchor during migration so blocks are revalidated
                public event RESET_BEST_CHAIN {
                    levels: info
                    required prev_best_chain: amaru_kernel::HeaderHash
                    required new_best_chain: amaru_kernel::HeaderHash
                }
            }
            chain_db {
                tags: setup
                /// Open the database
                span OPEN {
                    required path: String
                }
                /// Initialize the store
                event INITIALIZE {
                    levels: info
                    required ledger_tip: amaru_kernel::Point
                    optional best_chain_hash: amaru_kernel::HeaderHash
                }
                /// Remove the valid status of descendants of a given block to reapply those blocks.
                event CLEAR_VALID_DESCENDANTS {
                    levels: debug
                    required count: usize
                }
            }
            blocks {
                /// Validate downloaded blocks that are not yet validated
                span RECOVER_STORED {
                    tags: setup
                    required from: amaru_kernel::Point
                    required to: amaru_kernel::HeaderHash
                }
                /// Fetch a range of blocks starting from the specified tip
                span FETCH {
                    parents: crate::ChainChoice
                    tags: cpu
                    required tip: amaru_kernel::Point
                    required header_hash: amaru_kernel::HeaderHash
                    optional parent: amaru_kernel::Point
                }
                /// Startup recovery found an inconsistent stored chain.
                /// Reason ∈ {ledger_tip_is_origin, broken_chain}.
                public event RECOVER_INCONSISTENT {
                    levels: error
                    required from: amaru_kernel::Point
                    required to: amaru_kernel::HeaderHash
                    required reason: String
                }
                /// Failed to check whether a stored block exists during startup recovery
                public event RECOVER_FAILED {
                    levels: error
                    required error: String
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// A header required for block fetching could not be loaded from the store
                public event HEADER_NOT_FOUND {
                    levels: error
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Begin replaying stored blocks up to the given tip during startup recovery
                event REPLAY {
                    levels: debug
                    tags: setup
                    required tip: amaru_kernel::Point
                }
                /// Resubmit one stored block for validation during startup recovery
                event REPLAY_BLOCK {
                    levels: debug
                    tags: setup
                    required point: amaru_kernel::Point
                }
                /// No missing-block boundary found for the new tip; nothing to fetch
                event NO_BOUNDARY {
                    levels: debug
                }
                /// Failed to compute the set of missing blocks
                public event FIND_MISSING_FAILED {
                    levels: error
                    required error: String
                }
                /// The batch of missing blocks is empty; resume fetching from the tip
                public event NOTHING_TO_FETCH {
                    levels: info
                    required tip: amaru_kernel::Point
                    required parent: amaru_kernel::Point
                }
                /// Request a batch of missing blocks from peers
                event REQUEST {
                    levels: debug
                    required from: amaru_kernel::Point
                    required through: amaru_kernel::Point
                    required length: usize
                }
                /// No covering peer set was selected; falling back to all initiating connections
                event WEAK_PEER_SELECTION {
                    levels: debug
                    required weak: bool
                }
                /// Failed to decode a block received from a peer
                public event DECODE_FAILED {
                    levels: warn, error
                    required peer: %amaru_kernel::Peer
                    required error: String
                }
                /// Received a block from a peer
                event RECEIVED {
                    levels: debug
                    required point: amaru_kernel::Point
                }
                /// Received a block while no batch is active (straggler)
                event STRAGGLER {
                    levels: debug
                    required peer: %amaru_kernel::Peer
                }
                /// Received a block whose parent does not match the batch boundary
                event PARENT_MISMATCH {
                    levels: debug
                    required expected: amaru_kernel::HeaderHash
                    required actual: amaru_kernel::HeaderHash
                }
                /// Received a block out of order: its point is not the next missing point
                public event POINT_MISMATCH {
                    levels: warn
                    optional expected: amaru_kernel::Point
                    required actual: amaru_kernel::Point
                }
                /// Failed to persist a downloaded block
                public event STORE_FAILED {
                    levels: error
                    required error: String
                }
                /// Block fetching paused because no upstream peers are available
                public event PAUSED {
                    levels: info
                    required req_id: u64
                }
                /// Retry block fetching after a no-peers pause
                event RETRY {
                    levels: debug
                    required req_id: u64
                }
                /// Timed out waiting for requested blocks
                public event TIMEOUT {
                    levels: debug, warn
                    required req_id: u64
                }
            }
            node {
                tags: setup
                /// Initialize the node
                span INITIALIZE {}
            }
            chain {
                /// Find chain intersection point with peer
                public span FIND_INTERSECTION {
                    tags: bootstrap
                    required peer: String
                    required intersection_slot: amaru_kernel::Slot
                }
                /// Received a new tip from an upstream peer
                public span SELECT_FROM_TIP {
                    parents: crate::ChainSyncProcess
                    tags: cpu
                    required tip: amaru_kernel::Point
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Received a block validation result
                public span SELECT_FROM_BLOCK_VALIDATION {
                    parents: crate::CarriedHeader
                    tags: cpu
                    required point: amaru_kernel::Point
                    required valid: bool
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Some blocks have been fetched for the current chain, decide what to do next
                public span FETCH_NEXT {
                    parents: crate::FetchResume
                    tags: cpu
                    required point: amaru_kernel::Point
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// A tip announced by an upstream peer was not adopted.
                /// Reason ∈ {already_validated, already_invalid, already_tracked, invalid_ancestor}.
                public event TIP_IGNORED {
                    levels: debug, info
                    required tip: amaru_kernel::Point
                    required reason: String
                    optional parent: amaru_kernel::Point
                }
                /// A tip announced by an upstream peer is new and starts or extends a chain.
                /// Outcome ∈ {new_tip, from_origin, extend, fork}.
                public event TIP_ACCEPTED {
                    levels: debug
                    required tip: amaru_kernel::Point
                    required outcome: String
                    optional parent: amaru_kernel::Point
                }
                /// A block was validated successfully, so the chains awaiting it advance past it.
                ///
                /// The operator-facing counterpart is `consensus::tip::ADOPT`; this records the
                /// bookkeeping chain selection does with the result.
                event BLOCK_VALIDATED {
                    levels: debug
                    required tip: amaru_kernel::Point
                    /// How many tracked chains had their pending prefix advanced past this block
                    required advanced: usize
                    /// Whether the node is still catching up, where per-block timings are not
                    /// a meaningful network-health signal
                    required syncing: bool
                }
                /// A new candidate was chosen as the best tip.
                /// Reason ∈ {better_chain, previous_invalidated}.
                public event BEST_TIP_CANDIDATE {
                    levels: debug
                    required tip: amaru_kernel::Point
                    required reason: String
                    optional previous: amaru_kernel::Point
                }
                /// The best tip candidate was invalidated and forks depending on it were dropped
                public event BEST_TIP_INVALIDATED {
                    levels: info
                    required removed: usize
                }
                /// Chain forks were removed because they depend on an invalid block
                public event FORKS_REMOVED {
                    levels: warn
                    required removed: usize
                }
                /// No valid candidate remains; the best chain falls back to origin
                public event FALLBACK_TO_ORIGIN {
                    levels: warn
                }
                /// Failed to select a new best candidate after an invalidation
                public event FIND_BEST_CANDIDATE_FAILED {
                    levels: error
                    required error: String
                }
                /// Where block fetching resumes from, once per request.
                /// Outcome ∈ {resume_from_best_tip, already_at_best_tip, no_best_tip}; only
                /// `resume_from_best_tip` sends a tip downstream and carries its `parent`.
                public event RESUME_FETCH {
                    levels: debug
                    required outcome: String
                    required point: amaru_kernel::Point
                    required best_tip: amaru_kernel::Point
                    optional parent: amaru_kernel::Point
                }
                /// A header needed for chain selection could not be loaded from the store.
                /// Role ∈ {tip, best_candidate, best_candidate_parent, parent, validation_target}.
                public event HEADER_NOT_FOUND {
                    levels: warn, error
                    required role: String
                    required header_hash: amaru_kernel::HeaderHash
                    optional tip: amaru_kernel::Point
                }
                /// Failed to persist the validation result of a block
                public event STORE_VALIDATION_FAILED {
                    levels: error
                    required error: String
                    required valid: bool
                }
            }
            performance {
                /// The performance worker thread stopped because it panicked
                public event WORKER_PANICKED {
                    levels: error
                    required error: String
                }
                /// The performance operation queue is growing faster than the worker drains it
                public event QUEUE_LAGGING {
                    levels: warn
                    required queue_depth: u64
                }
                /// The performance operation queue exceeded its hard limit; the node aborts
                public event QUEUE_OVERFLOW {
                    levels: error
                    required queue_depth: u64
                    required threshold: u64
                }
            }
            best_tip_candidate {
                /// Walk the stored block tree to find the best candidate tip
                span SEARCH {
                    required anchor: amaru_kernel::HeaderHash
                    optional visited: usize
                    optional best_candidate: amaru_kernel::HeaderHash
                }
                /// A stored block was skipped while searching because it is invalid
                event SKIP_INVALID {
                    levels: debug
                    required header_hash: amaru_kernel::HeaderHash
                }
            }
            block_source {
                /// Forget tracked blocks that fell too far behind the adopted tip
                span PRUNE {
                    optional pruned: usize
                    optional retained: usize
                }
                /// A peer announced a block body
                event RECEIVED {
                    levels: debug
                    required peer: %amaru_kernel::Peer
                    required point: amaru_kernel::Point
                }
                /// A peer announced a block already known to be invalid
                public event KNOWN_INVALID {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required point: amaru_kernel::Point
                }
                /// A block validation result was recorded against its known sources
                event VALIDATION {
                    levels: debug
                    required point: amaru_kernel::Point
                    required valid: bool
                }
            }
            chainsync {
                /// A chainsync session with an upstream peer was initialized
                public event INITIALIZED {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required conn_id: u64
                }
                /// A chainsync session was re-initialized while still active; prior state is purged
                public event REINITIALIZED {
                    levels: warn
                    required peer: %amaru_kernel::Peer
                    required conn_id: u64
                }
                /// A chainsync session terminated and its connection state was purged
                public event TERMINATED {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required conn_id: u64
                }
                /// An intersection with the peer's chain was found
                public event INTERSECT_FOUND {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required conn_id: u64
                    required current: amaru_kernel::Point
                    required highest: amaru_kernel::Point
                }
                /// No intersection with the peer's chain was found, so chainsync with it stops
                public event INTERSECT_NOT_FOUND {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required highest: amaru_kernel::Point
                }
                /// The peer intersected on a point absent from our own store, so chainsync with it
                /// stops. Unlike `INTERSECT_NOT_FOUND` this points at local state, not at the peer.
                public event UNKNOWN_INTERSECTION_POINT {
                    levels: warn
                    required peer: %amaru_kernel::Peer
                    required current: amaru_kernel::Point
                    required highest: amaru_kernel::Point
                }
                /// A header was announced by a peer
                event ROLL_FORWARD {
                    levels: trace
                    required peer: %amaru_kernel::Peer
                    required variant: String
                    required highest: amaru_kernel::Point
                }
                /// A header announced by a peer was processed.
                /// Outcome ∈ {already_stored, stored}.
                event ROLL_FORWARD_DONE {
                    levels: debug
                    required peer: %amaru_kernel::Peer
                    required current: amaru_kernel::Point
                    required highest: amaru_kernel::Point
                    required outcome: String
                }
                /// A peer rolled back to an earlier point
                public event ROLL_BACKWARD {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required current: amaru_kernel::Point
                    required highest: amaru_kernel::Point
                }
                /// A rollback requested by a peer could not be applied; the peer is adversarial
                public event ROLL_BACKWARD_FAILED {
                    levels: error
                    required peer: %amaru_kernel::Peer
                    required error: String
                }
                /// Near-now headers have been arriving for a minute and the adopted tip is not
                /// getting closer to the wall clock. Sync that is still adopting faster than 10
                /// blocks per second does not raise this. Emitted at most once a minute.
                public event CHAIN_LAGGING {
                    levels: error
                    required peer: %amaru_kernel::Peer
                    required live_slot: amaru_kernel::Slot
                    required our_slot: amaru_kernel::Slot
                    required lag: i64
                }
                /// A header's validation is held back until what blocks it resolves.
                /// Reason ∈ {ledger_height, stake_distribution, clock_skew, follow_up}; the height
                /// fields are present for `ledger_height`, where they say how far behind we are.
                event HEADER_DEFERRED {
                    levels: debug
                    required peer: %amaru_kernel::Peer
                    required reason: String
                    required header_hash: amaru_kernel::HeaderHash
                    optional header_height: u64
                    optional ledger_height: u64
                    optional limit: u64
                }
            }
            roll_forward {
                tags: cpu
                /// Received a new tip to roll forward
                span PROCESS {
                    required tip: amaru_kernel::Point
                    required peer: %amaru_kernel::Peer
                    optional header_hash: amaru_kernel::HeaderHash
                }
            }
            roll_backward {
                tags: cpu
                /// Received a header to rollback
                span PROCESS {
                    required current: amaru_kernel::Point
                    required tip: amaru_kernel::Point
                    required peer: %amaru_kernel::Peer
                }
            }
            header {
                tags: cpu
                /// Decode header from raw bytes
                span DECODE {
                    required peer: %amaru_kernel::Peer
                }
                /// Validate the whole header
                span VALIDATE {
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Evolve the nonce based on header
                span EVOLVE_NONCE {
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// Check header cryptographic properties
                span CHECK {
                    required issuer_key: amaru_kernel::VerificationKey
                }
                /// Forward to a downstream peer
                span FORWARD {
                    parents: crate::CarriedHeader
                    required tip: amaru_kernel::Point
                    required peer: %amaru_kernel::Peer
                }
            }
            block {
                tags: cpu
                /// Validate a block by applying it to the current ledger
                span VALIDATE {
                    parents: crate::CarriedHeader
                    required tip: amaru_kernel::Point
                    required header_hash: amaru_kernel::HeaderHash
                    optional valid: bool
                    /// Ledger tip the block is applied on top of
                    optional current: amaru_kernel::Point
                    optional parent: amaru_kernel::Point
                }
                /// Skip a block validation when it is not better than the current ledger tip
                public event SKIP {
                    levels: debug
                    required current: amaru_kernel::Point
                    required tip: amaru_kernel::Point
                }
                /// Adopt a block as the next block in the best chain
                span ADOPT {
                    parents: crate::CarriedHeader
                    required tip: amaru_kernel::Point
                    required header_hash: amaru_kernel::HeaderHash
                }
                /// A tip was not adopted as the new best chain.
                /// Reason ∈ {shorter_than_best, not_better_than_best}.
                event ADOPT_SKIPPED {
                    levels: debug
                    required tip: amaru_kernel::Point
                    required reason: String
                    optional current_best_tip: amaru_kernel::Point
                }
                /// Adopting a tip as the new best chain failed.
                /// Step ∈ {adopt_tip, adopt_first_tip, drag_anchor_forward}.
                public event ADOPT_FAILED {
                    levels: error
                    required tip: amaru_kernel::Point
                    required step: String
                    required error: String
                }
                /// The chain store contradicts itself while adopting a tip.
                /// Invariant ∈ {header_missing, no_common_ancestor}.
                public event INVARIANT_VIOLATED {
                    levels: error
                    required tip: amaru_kernel::Point
                    required invariant: String
                }
                /// A header needed to adopt a tip could not be loaded.
                /// Role ∈ {incoming_tip, current_best}.
                public event HEADER_NOT_FOUND {
                    levels: warn
                    required role: String
                    optional tip: amaru_kernel::Point
                }
                /// Block validation cannot proceed because the parent is the genesis block
                public event VALIDATE_FROM_GENESIS {
                    levels: error
                    required tip: amaru_kernel::Point
                    required current: amaru_kernel::Point
                    required parent: amaru_kernel::Point
                }
                /// A block could not be applied to the ledger.
                /// Step ∈ {validate_block, switch_to_fork}.
                public event APPLY_FAILED {
                    levels: warn
                    required tip: amaru_kernel::Point
                    required step: String
                    required error: String
                }
                /// A block was rejected during validation
                public event INVALID {
                    levels: warn
                    required failed_tip: amaru_kernel::Point
                    required parent: amaru_kernel::Point
                    required error: String
                    /// Human-readable context on where the rejection came from
                    required detail: String
                }
                /// The ledger is switching to a different fork
                public event SWITCH_FORK {
                    levels: info
                    required current: amaru_kernel::Point
                    required parent: amaru_kernel::Point
                }
                /// Mismatched body hash after download, the peer is adversarial
                public event MISMATCHED_HASH {
                    levels: warn
                    required peer: %amaru_kernel::Peer
                    required header_hash: amaru_kernel::HeaderHash
                    optional expected: amaru_kernel::Hash<32>
                    optional actual: amaru_kernel::Hash<32>
                }
            }
            tip {
                /// The node switched between catching up and live.
                /// `mode` and `previous` ∈ {sync, live}.
                public event MODE {
                    levels: info
                    required mode: String
                    required previous: String
                    required slot: amaru_kernel::Slot
                }
                /// Adopt a tip as the next tip in the best chain
                public event ADOPT {
                    levels: debug, info
                    required slot: amaru_kernel::Slot
                    required header_hash: amaru_kernel::HeaderHash
                    required block_height: u64
                    required max_block_height: u64
                    required suppressed: u32
                }
            }
            forge {
                /// A led slot was not forged.
                /// Reason ∈ {ocert_not_yet_valid, ocert_expired, tip_ahead, not_led, woke_late}.
                public event MISSED_SLOT {
                    levels: warn
                    required slot: amaru_kernel::Slot
                    required reason: String
                }
                /// Forging the header or storing it failed. The node shuts down.
                /// Step ∈ {sign_header, validate_header, store_header, store_block}.
                public event FORGE_FAILED {
                    levels: error
                    required slot: amaru_kernel::Slot
                    required step: String
                    required error: String
                }
                /// Leader schedules still held, with how many led slots remain in each epoch
                /// and how many of k blocks since freeze have been adopted.
                /// `next_slot` is the UTC onset of the next armed led slot, `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
                public event SCHEDULE {
                    levels: info
                    required slots: std::collections::BTreeMap<amaru_kernel::Epoch, usize>
                    optional next_slot: String
                    required freeze_depth: u64
                    required settled: bool
                }
                /// A block was forged and stored, and its tip sent to chain selection.
                public event FORGED {
                    levels: info
                    required slot: amaru_kernel::Slot
                    required header_hash: amaru_kernel::HeaderHash
                    required parent: amaru_kernel::HeaderHash
                }
            }
            peer {
                tags: cpu
                /// A peer behaves like an adversary, ban it
                span BAN {
                    parents: crate::CarriedHeader
                    required peer: %amaru_kernel::Peer
                }
            }
            perf {
                header {
                    /// Event recorded once per header, when its processing reaches a terminal state.
                    /// The four network-health points themselves are the `amaru::blockperf` events
                    /// (`header.announced`, `block.requested`, `block.received`, `block.adopted`).
                    /// A header rejected on reception is logged at error and carries no durations.
                    /// A completed header stays at debug. The optional durations are the
                    /// `amaru::network` span durations, not a separate clock:
                    /// - `forward_micros`: `perf.header.forward`
                    /// - `block_fetch_wait_micros`: `perf.header.block_fetch_wait`
                    /// - `block_fetch_micros`: `perf.blocks.fetch`
                    /// - `adopt_micros`: body reception to adoption
                    public event LIFECYCLE {
                        levels: debug, error
                        optional peer: %amaru_kernel::Peer
                        optional header_hash: amaru_kernel::HeaderHash
                        optional outcome: String
                        optional error: String
                        optional slot_start_to_header_micros: u64
                        optional block_fetch_wait_micros: u64
                        optional block_fetch_micros: u64
                        optional forward_micros: u64
                        optional adopt_micros: u64
                    }
                }
                fork {
                    /// Event recorded when a fork switch ends. `duration_micros` is the
                    /// `amaru::network` span `perf.fork.switch`.
                    public event SWITCH {
                        levels: debug
                        required header_hash: amaru_kernel::HeaderHash
                        optional outcome: String
                        optional duration_micros: u64
                    }
                }
            }
        }
        blockperf {
            header {
                /// One of the first three distinct peers to announce this header while it is still
                /// being collected. A header that is already stored does not start a new line, and
                /// a header that has been adopted is not announced again.
                /// `rank` is 1, 2, or 3 in arrival order. Later peers are not logged.
                /// `slot_latency_ms` is milliseconds since the onset of this block's slot.
                public event ANNOUNCED {
                    levels: debug, info
                    required peer: %amaru_kernel::Peer
                    required header_hash: amaru_kernel::HeaderHash
                    required rank: u64
                    optional slot_latency_ms: u64
                }
            }
            block {
                /// Peers asked to fetch this block body. `peers` is a comma-separated list of
                /// socket addresses, sorted.
                /// `slot_latency_ms` is milliseconds since the onset of this block's slot.
                public event REQUESTED {
                    levels: debug, info
                    required header_hash: amaru_kernel::HeaderHash
                    required peers: String
                    optional slot_latency_ms: u64
                }
                /// A distinct peer delivered this block body.
                /// `rank` is 1 for the first delivery, then 2, 3, … in arrival order.
                /// `slot_latency_ms` is milliseconds since the onset of this block's slot.
                /// `fetch_latency_ms` is milliseconds since the request was sent to this peer.
                public event RECEIVED {
                    levels: debug, info
                    required peer: %amaru_kernel::Peer
                    required header_hash: amaru_kernel::HeaderHash
                    required rank: u64
                    optional slot_latency_ms: u64
                    optional fetch_latency_ms: u64
                }
                /// The block was adopted locally.
                /// `peer` is the first peer that delivered the body, when a delivery was recorded.
                /// `slot_latency_ms` is milliseconds since the onset of this block's slot.
                public event ADOPTED {
                    levels: debug, info
                    required header_hash: amaru_kernel::HeaderHash
                    optional peer: %amaru_kernel::Peer
                    optional slot_latency_ms: u64
                }
            }
        }
        ledger {
            tags: cpu
            state {
                /// Roll forward with a new block
                public span ROLL_FORWARD {}
                /// Roll backward to a specific point
                public span ROLL_BACKWARD {}
                /// Switching to an alternative chain fork
                public span SWITCH_TO_FORK {
                    required fork_point: amaru_kernel::Point
                    required fork_length: usize
                    required rollback_length: usize
                    optional outcome: String
                    // In case of an error, this says if the stable was impacted
                    optional stable_modified: bool
                }
                /// Forward ledger state with new volatile state
                public span PUSH {}
            }
            tip {
                /// Updated view of the locally adopted chain tip and its derived ledger health.
                public event UPDATE {
                    levels: debug
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
                public event INITIAL_BEGIN {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                }
                /// Report progress for one of the initial stake distributions loaded on startup
                public event INITIAL_PROGRESS {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required progress: f64
                }
                /// Finished computing all initial stake distributions loaded on startup
                public event INITIAL_READY {
                    levels: info
                    required epochs: String
                }
                /// Compute stake distribution for epoch
                public span COMPUTE {
                    required epoch: amaru_kernel::Epoch
                }
                /// Rotate stake distributions at an epoch boundary
                public event ROTATE {
                    levels: info
                    required available_stake_distributions: String
                }
                /// Snapshot of the stake distribution taken at an epoch boundary
                public event SNAPSHOT {
                    levels: info
                    required accounts: usize
                    required dreps: usize
                    required pools: usize
                    required active_stake: amaru_kernel::Lovelace
                    required pools_voting_stake: amaru_kernel::Lovelace
                    required dreps_voting_stake: amaru_kernel::Lovelace
                    optional cc_update: %amaru_kernel::ConstitutionalCommitteeUpdate
                }
            }
            rewards {
                /// Compute rewards for epoch
                public span COMPUTE {
                    required for_epoch: amaru_kernel::Epoch
                    required using_stake_distribution_from_epoch: amaru_kernel::Epoch
                }
                /// Summary of the rewards calculation for an epoch
                public event SUMMARIZE {
                    levels: info
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
                public span APPLY {
                    required point_slot: amaru_kernel::Slot
                }
                /// Prepare block for validation
                public span PREPARE {}
            }
            transaction {
                /// Validate a single transaction
                public span VALIDATE {
                    required id: amaru_kernel::TransactionId,
                }

                script {
                    /// A single script execution, with the associated redeemer qualifiers
                    public span EXECUTE {
                        required purpose: %amaru_kernel::RedeemerTag
                        required index: u32
                        optional acquire_arena_micros: u64
                        optional decode_script_micros: u64
                        optional build_uplc_program_micros: u64
                        optional evaluate_uplc_program_micros: u64
                    }
                }
            }
            rules {
                /// Block-related rules and other preflight checks
                public span BLOCK {}

                /// All phase one validations
                public span PHASE_ONE {
                    /// Ledger rules related to size, metadata and 'global' preflight checks
                    optional preflight_micros: u64
                    /// Ledger rules and state-transitions for certificates
                    optional certificates_micros: u64
                    /// Ledger rules and state-transitions for collateral
                    optional collateral_micros: u64
                    /// Ledger rules and state-transitions for collateral return
                    optional collateral_return_micros: u64
                    /// Ledger rules and state-transitions for treasury donation
                    optional donation_micros: u64
                    /// Ledger rules and state-transitions for fees
                    optional fees_micros: u64
                    /// Ledger rules and state-transitions for inputs
                    optional inputs_micros: u64
                    /// Ledger rules and state-transitions for metadata
                    optional metadata_micros: u64
                    /// Ledger rules and state-transitions for minted/burned assets
                    optional mint_micros: u64
                    /// Ledger rules and state-transitions for outputs
                    optional outputs_micros: u64
                    /// Ledger rules and state-transitions for governance proposals
                    optional proposals_micros: u64
                    /// Ledger rules and state-transitions for script witnesses
                    optional scripts_micros: u64
                    /// Ledger rules and state-transitions for key signatures
                    optional signatures_micros: u64
                    /// Ledger rules and state-transitions for validity interval
                    optional validity_interval_micros: u64
                    /// Ledger rules and state-transitions for governance votes
                    optional votes_micros: u64
                    /// Ledger rules and state-transitions for withdrawals
                    optional withdrawals_micros: u64
                }

                /// Initialize script context and cost models for phase-2 validations, common to all scripts
                public span PHASE_TWO {
                    optional script_context_micros: u64
                }
            }
            block_validation_context {
                /// Create validation context for a block
                public span CREATE {
                    required block_id: amaru_kernel::HeaderHash
                    required block_number: u64
                    required block_body_size: u64
                    optional total_inputs: u64
                }
            }
            transaction_validation_context {
                /// Create validation context for a transaction
                public span CREATE {
                    required id: amaru_kernel::TransactionId
                }
            }
            validation_context {
                inputs {
                    /// Resolve transaction inputs from the volatile db or the stable one
                    public span HYDRATE {
                        optional from_volatile: u64
                        optional from_db: u64
                    }
                }
                pools {
                    /// Resolve pools from the volatile db or the stable one
                    public span HYDRATE {
                        optional from_volatile: u64
                        optional from_db: u64
                    }
                }
                accounts {
                    /// Resolve accounts from the volatile db or the stable one
                    public span HYDRATE {
                        optional from_volatile: u64
                        optional from_db: u64
                    }
                }
                dreps {
                    /// Resolve dreps from the volatile db or the stable one
                    public span HYDRATE {
                        optional from_volatile: u64
                        optional from_db: u64
                    }
                }
                committee {
                    /// Resolve committee members from the volatile db or the stable one
                    public span HYDRATE {}
                }
                proposals {
                    /// Resolve proposals from the volatile db or the stable one
                    public span HYDRATE {
                        optional from_volatile: u64
                        optional from_db: u64
                    }
                }
            }
            relays {
                /// Fetch candidate relays from the immutable store
                public span COLLECT {
                    optional count: String
                }
            }
            epoch_transition {
                /// Epoch transition processing
                public span COMPUTE {
                    required from: amaru_kernel::Epoch
                    required into: amaru_kernel::Epoch
                    optional skipped: bool
                    optional resuming_from: String
                }
                /// Create pools updates
                public span NEW_POOLS_UPDATES {}
                /// Create governance updates (i.e. ratify proposals) at an epoch boundary.
                public span NEW_GOVERNANCE_UPDATES {
                    /// Total number of proposals in scope. This also includes proposals that have
                    /// *just* been submitted.
                    required proposals_count: u64
                }
                /// Flushing the epoch transition overlay to disk
                public span APPLY {
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
                public event TICK_POOL {
                    levels: debug
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
                public event RETIRE_POOL {
                    levels: debug
                    required id: amaru_kernel::PoolId
                }
                /// Rollback an in-flight epoch transition
                public event ROLLBACK {
                    levels: debug
                    required from: amaru_kernel::Epoch
                    required to: amaru_kernel::Epoch
                }
                /// Record an in-flight epoch transition
                public event RECORD {
                    levels: debug
                    required from: amaru_kernel::Epoch
                    required to: amaru_kernel::Epoch
                }
            }
            governance {
                /// Create ratification context
                public span NEW_RATIFICATION_CONTEXT {
                    /// Epoch to ratify; distinct from the actual epoch this calculation is happening.
                    required ratifying_epoch: amaru_kernel::Epoch
                    /// Value of the treasury considered for this ratification round.
                    optional treasury: amaru_kernel::Lovelace
                    /// Total number of votes to ratify.
                    optional votes: u64
                }
                /// Ratify proposals at epoch boundary
                public span RATIFY_PROPOSALS {
                    required epoch: amaru_kernel::Epoch
                    optional roots_protocol_parameters: String
                    optional roots_hard_fork: String
                    optional roots_constitutional_committee: String
                    optional roots_constitution: String
                }
                /// Ratify a proposal while traversing the governance forest
                public span RATIFYING {
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
                public span ENACTING {
                    required proposal_id: String
                    required proposal_kind: String
                    optional pruned_relatives: String
                }
            }
            volatile {
                /// Recompute the volatile aggregate
                public span AGGREGATE {}
                /// The volatile db is still warming up and hasn't reached a stable point yet
                public event WARM_UP {
                    levels: trace
                    required size: usize
                }
            }
            account {
                /// Pay withdrawals to an account, or refund its deposit
                public event PAY_OR_REFUND {
                    levels: debug
                    required credential_type: %amaru_kernel::CredentialKind
                    required account: amaru_kernel::Hash<28>
                    required deposit: amaru_kernel::Lovelace
                }
            }
            chain_growth {
                /// Fewer than k blocks were seen within the stability window
                public event VIOLATE {
                    levels: warn
                    required unstable_tail_length: usize
                    required reason: String
                }
            }
            constitutional_committee {
                /// The constitutional committee votes were ignored during ratification
                public event IGNORE {
                    levels: warn
                    required active_members: usize
                    required min_committee_size: u16
                    required reason: String
                }
                /// Load the current constitutional committee on startup
                public span DUMP {
                    required status: %amaru_kernel::ConstitutionalCommitteeStatus
                }
            }
            constitutional_committee_member {
                /// Load the current constitutional committee member on startup
                public event DUMP {
                    levels: info
                    required cold_credential: %amaru_kernel::Credential
                    optional status: %amaru_kernel::ConstitutionalCommitteeMemberStatus
                    optional valid_until: amaru_kernel::Epoch
                }
            }
            governance_activity {
                /// Update the number of consecutive dormant epochs
                public event UPDATE {
                    levels: debug
                    required consecutive_dormant_epochs: u32
                }
            }
            pots {
                /// Load the current ledger pots
                public event DUMP {
                    levels: info
                    required treasury: amaru_kernel::Lovelace
                    required reserves: amaru_kernel::Lovelace
                    required fees: amaru_kernel::Lovelace
                    required donations: amaru_kernel::Lovelace
                }
            }
            overlay {
                /// No pools updates found in the epoch transition overlay
                public event NO_POOLS_UPDATES {
                    levels: debug
                }
                /// No governance updates found in the epoch transition overlay
                public event NO_GOVERNANCE_UPDATES {
                    levels: debug
                }
            }
            proposal {
                /// Observe a governance proposal that is currently active
                public event ACTIVE {
                    levels: info
                    required id: String
                    required proposal_kind: String
                    required proposed_in: amaru_kernel::Epoch
                    required valid_until: amaru_kernel::Epoch
                    optional detail: String
                }
                /// Drop an expired or ratified governance proposal
                public event DROP {
                    levels: info
                    required id: String
                    required expired: bool
                    required ratified_or_evicted: bool
                }
                /// Skip a governance proposal during ratification
                public event SKIP {
                    levels: debug
                    required id: %amaru_kernel::ProposalId
                    required reason: String
                    optional proposed_in: amaru_kernel::Epoch
                    optional ratifying_epoch: amaru_kernel::Epoch
                    optional withdrawal: amaru_kernel::Lovelace
                    optional treasury: amaru_kernel::Lovelace
                    optional invalid_members: String
                }
            }
            proposal_roots {
                /// Summary of the governance proposal roots after ratification
                public event SUMMARIZE {
                    levels: debug
                    optional constitution: String
                    optional constitutional_committee: String
                    optional hard_fork: String
                    optional protocol_parameters: String
                }
            }
            protocol {
                /// Upgrade to a new protocol version
                public event UPGRADE {
                    levels: info
                    required old_version: u64
                    required new_version: u64
                }
            }
            protocol_parameters {
                /// Dump the current protocol parameters
                public event DUMP {
                    levels: info
                    optional protocol_version: %amaru_kernel::ProtocolVersion
                    optional max_block_body_size: u64
                    optional max_transaction_size: u64
                    optional max_block_header_size: u16
                    optional max_tx_ex_units: %amaru_kernel::ExUnits
                    optional max_block_ex_units: %amaru_kernel::ExUnits
                    optional max_value_size: u64
                    optional max_collateral_inputs: u16
                    optional min_fee_a: amaru_kernel::Lovelace
                    optional min_fee_b: u64
                    optional stake_credential_deposit: amaru_kernel::Lovelace
                    optional stake_pool_deposit: amaru_kernel::Lovelace
                    optional monetary_expansion_rate: %amaru_kernel::RationalNumber
                    optional treasury_expansion_rate: %amaru_kernel::RationalNumber
                    optional min_pool_cost: amaru_kernel::Lovelace
                    optional lovelace_per_utxo_byte: amaru_kernel::Lovelace
                    optional prices: %amaru_kernel::ExUnitPrices
                    optional min_fee_ref_script_lovelace_per_byte: %amaru_kernel::RationalNumber
                    optional max_ref_script_size_per_tx: u32
                    optional max_ref_script_size_per_block: u32
                    optional ref_script_cost_stride: u32
                    optional ref_script_cost_multiplier: %amaru_kernel::RationalNumber
                    optional stake_pool_max_retirement_epoch: u64
                    optional optimal_stake_pools_count: u16
                    optional pledge_influence: %amaru_kernel::RationalNumber
                    optional cost_models: %amaru_kernel::CostModels
                    optional collateral_percentage: u16
                    optional pool_voting_thresholds: %amaru_kernel::PoolVotingThresholds
                    optional drep_voting_thresholds: %amaru_kernel::DRepVotingThresholds
                    optional min_committee_size: u16
                    optional max_committee_term_length: u64
                    optional gov_action_lifetime: u64
                    optional gov_action_deposit: amaru_kernel::Lovelace
                    optional drep_deposit: amaru_kernel::Lovelace
                    optional drep_expiry: u64
                }
            }
            ratification {
                /// Summary of the outcome of a ratification round
                public event SUMMARIZE {
                    levels: info
                    required is_dormant_epoch: bool
                    optional pruned_proposals: String
                    optional refunds: String
                    optional withdrawals: String
                    optional new_constitution: String
                    optional constitutional_committee_update: String
                }
                /// Skip the remaining proposals for this epoch
                public event SKIP {
                    levels: info
                    required reason: String
                }
            }
        }
        bootstrap {
            /// Bootstrap completed successfully
            public event COMPLETE {
                levels: info
                required duration_seconds: f64
                required epoch: amaru_kernel::Epoch
                required point: String
            }
            accounts {
                /// Existing accounts found in the store before import
                public event IS_NOT_EMPTY {
                    levels: warn
                }
                /// Import accounts from a snapshot
                public event IMPORT {
                    levels: info
                    required size: usize
                }
            }
            block_issuers {
                /// Import block issuers from a snapshot
                public event IMPORT {
                    levels: info
                    required count: u64
                }
            }
            constitution {
                /// Import the constitution from a snapshot
                public event IMPORT {
                    levels: info
                    required anchor: String
                    required guardrails: String
                }
            }
            constitutional_committee {
                /// Import the constitutional committee from a snapshot
                public event IMPORT {
                    levels: info
                    required state: String
                    optional threshold: String
                    optional members: usize
                }
            }
            dreps {
                /// Import DReps from a snapshot
                public event IMPORT {
                    levels: info
                    required size: usize
                }
            }
            fetch {
                /// Received a rollback while fetching bootstrap headers
                public event ROLLBACK {
                    levels: info
                    required point: %amaru_kernel::NetworkPoint
                    required tip: amaru_kernel::Point
                }
            }
            governance_activity {
                /// Import the governance activity from a snapshot
                public event IMPORT {
                    levels: info
                    required dormant_epochs: u32
                }
            }
            header {
                /// Import a single header into the chain store
                public event IMPORT {
                    levels: info
                    required header: amaru_kernel::HeaderHash
                }
            }
            headers {
                /// Fetch bootstrap headers from a peer
                public event FETCH {
                    levels: info
                    required requested_point: %amaru_kernel::NetworkPoint
                    required intersection: %amaru_kernel::NetworkPoint
                    required headers_per_point: usize
                }
                /// The chain-sync client failed while requesting or awaiting the next header.
                /// Operation ∈ {request_next, await_next}.
                public event NEXT_FAILED {
                    levels: error
                    required operation: String
                    required error: String
                }
            }
            import {
                /// Import UTxO entries from a snapshot
                public event UTXO {
                    levels: info
                    required size: usize
                }
            }
            nonces {
                /// Import initial nonces into the chain store
                public event IMPORT {
                    levels: info
                    required point: amaru_kernel::Point
                }
            }
            opcert_sequence_numbers {
                /// Import initial opcert sequence numbers into the chain store
                public event IMPORT {
                    levels: info
                    required point: amaru_kernel::Point
                }
            }
            peer {
                /// Failed to connect to a peer while bootstrapping
                public event FAILED_TO_CONNECT {
                    levels: error
                    required peer: String
                    required reason: String
                }
            }
            pots {
                /// Import treasury/reserves/fees pots from a snapshot
                public event IMPORT {
                    levels: info
                    required treasury: amaru_kernel::Lovelace
                    required reserves: amaru_kernel::Lovelace
                    required fees: amaru_kernel::Lovelace
                    required donations: amaru_kernel::Lovelace
                }
            }
            proposal_roots {
                /// Import governance proposal roots from a snapshot
                public event IMPORT {
                    levels: info
                    required constitution: String
                    required constitutional_committee: String
                    required hard_fork: String
                    required protocol_parameters: String
                }
            }
            progress {
                /// Enter a canonical bootstrap stage
                public event STAGE {
                    levels: info
                    required stage: String
                }
                /// Report the selected snapshot window and its aggregate compressed size
                public event SNAPSHOTS_SELECTED {
                    levels: info
                    required snapshot_count: usize
                    optional total_bytes: u64
                }
                /// Report absolute aggregate snapshot download progress
                public event DOWNLOAD {
                    levels: info
                    required downloaded_bytes: u64
                    required completed_snapshots: usize
                }
                /// Report successful bootstrap completion
                public event COMPLETE {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required point: String
                }
            }
            proposals {
                /// Existing proposals found in the store before import
                public event IS_NOT_EMPTY {
                    levels: warn
                }
                /// Import governance proposals from a snapshot
                public event IMPORT {
                    levels: info
                    required size: usize
                }
            }
            recently_pruned_proposals {
                /// Import proposals pruned at the snapshot's epoch boundary, from its ratify state
                public event IMPORT {
                    levels: info
                    required size: usize
                }
            }
            snapshot {
                /// Download a snapshot archive
                public event DOWNLOAD {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required point: String
                }
                /// Snapshot already downloaded; skipping download
                public event SKIP_DOWNLOAD {
                    levels: info
                    required snapshot: String
                }
                /// Import a compressed snapshot archive
                public event IMPORT_ARCHIVE {
                    levels: info
                    required path: String
                }
                /// Import from the tvar data
                public event IMPORT_TVAR {
                    levels: info
                    required point: amaru_kernel::Point
                    required new_epoch_state_offset: usize
                }
                /// The parsed snapshot's current era is not Conway; later decoding may fail
                public event UNEXPECTED_ERA {
                    levels: warn
                    required snapshot_era: %amaru_kernel::EraName
                }
            }
            snapshots {
                /// Import all snapshots
                public event IMPORT {
                    levels: info
                    required count: usize
                }
            }
            stake_pools {
                /// Import stake pools from a snapshot
                public event IMPORT {
                    levels: info
                    required registered: usize
                    required retiring: usize
                }
            }
            votes {
                /// Import governance votes from a snapshot
                public event IMPORT {
                    levels: info
                    required size: usize
                }
            }
        }
        cli {
            /// Process terminated with an error.
            public event ERROR {
                levels: error
                required description: String
                optional cause: String
            }
            dev {
                tags: cli
                /// A developer command started, with the arguments it resolved.
                /// Command names the subcommand, e.g. "dev chain prune".
                public event RUN {
                    levels: info
                    required command: String
                    required network: %amaru_kernel::NetworkName
                    optional chain_dir: String
                    optional ledger_dir: String
                    optional headers_dir: String
                    optional input: String
                    optional start: String
                    optional block: String
                    optional parent: String
                    optional peer_address: String
                    optional epoch: String
                    optional count: usize
                    optional from_point: String
                    optional only_blocks: bool
                    optional only_validation_results: bool
                    /// Extra guidance for the operator, when a better command exists
                    optional hint: String
                }
                chain {
                    /// The pruning boundary derived from the oldest ledger snapshot
                    public event PRUNE_BOUNDARY {
                        levels: info
                        required oldest_ledger_epoch: u64
                        required boundary_slot: u64
                    }
                    /// The chain store anchor was moved to a new hash
                    public event ANCHOR_UPDATED {
                        levels: info
                        required new_anchor: amaru_kernel::HeaderHash
                    }
                    /// The chain database is already at the current version
                    public event MIGRATION_NOT_NEEDED {
                        levels: info
                    }
                    /// The chain database could not be opened
                    public event OPEN_FAILED {
                        levels: error
                        required error: String
                    }
                    /// The number of stored points selected for removal
                    public event POINTS_TO_REMOVE {
                        levels: info
                        required points: usize
                    }
                    /// The best chain hash is being moved back before removing points
                    public event MOVING_BEST_CHAIN {
                        levels: warn
                    }
                    /// A header on the path back to the best chain has no stored parent
                    public event PARENT_NOT_FOUND {
                        levels: error
                        required header_hash: amaru_kernel::HeaderHash
                    }
                    /// A point is being removed from the chain store
                    public event POINT_REMOVED {
                        levels: info
                        required point: amaru_kernel::Point
                    }
                    /// The stored validation status of a block is being cleared
                    public event VALIDATION_CLEARED {
                        levels: info
                        required header_hash: amaru_kernel::HeaderHash
                    }
                }
                ledger {
                    /// A ledger snapshot was removed
                    public event SNAPSHOT_REMOVED {
                        levels: info
                        required epoch: u64
                    }
                    /// A ledger snapshot to remove does not exist
                    public event SNAPSHOT_NOT_FOUND {
                        levels: warn
                        required epoch: u64
                    }
                }
            }
            node {
                tags: setup
                /// The effective configuration a node run starts with
                public span RUN {
                    required chain_dir: String
                    required ledger_dir: String
                    required listen_address: String
                    required max_extra_ledger_snapshots: String
                    required migrate_chain_db: bool
                    required network: %amaru_kernel::NetworkName
                    required peer_address: String
                    required peer_snapshot: String
                    required peer_snapshot_relays: usize
                    required pid_file: String
                    required submit_api_address: String
                    required trace_buffer_min_entries: usize
                    required trace_buffer_max_size: usize
                    required trace_dump_path: String
                    required peer_removal_cooldown_secs: u64
                    required mempool_max_bytes: String
                    required tx_submission_max_window: u16
                    required tx_submission_fetch_batch_bytes: u64
                    required tx_submission_inflight_timeout_ms: u64
                    required tx_submission_insert_timeout_ms: u64
                    /// Path to an era history override, when one was given
                    optional era_history: String
                    /// Serialised global parameters, for test networks only
                    optional global_parameters: String
                }
                /// The submit API did not stop cleanly during shutdown.
                /// Reason ∈ {join_error, timeout}.
                public event SUBMIT_API_SHUTDOWN_FAILED {
                    levels: warn
                    required reason: String
                    optional error: String
                }
            }
            chain_db {
                /// Chain database already exists
                public event EXIST {
                    levels: warn
                    required dir: String
                    required hint: String
                }
            }
            current_epoch {
                /// Resolve the current epoch from Koios
                public event RESOLVE {
                    levels: info
                    required epoch: u64
                }
            }
            db_analyser {
                /// Run db-analyser to produce a ledger snapshot
                public event RUN {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    optional analyse_from: amaru_kernel::Slot
                }
                /// Reuse an existing db-analyser ledger snapshot
                public event REUSE_LEDGER_SNAPSHOT {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required snapshot: String
                }
            }
            last_block {
                /// Resolve the last produced block for an epoch
                public event RESOLVE {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required point: %amaru_kernel::NetworkPoint
                }
            }
            ledger_db {
                /// Ledger database already exists
                public event EXIST {
                    levels: warn
                    required dir: String
                    required hint: String
                }
            }
            mithril {
                /// Synchronize the cardano-node database from Mithril
                public event DOWNLOAD {
                    levels: info
                    required from_chunk: u64
                    required target_dir: String
                }
                /// Finished replaying downloaded blocks into the stores
                public event INGEST_COMPLETED {
                    levels: info
                    required processed: u64
                    required duration_seconds: f64
                    required processed_per_seconds: f64
                }
                /// Complete chain-store adoption after an interrupted Mithril ledger update
                public event RECOVER_CHAIN_TIP {
                    levels: info
                    required ledger_tip: amaru_kernel::Point
                    required chain_tip: amaru_kernel::Point
                }
                /// Local cardano-node database is recent enough; skipping Mithril download
                public event SKIP_DOWNLOAD {
                    levels: info
                    required from_chunk: u64
                    required required_chunk: u64
                    required target_dir: String
                    required reason: String
                }
            }
            node {
                /// Bootstrap a node from published snapshots
                public event BOOTSTRAP {
                    levels: info
                    required chain_dir: String
                    required ledger_dir: String
                    required network: %amaru_kernel::NetworkName
                    optional epoch: amaru_kernel::Epoch
                }
                /// Remove ledger and chain database from disk
                public event RM {
                    levels: info
                    required chain_dir: String
                    required ledger_dir: String
                    required network: %amaru_kernel::NetworkName
                }
                /// Roll the node databases back after a failure
                public event ROLLBACK {
                    levels: info
                    required chain_dir: String
                    required ledger_dir: String
                    required network: %amaru_kernel::NetworkName
                    required mode: String
                    optional epoch: u64
                    optional ledger_tip: String
                    optional best_chain: String
                    optional anchor: String
                }
            }
            snapshot {
                /// Create snapshots for the given network
                public event CREATE {
                    levels: info
                    required network: %amaru_kernel::NetworkName
                    optional epoch: amaru_kernel::Epoch
                    required snapshot_output_dir: String
                    required config_dir: String
                    required cardano_node_db: String
                    required dist_dir: String
                    optional snapshots: String
                }
                /// Finished creating a snapshot archive
                public event CREATED {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required archive: String
                }
                /// Package a snapshot archive
                public event PACKAGE {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required archive: String
                }
                /// Snapshot archive already packaged; skipping
                public event SKIP_PACKAGE {
                    levels: info
                    required epoch: amaru_kernel::Epoch
                    required slot: amaru_kernel::Slot
                    required archive: String
                    required reason: String
                }
                /// Publish snapshot archives
                public event PUBLISH {
                    levels: info
                    required network: %amaru_kernel::NetworkName
                    required local: usize
                    required remote: usize
                }
                /// Upload a snapshot archive
                public event UPLOAD {
                    levels: info
                    required archive: String
                }
                /// Finished uploading a snapshot archive
                public event UPLOADED {
                    levels: info
                    required archive: String
                }
                /// Snapshot archive already uploaded; skipping
                public event SKIP_UPLOAD {
                    levels: info
                    required archive: String
                }
                /// Update the published snapshot index
                public event UPDATE_INDEX {
                    levels: info
                    required network: %amaru_kernel::NetworkName
                    required snapshots: usize
                }
            }
        }
        mithril {
            progress {
                /// Mithril synchronization entered a new stage
                public event STAGE {
                    levels: info
                    required stage: String
                }
                /// Selected the applicable Mithril snapshot
                public event SNAPSHOT {
                    levels: info
                    required hash: String
                    required through_chunk: u64
                }
                /// Absolute Mithril database download progress
                public event DOWNLOAD {
                    levels: info
                    required downloaded_bytes: u64
                    required completed_files: u64
                    required total_files: u64
                    optional total_bytes: u64
                }
                /// Absolute block ingestion progress
                public event INGEST {
                    levels: info
                    required blocks: u64
                    required point: amaru_kernel::Point
                }
                /// Mithril synchronization completed successfully
                public event COMPLETE {
                    levels: info
                    required point: amaru_kernel::Point
                    required processed_blocks: u64
                }
            }
            snapshot {
                /// Fetch and verify a Mithril snapshot
                public event FETCH {
                    levels: info
                    required hash: String
                    required from_chunk: u64
                }
                /// Download and unpack immutable files from a Mithril snapshot
                public event DOWNLOAD {
                    levels: info
                    required target_dir: String
                    required from_chunk: u64
                    required through_chunk: u64
                }
                /// Download and verify the digests for a Mithril snapshot
                public event VERIFY_DIGESTS {
                    levels: info
                    required target_dir: String
                }
                /// Verify the local cardano-node database against a Mithril certificate
                public event VERIFY_DATABASE {
                    levels: info
                    required target_dir: String
                }
                /// Mithril cardano-node database is ready
                public event READY {
                    levels: info
                    required target_dir: String
                }
                /// Rebuild an invalid local immutable cache before retrying once
                public event REBUILD_CACHE {
                    levels: warn
                    required immutable_dir: String
                    required reason: String
                }
            }
        }
        stores {
            tags: db
            batch {
                /// Commit a write batch
                public span COMMIT {}
                /// Rollback a write batch
                public span ROLLBACK {}
                /// A transaction was dropped without commit or rollback.
                /// Outcome ∈ {left_open, auto_rolled_back}.
                public event DROPPED_WITHOUT_CLOSE {
                    levels: warn, error
                    required outcome: String
                }
            }
            ledger {
                epoch {
                    /// Create ledger snapshot for epoch
                    public span CREATE_SNAPSHOT {
                        required epoch: amaru_kernel::Epoch
                    }
                    /// Prune old snapshots
                    public span PRUNE_OLD_SNAPSHOTS {
                        required functional_minimum: amaru_kernel::Epoch
                        required desired_minimum: amaru_kernel::Epoch
                    }
                    /// Epoch transition tracking
                    public span TRY_TRANSITION {
                        required from: String
                        required to: String
                    }
                }
                overlay {
                    /// Reset fees to zero
                    public span RESET_FEES {}
                    /// Reset blocks count to zero
                    public span RESET_BLOCKS_COUNT {}
                    /// Pay rewards to all accounts before the epoch end
                    public span PAY_REWARDS {
                        /// Total number of accounts that received non-zero rewards
                        optional accounts_paid: u64
                        /// Total rewards effectively paid to ALL accounts; does not include unassignable rewards
                        optional rewards_paid: amaru_kernel::Lovelace
                        /// Treasury increase; corresponding to both the treasury tax and the unpaid rewards
                        optional treasury_delta: amaru_kernel::Lovelace
                        /// Reserves depletion from incentives; always negative.
                        optional reserves_delta: i64
                    }
                    account {
                        /// An account supposed to receive rewards is gone
                        event GONE {
                            levels: error
                            required rewards: amaru_kernel::Lovelace
                            required account: %amaru_kernel::Credential
                        }
                    }
                    /// Pruned proposals at an epoch boundary, recorded to facilitate future stake
                    /// distribution calculations.
                    public span RECORD_PRUNED_PROPOSALS {}
                    /// Pay withdrawals to accounts, or refund deposits
                    public span PAY_OR_REFUND_ACCOUNTS {
                        /// Total quantity of ADA paid, excluding treasury leftovers
                        optional total_paid_or_refunded: amaru_kernel::Lovelace
                        /// Total amounts that couldn't be paid to accounts, going back to treasury instead.
                        optional treasury_leftovers: amaru_kernel::Lovelace
                    }
                    /// Updating pools metadata or retiring pools at an epoch boundary.
                    public span UPDATE_OR_RETIRE_POOLS {
                        /// Total number of pools updating metadata
                        required pools_updated: u64
                        /// Total number of pools retired
                        required pools_retired: u64
                    }
                    /// Enact all governance updates and flush their outcome to disk
                    public span APPLY_GOVERNANCE_UPDATES {}
                    /// Add or remove CC members; or switch to a no-confidence state
                    public span UPDATE_CONSTITUTIONAL_COMMITTEE {
                        /// Whether or not updates switches the committee to a "no-confidence" state
                        required no_confidence: bool
                    }
                }
                utxo {
                    /// Point-read a UTxO entry
                    public span GET {}
                    /// Batch-insert UTxO entries
                    public span ADD {}
                    /// Batch-delete UTxO entries
                    public span REMOVE {}
                }
                pools {
                    /// Point-read a pool entry
                    public span GET {}
                    /// Batch-upsert pool entries
                    public span ADD {}
                    /// Schedule pool retirement
                    public event REMOVE {
                        levels: error
                        optional pool: amaru_kernel::PoolId
                        optional reason: String
                    }
                }
                accounts {
                    /// Point-read an account entry
                    public span GET {}
                    /// Batch-upsert account entries
                    public span ADD {}
                    /// Batch-delete account entries
                    public span REMOVE {}
                    /// Update rewards balance for a single account
                    public event SET {
                        levels: debug
                        optional credential_type: %amaru_kernel::CredentialKind
                        optional account: amaru_kernel::Hash<28>
                        optional reason: String
                    }
                    /// Reset rewards counters for many accounts
                    public event RESET_MANY {
                        levels: error
                        optional credential: %amaru_kernel::Credential
                        optional reason: String
                    }
                }
                recently_unregistered_accounts {
                    /// Insert a recently unregistered account
                    public span INSERT {}
                    /// Remove a recently unregistered account
                    public span REMOVE {}
                    /// Prune recently unregistered accounts
                    public span PRUNE {
                        required epoch: amaru_kernel::Epoch
                    }
                }
                dreps {
                    /// Point-read a DRep entry
                    public span GET {}
                    /// Batch-upsert DRep registrations
                    public event ADD {
                        levels: error
                        optional credential: %amaru_kernel::Credential
                        optional reason: String
                    }
                    /// Record DRep de-registration
                    public event REMOVE {
                        levels: error
                        optional drep: %amaru_kernel::Credential
                        optional reason: String
                    }
                    /// Refresh DRep expiry after a vote
                    public event SET_VALID_UNTIL {
                        levels: warn
                        optional credential: %amaru_kernel::Credential
                        optional reason: String
                    }
                }
                cc_members {
                    /// Read a constitutional committee member
                    public span GET {}
                    /// Upsert a constitutional committee member
                    public span UPSERT {}
                }
                proposals {
                    /// Insert governance proposals
                    public span ADD {}
                    /// Read governance proposals
                    public span GET { }
                    /// Remove enacted or expired proposals
                    public span REMOVE {}
                }
                recently_pruned_proposals {
                    /// Inserting recently pruned proposals
                    public span REPLACE_ALL {}
                }
                votes {
                    /// Record governance votes
                    public span ADD {}
                    /// Remove now-obsolete governance votes
                    public span REMOVE {}
                }
                slots {
                    /// Point-read a slot/block-issuer entry
                    public span GET {}
                    /// Write a slot/block-issuer entry
                    public span PUT {}
                }
                pots {
                    /// Read treasury/reserve/fees pots
                    public span GET {}
                    /// Write treasury/reserve/fees pots
                    public span PUT {}
                }
                snapshots {
                    /// Validate sufficient snapshots exist
                    public span VALIDATE {
                        optional snapshot_count: u64
                        optional continuous_ranges: u64
                    }
                    /// Skipped an unexpected file found in the snapshots directory
                    public event UNEXPECTED_FILE {
                        levels: warn
                        required filename: String
                    }
                }
                /// Full scan for a given collection
                public span ITER_SCAN {
                    required db_collection_name: String
                    optional rows_scanned: u64
                    optional rows_written: u64
                    optional rows_deleted: u64
                }
            }
            consensus {
                header {
                    /// Store a block header
                    public span STORE {
                        required hash: amaru_kernel::HeaderHash
                    }
                }
                block {
                    /// Store a raw block
                    public span STORE {
                        required hash: amaru_kernel::HeaderHash
                    }
                }
                chain {
                    /// Roll forward the chain to a point
                    public span ROLL_FORWARD {
                        required hash: amaru_kernel::HeaderHash
                        required slot: amaru_kernel::Slot
                    }
                    /// Switch the chain to a new fork
                    public span SWITCH_TO_FORK {
                        required hash: amaru_kernel::HeaderHash
                        required slot: amaru_kernel::Slot
                    }
                }
            }
        }
        mempool {
            state {
                /// Compact view of the mempool occupancy for terminal dashboards.
                public event UPDATE {
                    levels: debug
                    required tx_count: u64
                    required size_bytes: u64
                }
            }
            transaction {
                /// Transaction received by the mempool stage, before validation.
                public event RECEIVED {
                    levels: debug
                    required id: amaru_kernel::TransactionId
                    required origin: String
                }
                /// Transaction validated and inserted into the mempool.
                public event ACCEPTED {
                    levels: info
                    required id: amaru_kernel::TransactionId
                    required seq_no: u64
                    required origin: String
                }
                /// Transaction rejected at insertion. Reason ∈ {invalid, duplicate, mempool_full}.
                public event REJECTED {
                    levels: info
                    required id: amaru_kernel::TransactionId
                    required reason: String
                    optional validation_error: String
                }
                /// Transaction removed from the mempool. Reason ∈ {included_in_adopted_block, evicted_after_new_tip}.
                public event EVICTED {
                    levels: info
                    required id: amaru_kernel::TransactionId
                    required tip: amaru_kernel::Point
                    required reason: String
                }
                /// Detail trace carrying upstream peer attribution for a received tx.
                event RECEIVED_DETAIL {
                    levels: debug
                    required id: amaru_kernel::TransactionId
                    required peer: %amaru_kernel::Peer
                }
                /// Detail trace for a tip-driven revalidation pass.
                event REVALIDATION_DETAIL {
                    levels: debug
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
                    span PROCESS {
                        required message_type: String
                        required conn_id: u64
                        required peer: %amaru_kernel::Peer
                        required role: String
                        required local_use: String
                        required duplex: bool
                        required stopping: u64
                    }
                }
                /// A mini-protocol stage running on a connection died
                public event CHILD_DIED {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required conn_id: u64
                    required child: String
                }
                /// A mini-protocol stage running on a connection stopped upon request
                public event CHILD_STOPPED {
                    levels: info
                    required peer: %amaru_kernel::Peer
                    required conn_id: u64
                    required child: String
                }
                /// The peer refused our proposed protocol versions
                public event HANDSHAKE_REFUSED {
                    levels: error
                    required reason: String
                }
                /// The peer answered a version query instead of negotiating
                public event HANDSHAKE_QUERY_REPLY {
                    levels: info
                    required version_table: String
                }
                /// An inbound connection could not be accepted.
                /// Reason ∈ {aborted, error}.
                public event ACCEPT_FAILED {
                    levels: debug, error
                    required reason: String
                    optional error: String
                }
            }
            manager {
                message {
                    /// Handle manager stage messages
                    public span PROCESS {
                        required message_type: String
                    }
                }
                peer {
                    /// A new peer was added to the manager
                    public span ADD {
                        required peer: %amaru_kernel::Peer
                    }
                    /// Initiating an outbound connection to a peer
                    public event CONNECT {
                        levels: info
                        required peer: %amaru_kernel::Peer
                    }
                    /// An inbound connection was accepted from a peer
                    public span ACCEPTED {
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                    }
                    /// A peer was removed from the manager
                    public span REMOVE {
                        required peer: %amaru_kernel::Peer
                    }
                    /// A peer connection has died
                    public span CONNECTION_DIED {
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required role: String
                    }
                    /// A connection request for a peer was discarded.
                    /// Reason ∈ {already_connected_or_scheduled, already_connected, not_added}.
                    public event CONNECT_DISCARDED {
                        levels: debug, info
                        required peer: %amaru_kernel::Peer
                        required reason: String
                    }
                    /// An outbound connection to a peer was established
                    public event CONNECTED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                    }
                    /// An outbound connection attempt failed
                    public event CONNECT_FAILED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required error: String
                    }
                    /// The handshake completed on a connection
                    public event HANDSHAKE_COMPLETED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required full_duplex_capable: bool
                        required full_duplex: bool
                        required advertisable: bool
                    }
                    /// A duplicate connection is terminated after its handshake completed
                    public event DUPLICATE_TERMINATED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                    }
                    /// A connection is being closed on request.
                    /// Direction ∈ {inbound, outbound}.
                    public event DISCONNECTING {
                        levels: debug, info
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required direction: String
                    }
                    /// A disconnect request could not be carried out.
                    /// Reason ∈ {not_connected, connection_not_found, peer_already_removed,
                    /// before_handshake}.
                    public event DISCONNECT_IGNORED {
                        levels: debug, info
                        required peer: %amaru_kernel::Peer
                        required reason: String
                        optional conn_id: u64
                    }
                    /// A dead connection was reconciled with the peer's remaining state.
                    /// Outcome ∈ {peer_removed, kept_for_outbound, retries_suppressed,
                    /// reconnect_scheduled}.
                    public event CONNECTION_DIED_HANDLED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required outcome: String
                    }
                    /// Closing the socket of a dead connection failed
                    public event CLOSE_FAILED {
                        levels: error
                        required peer: %amaru_kernel::Peer
                        required error: String
                    }
                    /// A change of local use was requested on a connection
                    public event SET_LOCAL_USE {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required local_use: String
                    }
                    /// The connection finished converging to this local use
                    public event LOCAL_USE_APPLIED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required local_use: String
                    }
                }
                listen {
                    tags: setup
                    /// The node is accepting inbound connections on an address
                    public event STARTED {
                        levels: info
                        required listen_addr: String
                    }
                    /// The node could not listen on the configured address
                    public event FAILED {
                        levels: error
                        required listen_addr: String
                        required error: String
                    }
                }
                blocks {
                    /// Dispatch a block-fetch request to connected peers
                    event FETCH {
                        levels: debug
                        required from: amaru_kernel::Point
                        required through: amaru_kernel::Point
                        optional peers: String
                    }
                    /// A block-fetch request was dispatched to at least one connection
                    event FETCH_SENT {
                        levels: debug
                        required id: u64
                        required sent: usize
                    }
                    /// No connection was available to serve a block-fetch request
                    public event FETCH_NO_PEERS {
                        levels: debug
                        required id: u64
                    }
                }
                sharing {
                    /// No initiating connection was available to request shared peers from
                    event REQUEST_NO_CONNECTION {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                    }
                }
            }
            peer_selection {
                peer {
                    /// A connection has been established and the handshake completed successfully.
                    public span CONNECTED {
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required direction: String
                        required full_duplex_capable: bool
                        required full_duplex: bool
                    }
                    /// A connection has been terminated (graceful disconnect, error, handshake refusal,
                    /// or network error).
                    public span DISCONNECTED {
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required direction: String
                        optional reason: String
                    }
                    /// A peer was removed after behaving adversarially
                    public event REMOVED {
                        levels: warn
                        required peer: %amaru_kernel::Peer
                        required direction: String
                        required peer_state: String
                        required is_static: bool
                    }
                    /// A peer was added to the outbound set
                    public event ADDED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required was_banned: bool
                    }
                    /// A peer was not added to the outbound set.
                    /// Reason ∈ {already_added, too_many_inbound}.
                    public event ADD_SKIPPED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required reason: String
                    }
                    /// A candidate address was rejected and will not be used as a Peer.
                    public event ADDRESS_REJECTED {
                        levels: warn
                        required address: String
                        required reason: String
                    }
                    /// A selected bootstrap name resolved to a single peer, ready to dial.
                    public event RESOLVED {
                        levels: info
                        required candidate: String
                        required origin: String
                        required peer: %amaru_kernel::Peer
                    }
                    /// Name resolution for a bootstrap candidate failed (no viable address).
                    public event RESOLVE_FAILED {
                        levels: warn
                        required candidate: String
                        required reason: String
                    }
                    /// A peer reconnected while a previous connection was still registered;
                    /// the older connection is dropped. Direction ∈ {inbound, outbound}.
                    public event RECONNECTED {
                        levels: info, warn
                        required peer: %amaru_kernel::Peer
                        required direction: String
                        required conn_id: u64
                    }
                    /// A peer was reported as adversarial and is about to be banned
                    event ADVERSARIAL {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                    }
                    /// Local use dropped to Maintenance. Reason ∈ {churn, uninteresting}.
                    public event DEMOTED {
                        levels: info
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required reason: String
                    }
                }
                ledger {
                    /// Look for peer candidates registered as relays in the ledger
                    span CHECK_CANDIDATES {
                        required last_height: u64
                    }
                    /// Failed to read registered relay addresses from the ledger
                    public event CANDIDATES_FAILED {
                        levels: warn
                        required error: String
                    }
                }
                /// Connect to the initial set of peers at startup
                public event CONNECT_INITIAL {
                    levels: info
                    tags: setup
                    required static_peers: usize
                    required snapshot_peers: usize
                }
                sharing {
                    /// Peer-sharing address list received from peer.
                    public event RECEIVED {
                        levels: info
                        /// Peer that answered (learn) or requested (advertise) the share.
                        required peer: %amaru_kernel::Peer
                        /// Comma-separated list of shared listen addresses.
                        required peers: String
                        /// how many addresses were newly added to the shared pool.
                        required added: usize
                        /// size of the shared-peers pool after this reply.
                        required total: usize
                    }
                    /// Peer-sharing request served for peer.
                    public event SENT {
                        levels: info
                        /// Peer that answered (learn) or requested (advertise) the share.
                        required peer: %amaru_kernel::Peer
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
                    span CHAINSYNC_INITIATOR_STAGE {
                        required message_type: String
                    }
                    /// Handle chain sync initiator protocol messages
                    span CHAINSYNC_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                    /// Sample stored points to propose as chain intersections
                    span INTERSECT_POINTS {
                        optional points: Option<&[amaru_kernel::Point]>
                    }
                    /// A rollback target announced by the peer is not in the chain store
                    public event ROLLBACK_POINT_NOT_FOUND {
                        levels: error
                        required header_hash: amaru_kernel::HeaderHash
                    }
                }
                responder {
                    /// Handle chain sync responder stage messages
                    span CHAINSYNC_RESPONDER_STAGE {
                        required message_type: String
                    }
                    /// Handle chain sync responder protocol messages
                    span CHAINSYNC_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                    /// The peer ended the chainsync session
                    public event STOPPED {
                        levels: info
                    }
                }
            }
            blockfetch {
                responder {
                    /// A requested block range was refused.
                    /// Reason ∈ {inverted_range, exceeds_max_blocks}.
                    event RANGE_REFUSED {
                        levels: debug
                        required from: %amaru_kernel::NetworkPoint
                        required through: %amaru_kernel::NetworkPoint
                        required reason: String
                        optional max_blocks: usize
                    }
                }
            }
            handshake {
                initiator {
                    /// Handle handshake initiator stage messages
                    span HANDSHAKE_INITIATOR_STAGE {
                        required message_type: String
                    }
                    /// Handle handshake initiator protocol messages
                    span HANDSHAKE_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                    /// The protocol versions we offer to the peer
                    event PROPOSING_VERSIONS {
                        levels: debug
                        required our_versions: String
                    }
                    /// The outcome of the version negotiation
                    event CONCLUSION {
                        levels: debug
                        required handshake_result: String
                    }
                    /// Both sides opened a connection at the same time
                    event SIMULTANEOUS_OPEN {
                        levels: debug
                        required version_table: String
                    }
                }
                responder {
                    /// Handle handshake responder stage messages
                    span HANDSHAKE_RESPONDER_STAGE {
                        required version_table: String
                    }
                    /// Handle handshake responder protocol messages
                    span HANDSHAKE_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            keepalive {
                peer {
                    /// Measured round-trip time for a keepalive exchange on an established peer connection.
                    public event ROUND_TRIP {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                        required round_trip_micros: u64
                    }
                }
                initiator {
                    /// Handle keepalive initiator stage messages
                    span KEEPALIVE_INITIATOR_STAGE {
                        required cookie: u16
                    }
                    /// Handle keepalive initiator protocol messages
                    span KEEPALIVE_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                }
                responder {
                    /// Handle keepalive responder stage messages
                    span KEEPALIVE_RESPONDER_STAGE {
                        required cookie: u16
                    }
                    /// Handle keepalive responder protocol messages
                    span KEEPALIVE_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            peer_sharing {
                initiator {
                    /// Handle peer-sharing initiator stage messages
                    span PEER_SHARING_INITIATOR_STAGE {
                        required peer: %amaru_kernel::Peer
                        required conn_id: u64
                    }
                    /// Handle peer-sharing initiator protocol messages
                    span PEER_SHARING_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                    /// The peer broke the peer-sharing protocol and the connection is terminated.
                    /// Reason ∈ {no_request_in_flight, too_many_addresses}.
                    public event PROTOCOL_VIOLATION {
                        levels: warn
                        required reason: String
                        optional requested: u8
                        optional received: usize
                    }
                }
                responder {
                    /// Handle peer-sharing responder stage messages
                    span PEER_SHARING_RESPONDER_STAGE {
                        required amount: u8
                    }
                    /// Handle peer-sharing responder protocol messages
                    span PEER_SHARING_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                }
            }
            tx_submission {
                /// The tx-submission protocol is being torn down; the cause names the rule broken
                public event TERMINATING {
                    levels: warn
                    required cause: String
                }
                /// The responder side of the protocol was initialized
                event INITIALIZED {
                    levels: trace
                }
                initiator {
                    /// Handle tx-submission initiator stage messages
                    span TX_SUBMISSION_INITIATOR_STAGE {
                        required message_type: String
                        required peer: %amaru_kernel::Peer
                    }
                    /// Handle tx-submission initiator protocol messages
                    span TX_SUBMISSION_INITIATOR_PROTOCOL {
                        required message_type: String
                    }
                    /// Advertise transaction ids (and their sizes) to the peer in a ReplyTxIds.
                    event REPLY_TX_IDS {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required count: usize
                        required ids: &[amaru_kernel::TransactionId]
                    }
                    /// Send transaction bodies to the peer in a ReplyTxs. Advertised ids whose
                    /// tx was evicted before the fetch are listed in `omitted`.
                    event REPLY_TXS {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required count: usize
                        optional omitted: String
                    }
                    /// The peer acknowledged the advertised ids.
                    event ACKNOWLEDGED {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required ack: u16
                        required window: usize
                    }
                    /// A blocking RequestTxIds needs to wait until the mempool reaches `seq_no`.
                    event WAIT_FOR_AT_LEAST {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required seq_no: u64
                        optional req: u16
                    }
                    /// The peer requested transaction ids or bodies.
                    /// Request ∈ {tx_ids_blocking, tx_ids_non_blocking, txs}.
                    event RECEIVED_REQUEST {
                        levels: debug
                        required request: String
                        optional ack: u16
                        optional req: u16
                        optional count: usize
                        optional ids: String
                    }
                    /// The peer asked for transactions that are not in our outstanding window
                    public event UNAVAILABLE_TXS {
                        levels: warn
                        required unavailable: String
                    }
                    /// The peer acknowledged more transaction ids than are outstanding
                    public event OVER_ACKNOWLEDGED {
                        levels: warn
                        required ack: u16
                        required window: usize
                    }
                }
                responder {
                    /// Handle tx-submission responder stage messages
                    span TX_SUBMISSION_RESPONDER_STAGE {
                        required message_type: String
                        required peer: %amaru_kernel::Peer
                    }
                    /// Handle tx-submission responder protocol messages
                    span TX_SUBMISSION_RESPONDER_PROTOCOL {
                        required message_type: String
                    }
                    /// The peer advertised transaction ids in a ReplyTxIds.
                    event REPLY_TX_IDS_RECEIVED {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required count: usize
                    }
                    /// The peer delivered transaction bodies in a ReplyTxs.
                    event REPLY_TXS_RECEIVED {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required count: usize
                    }
                    /// An advertised tx is already in our mempool: it will be acknowledged
                    /// without ever fetching its body.
                    event SKIP_FETCH {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required id: amaru_kernel::TransactionId
                    }
                    /// Request tx ids from the peer, acknowledging processed ones.
                    event REQUEST_TX_IDS {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required ack: u16
                        required req: u16
                        required blocking: bool
                    }
                    /// Request tx bodies from the peer.
                    event REQUEST_TXS {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required count: usize
                        required ids: &[amaru_kernel::TransactionId]
                    }
                    /// Mempool near capacity: fetching is deferred until capacity frees up.
                    event AWAITING_CAPACITY {
                        levels: debug
                        required peer: %amaru_kernel::Peer
                        required pending: usize
                    }
                    /// The peer replied with more transaction ids than were requested
                    public event OVER_REPLIED {
                        levels: warn
                        required requested: u16
                        required received: usize
                        required max_window: u16
                    }
                    /// The peer sent transaction bodies that were never requested
                    public event UNSOLICITED_TXS {
                        levels: warn
                        required not_requested: String
                    }
                    /// The mempool did not answer an insertion batch before the timeout
                    public event MEMPOOL_TIMEOUT {
                        levels: error
                    }
                    /// A transaction received from a peer was handed to the mempool.
                    /// Outcome ∈ {inserted, invalid, mempool_full, duplicate}.
                    public event RECEIVED_TX {
                        levels: debug, warn
                        required id: amaru_kernel::TransactionId
                        required outcome: String
                        optional error: String
                    }
                }
            }
            mux {
                protocol {
                    /// Register protocol with muxer
                    span REGISTER {}
                    /// Buffer protocol messages
                    span BUFFER {}
                    /// Handle outgoing protocol messages
                    span OUTGOING {
                        optional proto_id: String
                        optional bytes: u64
                    }
                    /// Get next segment to send
                    span NEXT_SEGMENT {}
                    /// Handle received protocol data
                    event RECEIVED {
                        levels: trace
                        optional bytes: u64
                        optional proto_id: String
                    }
                    /// Run the protocol handler for one received segment
                    span HANDLE {
                        required bytes: u64
                    }
                    /// Want next message for protocol
                    span WANT_NEXT {}
                    /// A protocol segment was handed to the network. High-rate event.
                    event SEND {
                        levels: trace
                        required proto_id: String
                        required bytes: u64
                    }
                    /// A protocol segment was queued for sending. High-rate event.
                    event ENQUEUE {
                        levels: trace
                        required proto_id: String
                        required bytes: u64
                    }
                    /// A segment is written to the wire. High-rate event.
                    event SEGMENT_SENT {
                        levels: trace
                        required proto_id: String
                        required bytes: u64
                        required next: u64
                    }
                    /// A protocol updated how many bytes it is waiting for. High-rate event.
                    event WANT_UPDATED {
                        levels: trace
                        required want: usize
                    }
                    /// Bytes were delivered to a protocol buffer. High-rate event.
                    event BYTES_RECEIVED {
                        levels: trace
                        required wanted: usize
                    }
                    /// A complete message was extracted from a protocol buffer. High-rate event.
                    event MESSAGE_EXTRACTED {
                        levels: trace
                        required bytes: usize
                    }
                    /// The next delivery to a protocol is deferred until more bytes arrive
                    event DELIVERY_DEFERRED {
                        levels: trace
                    }
                    /// Incoming bytes are dropped because the protocol stopped consuming them
                    event IGNORING_BYTES {
                        levels: debug
                        required bytes: usize
                    }
                    /// A protocol buffer grew past its limit; incoming data is now ignored
                    event BUFFER_IGNORING {
                        levels: trace
                        required buffer: usize
                    }
                    /// A protocol message does not fit in the buffer allotted to it
                    public event BUFFER_EXCEEDED {
                        levels: info
                        required buffered: usize
                        required max_buffer: usize
                    }
                    /// Reducing a protocol buffer was not enough and the connection was killed
                    public event BUFFER_OVERFLOW {
                        levels: warn
                        required buffer: usize
                        required limit: usize
                    }
                }
                /// The muxer failed while moving data between a protocol and the network.
                /// Operation ∈ {send, recv_header, decode_header, recv_data, muxing, after_done}.
                public event FAILED {
                    levels: warn, error
                    required role: String
                    required peer: %amaru_kernel::Peer
                    required operation: String
                    required error: String
                }
                /// A segment header announcing an empty payload was received
                public event EMPTY_SEGMENT {
                    levels: info
                    required role: String
                    required peer: %amaru_kernel::Peer
                }
                /// The muxer is shutting down after a read or write error
                event TERMINATING {
                    levels: debug
                    required role: String
                }
            }
            /// A protocol handler received invalid input
            public event INVALID_INPUT {
                levels: error
                required proto: String
                required peer: %amaru_kernel::Peer
                required state: String
                required input: String
            }
        }
        setup {
            lifecycle {
                /// A termination signal was received; the node is shutting down
                public event TERMINATION_SIGNAL {
                    levels: warn
                }
                /// The consensus pipeline stopped while the node was still running
                public event CONSENSUS_DIED {
                    levels: error
                }
            }
            pid {
                /// The PID file for this node instance was created
                event CREATED {
                    levels: debug
                    required path: String
                    required pid: u32
                }
                /// The PID file could not be created or written
                public event WRITE_FAILED {
                    levels: warn
                    required error: String
                }
            }
            file_descriptors {
                /// The soft limit on open files is below what Amaru needs
                public event TOO_LOW {
                    levels: error
                    required current_soft_fd_limit: u64
                    required current_hard_fd_limit: u64
                    required expected_min: u64
                    /// Operator-facing instruction on how to raise the limit
                    required hint: String
                }
                /// The open-file limit could not be queried
                public event UNKNOWN {
                    levels: warn
                    required expected_min: u64
                }
            }
            trace_buffer {
                /// The stage trace buffer was written to disk
                public event DUMPED {
                    levels: info
                    required path: String
                }
                /// The stage trace buffer could not be written to disk
                public event DUMP_FAILED {
                    levels: error
                    required path: String
                    required error: String
                }
            }
            peer_snapshot {
                /// A peer snapshot was loaded at startup
                public event LOADED {
                    levels: info
                    required path: String
                    required point: %amaru_kernel::NetworkPoint
                    required pools: usize
                    required relays: usize
                    required node_to_client_version: u64
                    required configs_commit: String
                }
                /// A peer snapshot was loaded but holds no relay addresses
                public event EMPTY {
                    levels: warn
                    required path: String
                    required point: %amaru_kernel::NetworkPoint
                    required pools: usize
                }
                /// No embedded peer snapshot exists for the selected network
                public event MISSING {
                    levels: warn
                    required network: %amaru_kernel::NetworkName
                }
            }
            observability {
                /// Observability stack initialization
                public event INIT {
                    levels: info
                    required with_open_telemetry: bool
                    required with_json_traces: bool
                    required with_colors: bool
                }
                /// OTLP export failed; collection may not be started for every signal
                public event EXPORT_FAILED {
                    levels: warn
                    /// Comma-separated signals whose exporters reported failures
                    required unavailable_signals: String
                }
                /// OTLP collection recovered for previously unavailable signals
                public event EXPORT_RECOVERED {
                    levels: info
                    /// Comma-separated signals whose exporters connected successfully again
                    required recovered_signals: String
                }
            }
            build {
                /// Running binary build/version identity (package version, git commit, target).
                public event VERSION {
                    levels: info
                    required version: String
                    required git_commit: String
                    required git_dirty: bool
                    required os: String
                    required arch: String
                }
            }
            trace {
                /// Resolution of a trace filter from the environment
                public event FILTER {
                    levels: info, warn
                    required var: String
                    required value: String
                    required provided_by_user: bool
                    optional provided_invalid: bool
                    optional error: String
                }
            }
        }
        node {
            build {
                tags: setup
                /// Opened the ledger state; reports the ledger tip at startup
                public event LEDGER_OPENED {
                    levels: info
                    required tip: amaru_kernel::Point
                }
                /// Failed to notify the peer tracker of a stake distribution update
                public event STAKE_DIST_NOTIFY_FAILED {
                    levels: warn
                }
            }
            metrics {
                /// The metrics collector could not find Amaru's own process
                public event PROCESS_NOT_FOUND {
                    levels: error
                    required pid: u32
                }
            }
            submit_api {
                tags: io
                /// The transaction submission HTTP server is listening
                public event STARTED {
                    levels: info
                    required local_addr: String
                }
                /// The transaction submission HTTP server stopped with an error
                public event STOPPED {
                    levels: warn
                    required error: String
                }
                /// A submitted transaction could not reach the mempool.
                /// Reason ∈ {send_failed, response_dropped, deserialize_failed}.
                public event MEMPOOL_UNREACHABLE {
                    levels: warn
                    required reason: String
                }
            }
        }
        network {
            connection {
                tags: io
                /// Accept loop for incoming connections
                span ACCEPT_LOOP {}
                /// Listen on address
                span LISTEN {}
                /// Accept a connection
                span ACCEPT {}
                /// Connect to a peer
                span CONNECT {}
                /// Send data over connection
                span SEND {}
                /// Receive data from connection
                span RECV {}
                /// Close connection
                span CLOSE {}
                /// Aborted an existing listener task so the address can be rebound on restart
                public event LISTENER_RESTART {
                    levels: info
                    required address: String
                }
                /// A TCP listener is bound and accepting incoming connections
                event LISTENING {
                    levels: debug
                    required local: String
                }
                /// The accept loop terminated because the listener or channel closed
                public event ACCEPT_LOOP_STOPPED {
                    levels: info
                    required local: String
                }
                /// Accepted an incoming TCP connection
                event ACCEPTED {
                    levels: debug
                    required peer_addr: String
                }
                /// Established a TCP connection to a peer
                event CONNECTED {
                    levels: debug
                    required peer: %amaru_kernel::Peer
                }
            }
            perf {
                header {
                    /// Header accepted from an upstream peer, or forged locally, until chain
                    /// selection finishes with it. A locally forged block has no upstream
                    /// roll-forward and is a root span.
                    public span FORWARD {
                        parents: crate::amaru::consensus::roll_forward::PROCESS
                        root
                        required header_hash: amaru_kernel::HeaderHash
                    }
                    /// Header waiting in chain selection before it can be fetched.
                    public span BLOCK_FETCH_WAIT {
                        parents: crate::amaru::network::perf::header::FORWARD
                        required header_hash: amaru_kernel::HeaderHash
                    }
                }
                blocks {
                    /// One header's block body, from the request of the range that contains it
                    /// until that body arrives.
                    public span FETCH {
                        parents: crate::amaru::network::perf::header::FORWARD
                        required header_hash: amaru_kernel::HeaderHash
                    }
                }
                fork {
                    /// One switch onto a fork, from detection until the switch ends.
                    /// `header_hash` is the fork tip. The span covers every block on that fork.
                    public span SWITCH {
                        parents: crate::amaru::network::perf::header::FORWARD
                        required header_hash: amaru_kernel::HeaderHash
                    }
                }
            }
        }
    }
}
