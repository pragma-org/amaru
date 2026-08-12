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

//! Procedural macros for observability instrumentation in amaru
//!
//! This crate provides compile-time validated tracing schemas and instrumentation.
//!
//! # Overview
//!
//! The macros in this crate work together to provide compile-time validation of tracing:
//!
//! - [`define_schemas!`] - Declares schemas with their fields and types
//! - [`debug_span!`](macro@trace_span) - Creates typed spans with strict schema validation
//! - [`trace_record!`](macro@trace_record) - Records fields to the current span
//!
//! # Disabling Tracing at Compile Time
//!
//! Set the `AMARU_TRACE_NO_EMIT` environment variable during compilation to disable all tracing:
//!
//! ```bash
//! cargo clean
//! AMARU_TRACE_NO_EMIT=1 cargo build --release
//! ```
//!
//! **Note:** `cargo clean` is required because cargo caches macro expansions. The environment
//! variable must be set during a clean build to take effect.
//!
//! When enabled, all macros become no-ops, completely removing tracing overhead.
//!
//! # Emitting Private Schemas
//!
//! Schemas are private by default. Set `AMARU_TRACE_EMIT_PRIVATE` to a truthy value at runtime
//! to emit spans and records created from private schemas.
//!
//! Truthy values are any non-empty values except `0` and `false`.
//!
//! # Schema definition embedded DSL for `define_schemas!` / `define_local_schemas!`.
//!
//! This module parses the schema language with [`syn`] and emits nested modules, schema
//! marker types, and the declarative helper macros used by `trace_span!`, `trace_event!`,
//! and `trace_record!`. Identifiers and types from the definition are re-emitted with their
//! original spans so rust-analyzer can go-to-definition from generated items back into the
//! schema source.
//!
//! # Embedded DSL specification
//!
//! A schema file is a sequence of **category** blocks. Categories nest arbitrarily and form
//! the module path of every schema declared beneath them. Schema names are distinguished
//! from category names by capitalization.
//!
//! ```text
//! define_schemas! {
//!     <category> {
//!         tags: <tag>, <tag>, ...          // optional; inherited by nested schemas
//!         <category> { ... }               // nested category
//!         /// Description of the event     // required doc comment on every schema
//!         [public] <SCHEMA> {
//!             tags: <tag>, ...             // optional; overrides inherited module tags
//!             required <field>: <Type> [,]
//!             optional <field>: <Type> [,]
//!         }
//!     }
//! }
//! ```
//!
//! ## Grammar
//!
//! ```text
//! input            := category+
//! category         := ident "{" category_body "}"
//! category_body    := ( tags_decl | category | schema )*
//!
//! schema           := attrs "public"? UPPER_IDENT "{" schema_body "}"
//! schema_body      := ( tags_decl | field )*
//!
//! tags_decl        := "tags" ":" tag ("," tag)*
//! tag              := lowercase_ident
//!
//! field            := attrs ("required" | "optional") ident ":" Type ","?
//! attrs            := outer_attribute*     // typically `///` doc comments
//! ```
//!
//! Where:
//!
//! - **`ident`** is a Rust identifier (letters, digits, `_`; not starting with a digit).
//! - **`UPPER_IDENT`** is an identifier whose first character is uppercase (by convention
//!   `SCREAMING_SNAKE_CASE`). These introduce schemas; all other identifiers introduce
//!   categories.
//! - **`Type`** is any Rust type accepted by [`syn::Type`] (primitives, paths, generics, …).
//! - **`public`** may only appear immediately before a schema name, never before a category.
//! - **`required` / `optional`** are prefix keywords on individual fields. Block forms such
//!   as `required { ... }` are not part of the language.
//! - Trailing commas after field type annotations are allowed.
//! - Square brackets `[` `]` are not used; field lists always use curly braces.
//!
//! ## Categories
//!
//! Categories are lowercase module segments. Nested categories produce a matching nested
//! `pub mod` tree in the expansion. The category path becomes part of:
//!
//! - the schema's fully-qualified path (`amaru::ledger::state::ROLL_FORWARD`);
//! - the tracing `target` (first two segments, e.g. `amaru::ledger`);
//! - the tracing event/span `name` (remaining segments plus schema name, lowercased and
//!   joined with `.`, e.g. `state.roll_forward`).
//!
//! ## Schemas
//!
//! Every schema **must** have at least one doc comment (`/// …`). Multi-line docs are
//! joined with spaces into the runtime registry description.
//!
//! Schemas are **private by default**. Mark with `public` to:
//!
//! - always emit spans/events/records (private ones need `AMARU_TRACE_EMIT_PRIVATE`);
//! - include the schema in the runtime registry dump used by documentation tooling.
//!
//! Empty field lists are valid: `ROLL_FORWARD {}`.
//!
//! ## Fields
//!
//! Each field is either `required` or `optional`:
//!
//! - **required** — must be supplied at every `trace_span!` / `trace_event!` call site;
//! - **optional** — may be omitted; may still be recorded later with `trace_record!`.
//!
//! Field names must be valid Rust identifiers (no string-literal names). The names
//! `name`, `schema`, and `message` are reserved by the tracing macros.
//!
//! Field types drive compile-time type checks in the generated `_RECORD!` helpers and the
//! typed accessors on the schema marker type. Transport across `tracing` is type-driven:
//! - primitives (`bool`, integers, floats) and fields declared exactly as `String` use typed
//!   `tracing::Value`; other string-like types (`&str`, `Cow<'_, str>`) take the CBOR path;
//! - all other types must implement [`Serialize`](serde::Serialize) and are encoded as CBOR
//!   (`record_bytes`). Explicit `%` / `?` formatters still require Display/Debug.
//!
//! Doc comments on individual fields are accepted and currently ignored by code generation
//! (they document the schema source for readers).
//!
//! ## Tags
//!
//! `tags: cpu, io` declares functional tags recorded automatically on every span as
//! boolean attributes `amaru.tag.<name>`. Tags declared on a category apply to all schemas
//! nested inside that have no local `tags:` line. A schema-level `tags:` fully replaces the
//! inherited set (it does not merge). Tags must be lowercase identifiers; duplicates in a
//! single declaration are rejected.
//!
//! Spans can be selected with an `EnvFilter` directive, for example:
//! `AMARU_LOG='[{amaru.tag.cpu=true}]=trace'`.
//!
//! ## What the expansion produces
//!
//! For each schema `amaru::ledger::state::ROLL_FORWARD` the macro emits:
//!
//! 1. Nested modules mirroring the category path, with identifiers taken from the definition
//!    (preserving source spans for go-to-definition).
//! 2. A unit struct `ROLL_FORWARD` with associated constants:
//!    - `NAME`, `TARGET`, `PATH`, `VALIDATION`, `PUBLIC`, `SCHEMA_FIELD_COUNT`
//!    - `FIELD_<NAME>` string constants for each field
//!    - typed `fn field_name(record) -> …` accessors (exported schemas only)
//! 3. Hidden declarative macros used by the instrumentation proc-macros:
//!    - `__…_REQUIRE!` — required-field presence
//!    - `__…_RECORD!` / `__…_ASSIGN!` — field type checks and value assignment
//!    - `__…_INSTRUMENT!` — span construction with metadata and tags
//!    - `__VALIDATE_…!` — module-level schema name check
//! 4. An `inventory` submission for the runtime registry (`define_schemas!` only).
//!
//! Call-site macros (`trace_span!`, `trace_event!`, `trace_record!`) remain source-compatible;
//! only the parser and the way identifiers are threaded through the expansion change.

