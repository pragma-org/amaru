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

//! Example demonstrating the observability instrumentation macros
//!
//! This shows how to use #[trace] and trace_record! with local schemas.
//!
//! Key features demonstrated:
//! - Using define_local_schemas! to define schemas locally
//! - Compile-time const path validation for schema names
//! - Function instrumentation with required and optional parameters
//! - Custom expression fields using field = expr syntax

use amaru_observability_macros::{define_local_schemas, trace, trace_record};

define_local_schemas! {
    consensus {
        validate_header {
            /// Evolve nonce example
            EVOLVE_NONCE {
                required hash: String
            }
        }
    }

    ledger {
        state {
            /// Epoch transition example
            EPOCH_TRANSITION {
                required from: u64
                required into: u64
            }

            /// Create validation context example
            CREATE_VALIDATION_CONTEXT {
                required block_body_hash: String
                required block_number: u64
                required block_body_size: u64
                optional total_inputs: u64
            }

            /// Resolve inputs example
            RESOLVE_INPUTS {
                optional resolved_from_context: u64
                optional resolved_from_volatile: u64
                optional resolved_from_db: u64
            }

            /// Roll forward example
            ROLL_FORWARD {}
        }
    }

    network {
        chainsync_client {
            /// Find intersection example
            FIND_INTERSECTION {
                required peer: String
                required intersection_slot: u64
            }
        }
    }
}

/// Example 1: Basic tracing with required fields
#[trace(consensus::validate_header::EVOLVE_NONCE)]
pub fn evolve_nonce(_hash: String) -> Result<(), String> {
    Ok(())
}

/// Example 2: Tracing with multiple required fields
#[trace(ledger::state::EPOCH_TRANSITION)]
pub fn epoch_transition(from: u64, into: u64) -> Result<(), String> {
    Ok(())
}

/// Example 3: Tracing with required and optional fields
#[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
pub fn create_validation_context(
    _block_body_hash: String,
    _block_number: u64,
    _block_body_size: u64,
    _total_inputs: u64,
) -> Result<(), String> {
    Ok(())
}

/// Example 4: Function that records fields to the current span
pub fn add_resolve_stats(_resolved_from_context: u64, _resolved_from_volatile: u64) {
    trace_record!(
        ledger::state::RESOLVE_INPUTS,
        resolved_from_context = _resolved_from_context,
        resolved_from_volatile = _resolved_from_volatile
    );
}

/// Example 5: Schema with only optional fields
#[trace(ledger::state::ROLL_FORWARD)]
pub fn roll_forward() -> Result<(), String> {
    Ok(())
}

/// Example 6: Network schema
#[trace(network::chainsync_client::FIND_INTERSECTION)]
pub fn find_intersection(_peer: String, _intersection_slot: u64) -> Result<(), String> {
    Ok(())
}

// Helper functions for custom expression examples
fn get_current_block_number() -> u64 {
    42
}

fn get_block_size() -> u64 {
    1024
}

fn calculate_total_inputs() -> u64 {
    5
}

/// Example 7: Custom expression fields - providing required fields through expressions
#[trace(ledger::state::CREATE_VALIDATION_CONTEXT,
        block_body_hash = "hash_current_block",
        block_number = get_current_block_number(),
        block_body_size = get_block_size())]
pub fn process_block_with_computed_fields(
    _block_body_hash: String,
    _block_number: u64,
    _block_body_size: u64,
) -> Result<(), String> {
    Ok(())
}

/// Example 8: Custom expressions with optional fields
#[trace(ledger::state::CREATE_VALIDATION_CONTEXT,
        block_body_hash = "hash_block_abc",
        block_number = get_current_block_number(),
        block_body_size = get_block_size(),
        total_inputs = calculate_total_inputs())]
pub fn process_block_with_all_custom_fields(
    _block_body_hash: String,
    _block_number: u64,
    _block_body_size: u64,
    _total_inputs: u64,
) -> Result<(), String> {
    Ok(())
}

/// Example 9: Record computed metrics to current span
pub fn add_processing_metrics() {
    trace_record!(
        ledger::state::RESOLVE_INPUTS,
        resolved_from_context = 10_u64,
        resolved_from_volatile = 20_u64
    );
    tracing::debug!("Added computed processing metrics to current span");
}

/// Example 10: Function params vs custom expressions
#[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
pub fn create_validation_with_context(
    block_body_hash: String,
    block_number: u64,
    block_body_size: u64,
    _context: &str,    // Extra param - ignored (not in schema)
    _debug_mode: bool, // Extra param - ignored (not in schema)
) -> Result<(), String> {
    Ok(())
}

fn main() {
    println!("This example demonstrates the observability instrumentation macros.");
    println!(
        "The macros use const variable paths instead of string literals for compile-time validation."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_flow() {
        assert!(evolve_nonce("hash123".to_string()).is_ok());
        assert!(epoch_transition(1, 2).is_ok());
        assert!(create_validation_context("hash".to_string(), 100, 1024, 5).is_ok());
    }

    #[test]
    fn test_network_operations() {
        assert!(find_intersection("peer123".to_string(), 100).is_ok());
    }

    #[test]
    fn test_trace_record() {
        add_resolve_stats(10, 20);
        add_processing_metrics();
    }

    #[test]
    fn test_custom_expression_fields() {
        assert!(process_block_with_computed_fields("dummy".to_string(), 1, 0).is_ok());
        assert!(process_block_with_all_custom_fields("dummy".to_string(), 1, 0, 0).is_ok());
    }

    #[test]
    fn test_extra_params() {
        assert!(create_validation_with_context("hash".to_string(), 100, 1024, "ctx", true).is_ok());
    }
}
