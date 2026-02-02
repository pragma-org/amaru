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
//! - [`#[augment_trace]`](macro@augment_trace) - Adds fields to the current span without requiring all fields

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

/// Augments the current span with additional optional fields.
///
/// Unlike [`#[trace]`](macro@trace), this macro can only use **optional** fields
/// from the schema. Required fields are not allowed in `augment_trace` because
/// augmenting a span should only add supplementary context, not core identifying
/// information.
///
/// This macro does NOT create a new span - it records fields to the current span.
/// The function parameters are automatically recorded at the start of the function.
///
/// Use this when you want to add additional context to an existing span
/// without creating a new one.
///
/// # Example
///
/// ```text
/// // Given a schema with optional fields like 'peer_id' and 'timing_ms':
/// #[augment_trace(consensus::chain_sync::VALIDATE_HEADER)]
/// fn add_peer_context(peer_id: String, timing_ms: u64) {
///     // peer_id and timing_ms are automatically recorded to the current span
/// }
/// ```
#[proc_macro_attribute]
pub fn augment_trace(args: TokenStream, input: TokenStream) -> TokenStream {
    traces::expand_augment_trace(args, input)
}