use std::env::var;

use proc_macro::TokenStream;

mod define_schemas;
mod traces;
mod utils;

/// Check if tracing is disabled via AMARU_TRACE_NO_EMIT environment variable.
/// When set (to any value), all tracing macros become no-ops.
fn is_trace_no_emit() -> bool {
    var("AMARU_TRACE_NO_EMIT").is_ok_and(|v| !v.is_empty())
}

// =============================================================================
// Public Macros
// =============================================================================

/// Defines tracing schemas with compile-time validation.
///
/// Parses the schema embedded DSL with [`syn`] and emits nested modules, schema marker
/// types, and declarative helper macros used by [`trace_span!`], [`trace_event!`], and
/// [`trace_record!`]. Identifiers and types from the definition keep their original spans
/// so go-to-definition reaches the schema source.
///
/// Full language reference: see the `define_schemas` module documentation in this crate.
///
/// A `tags: <name>, ...` declaration assigns functional tags to schemas, each recorded
/// automatically on every span as a boolean `amaru.tag.<name>` attribute. Tags can be
/// declared at the module level (inherited by all schemas beneath) or inside a schema
/// (overriding the module default). Spans can then be selected with an `EnvFilter`
/// directive, e.g. `AMARU_LOG='[{amaru.tag.cpu=true}]=trace'`.
///
/// Generated macros are exported with `#[macro_export]` for use across crates.
/// For local/test schemas that won't be exported, use [`define_local_schemas!`] instead.
#[proc_macro]
pub fn define_schemas(input: TokenStream) -> TokenStream {
    define_schemas::expand(input)
}

