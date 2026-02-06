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
//! - [`#[trace]`](macro@trace) - Instruments functions, requiring all required schema fields
//! - [`trace_record!`](macro@trace_record) - Records fields to the current span

use proc_macro::TokenStream;

mod define_schemas;
mod traces;
mod utils;

// =============================================================================
// Public Macros
// =============================================================================

/// Defines tracing schemas with compile-time validation.
///
/// This macro generates validator macros for each schema that ensure
/// fields have correct names and types at compile time.
///
/// Generated macros are exported with `#[macro_export]` for use across crates.
/// For local/test schemas that won't be exported, use `define_local_schemas!` instead.
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

/// Instruments a function with tracing.
///
/// The trace argument must be a const path defined via `define_schemas!`.
/// This macro validates at compile-time that:
/// - The schema constant exists
/// - All **required** fields are present as function parameters
/// - All parameters have the correct types matching the schema
/// - Optional fields may optionally be present with correct types
///
/// # Example
///
/// ```text
/// #[trace(consensus::chain_sync::VALIDATE_HEADER)]
/// fn validate_header(point_slot: u64, point_hash: String) -> Result<(), String> {
///     Ok(())
/// }
/// ```
#[proc_macro_attribute]
pub fn trace(args: TokenStream, input: TokenStream) -> TokenStream {
    traces::expand_trace(args, input)
}

/// Records fields to the current span with a schema anchor.
///
/// This macro records fields to the current span, with the schema constant documenting
/// which schema these fields belong to.
///
/// # Example
///
/// ```text
/// #[trace(ledger::state::APPLY_BLOCK)]
/// fn apply_block(block: &Block) {
///     trace_record!(ledger::state::APPLY_BLOCK, size = block.size(), tx_count = block.transactions.len());
/// }
/// ```
#[proc_macro]
pub fn trace_record(input: TokenStream) -> TokenStream {
    traces::expand_trace_record(input)
}

/// Creates a tracing span with compile-time validated schema anchor.
///
/// This macro creates TRACE-level spans with a schema-anchored approach
/// that provides compile-time validation.
///
/// # Example
///
/// ```text
/// trace_span!(operations::database::OPENING_CHAIN_DB, path = "...")
/// trace_span!(ledger::state::APPLY_BLOCK, block_size = 1024)
/// ```
#[proc_macro]
pub fn trace_span(input: TokenStream) -> TokenStream {
    traces::expand_trace_span(input)
}
