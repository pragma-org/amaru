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
//!
//! # Disabling Tracing at Compile Time
//!
//! Set the `AMARU_TRACE_NOOP` environment variable during compilation to disable all tracing:
//!
//! ```bash
//! cargo clean
//! AMARU_TRACE_NOOP=1 cargo build --release
//! ```
//!
//! **Note:** `cargo clean` is required because cargo caches macro expansions. The environment
//! variable must be set during a clean build to take effect.
//!
//! When enabled, all macros become no-ops, completely removing tracing overhead.

use proc_macro::TokenStream;
use std::env::var;

mod define_schemas;
mod traces;
mod utils;

/// Check if tracing is disabled via AMARU_TRACE_NOOP environment variable.
/// When set (to any value), all tracing macros become no-ops.
fn is_trace_noop() -> bool {
    var("AMARU_TRACE_NOOP").is_ok_and(|v| !v.is_empty())
}

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
/// #[trace(ledger::state::APPLY_BLOCK)]
/// fn apply_block(block: &Block) {
///     // Record to span only
///     trace_record!(ledger::state::APPLY_BLOCK, size = block.size());
///     
///     // Record to span and emit INFO log event
///     trace_record!(INFO, ledger::state::APPLY_BLOCK, tx_count = block.transactions.len());
/// }
/// ```
#[proc_macro]
pub fn trace_record(input: TokenStream) -> TokenStream {
    traces::expand_trace_record(input)
}

/// Creates a tracing span with compile-time validated schema anchor.
///
/// This macro creates spans with a schema-anchored approach that provides
/// compile-time validation. Supports custom log levels (default: TRACE).
///
/// # Syntax
///
/// ```text
/// trace_span!(SCHEMA, field = value, ...);           // TRACE-level span (default)
/// trace_span!(LEVEL, SCHEMA, field = value, ...);    // Custom level span
/// ```
///
/// # Example
///
/// ```text
/// trace_span!(operations::database::OPENING_CHAIN_DB, path = "...")
/// trace_span!(DEBUG, ledger::state::APPLY_BLOCK, block_size = 1024)
/// trace_span!(INFO, consensus::VALIDATE_HEADER)
/// ```
#[proc_macro]
pub fn trace_span(input: TokenStream) -> TokenStream {
    traces::expand_trace_span(input)
}