/// Defines local tracing schemas without exporting them.
///
/// This is identical to `define_schemas!` but does NOT add `#[macro_export]`
/// to the generated macros. Use this for test schemas or schemas that are
/// only used within the same crate.
///
/// This avoids the Rust error:
/// "macro-expanded `macro_export` macros from the current crate cannot be
/// referred to by absolute paths"
#[proc_macro]
pub fn define_local_schemas(input: TokenStream) -> TokenStream {
    define_schemas::expand_local(input)
}

/// Records fields to the current span with a schema anchor.
///
/// This macro records fields to the current span, with the schema constant documenting
/// which schema these fields belong to. Optionally emits a log event at a specified level.
///
/// # Syntax
///
/// ```text
/// trace_record!(SCHEMA, field = value, ...);           // Record to span only
/// trace_record!(LEVEL, SCHEMA, field = value, ...);    // Record to span AND emit log event
/// ```
///
/// # Example
///
/// ```text
/// fn apply_block(point_slot: u64, error: Option<&str>) {
///     let _span = debug_span!(ledger::block::APPLY, point_slot = point_slot);
///     let _guard = _span.enter();
///
///     if let Some(error) = error {
///         // Record to span only
///         trace_record!(ledger::block::APPLY, error = error);
///
///         // Record to span and emit INFO log event
///         trace_record!(INFO, ledger::block::APPLY, error = error);
///     }
/// }
/// ```
#[proc_macro]
pub fn trace_record(input: TokenStream) -> TokenStream {
    traces::expand_trace_record(input)
}

/// Emits a tracing event with a compile-time validated schema anchor.
///
/// The event `target` and `name` are derived from the schema constant, exactly
/// like the spans created by [`trace_span!`](macro@trace_span). Emission is
/// gated on the schema visibility (public schemas always emit; private ones
/// only when `AMARU_TRACE_EMIT_PRIVATE` is set).
///
/// # Syntax
///
/// ```text
/// trace_event!(LEVEL, SCHEMA, field = value, ...);
/// ```
///
/// # Example
///
/// ```text
/// trace_event!(ERROR, stores::ledger::accounts::RESET_MANY, ?credential, reason = "no account for given credential");
/// ```
#[proc_macro]
pub fn trace_event(input: TokenStream) -> TokenStream {
    traces::expand_trace_event(input)
}

/// Creates a tracing span with compile-time validated schema anchor.
///
/// This macro creates spans with a schema-anchored approach that provides
/// compile-time validation. Supports custom log levels (default: TRACE).
///
/// # Syntax
///
/// ```text
/// debug_span!(SCHEMA, field = value, ...);           // TRACE-level span (default)
/// debug_span!(LEVEL, SCHEMA, field = value, ...);    // Custom level span
/// ```
///
/// # Example
///
/// ```text
/// debug_span!(operations::database::OPENING_CHAIN_DB, path = "...")
/// debug_span!(DEBUG, ledger::block::APPLY, point_slot = 1024)
/// debug_span!(INFO, consensus::VALIDATE_HEADER)
/// ```
#[proc_macro]
pub fn trace_span(input: TokenStream) -> TokenStream {
    traces::expand_trace_span(input)
}
