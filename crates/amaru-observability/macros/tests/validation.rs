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

//! Core validation tests for trace and trace_record! macros
//!
//! This file consolidates all unit tests for:
//! - Required/optional field handling
//! - Custom expressions
//! - Extra function parameters (ignored)
//! - trace_record! restrictions
//!
//! UI compile_fail tests cover error cases separately.

use amaru_observability_macros::{define_local_schemas, trace, trace_record};

define_local_schemas! {
    consensus {
        validate_header {
            /// Evolve nonce for testing
            EVOLVE_NONCE {
                required hash: String
            }
        }
    }

    ledger {
        state {
            /// Apply block for testing
            APPLY_BLOCK {
                required point_slot: u64
            }

            /// Create validation context for testing
            CREATE_VALIDATION_CONTEXT {
                required block_body_hash: String
                required block_number: u64
                required block_body_size: u64
                optional total_inputs: u64
            }

            /// Epoch transition for testing
            EPOCH_TRANSITION {
                required from: u64
                required into: u64
            }

            /// Resolve inputs for testing
            RESOLVE_INPUTS {
                optional resolved_from_context: u64
                optional resolved_from_volatile: u64
                optional resolved_from_db: u64
            }

            /// Roll forward for testing
            ROLL_FORWARD {}
        }
    }

    network {
        chainsync_client {
            /// Find intersection for testing
            FIND_INTERSECTION {
                required peer: String
                required intersection_slot: u64
            }
        }
    }
}

mod required_fields {
    use super::*;

    #[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
    fn all_required(_block_body_hash: String, _block_number: u64, _block_body_size: u64) {}

    #[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
    fn required_different_order(
        _block_body_size: u64,
        _block_body_hash: String,
        _block_number: u64,
    ) {
    }

    #[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
    fn required_plus_optional(
        _block_body_hash: String,
        _block_number: u64,
        _block_body_size: u64,
        _total_inputs: u64,
    ) {
    }

    #[trace(ledger::state::EPOCH_TRANSITION)]
    fn simple_required(_from: u64, _into: u64) {}

    #[test]
    fn test_required_fields() {
        all_required("hash".into(), 100, 1024);
        required_different_order(1024, "hash".into(), 100);
        required_plus_optional("hash".into(), 100, 1024, 5);
        simple_required(1, 2);
    }
}

mod optional_fields {
    use super::*;

    #[trace(ledger::state::RESOLVE_INPUTS)]
    fn no_optional() {}

    #[trace(ledger::state::RESOLVE_INPUTS)]
    fn one_optional(_resolved_from_context: u64) {}

    #[trace(ledger::state::RESOLVE_INPUTS)]
    fn two_optional(_resolved_from_context: u64, _resolved_from_volatile: u64) {}

    #[trace(ledger::state::ROLL_FORWARD)]
    fn optional_only_schema() {}

    #[test]
    fn test_optional_fields() {
        no_optional();
        one_optional(10);
        two_optional(10, 20);
        optional_only_schema();
    }
}

mod custom_expressions {
    use super::*;

    fn compute_hash(input: &str) -> String {
        format!("hash_{}", input)
    }

    fn get_slot() -> u64 {
        12345
    }

    #[trace(consensus::validate_header::EVOLVE_NONCE, hash = compute_hash("test"))]
    fn inline_expression(_hash: String) {}

    #[trace(consensus::validate_header::EVOLVE_NONCE)]
    fn normal_parameter(hash: String) {
        let _ = hash;
    }

    #[trace(ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = compute_hash("block"),
            block_number = get_slot(),
            block_body_size = 512_u64 * 2)]
    fn multiple_expressions(_block_body_hash: String, _block_number: u64, _block_body_size: u64) {}

    #[trace(ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = format!("prefix_{}", "computed"),
            block_number = 42_u64 + 100,
            block_body_size = 1024)]
    fn complex_expressions(_block_body_hash: String, _block_number: u64, _block_body_size: u64) {}

    #[test]
    fn test_custom_expressions() {
        inline_expression("ignored".into());
        normal_parameter("param".into());
        multiple_expressions("ignored".into(), 0, 0);
        complex_expressions("ignored".into(), 0, 0);
    }
}

mod extra_params {
    use super::*;

    // Function parameters that don't match schema fields are silently ignored.
    // They're treated as dependencies used to compute custom expressions.
    // Custom expressions (field = expr) are strictly validated - typos cause compile errors.

    // This works: extra params are ignored, only schema-matching params are traced
    #[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
    fn with_extra_params(
        block_body_hash: String, // Schema field - traced
        block_number: u64,       // Schema field - traced
        block_body_size: u64,    // Schema field - traced
        context: &str,           // Extra param - ignored (not in schema)
        config: Option<u32>,     // Extra param - ignored (not in schema)
    ) {
        let _ = (context, config);
    }

    #[trace(ledger::state::CREATE_VALIDATION_CONTEXT)]
    fn with_extra_refs(
        block_body_hash: String, // Schema field - traced
        block_number: u64,       // Schema field - traced
        block_body_size: u64,    // Schema field - traced
        extra_ref: &str,         // Extra param - ignored
        extra_slice: &[u8],      // Extra param - ignored
    ) {
        let _ = (extra_ref, extra_slice);
    }

    #[test]
    fn test_extra_params_ignored() {
        with_extra_params("hash".into(), 100, 1024, "ctx", Some(42));
        with_extra_refs("hash".into(), 100, 1024, "ref", &[1, 2]);
    }
}

mod trace_record_tests {
    use super::*;

    fn augment_with_optional(_resolved_from_context: u64) {
        trace_record!(
            ledger::state::RESOLVE_INPUTS,
            resolved_from_context = _resolved_from_context
        );
    }

    fn augment_with_multiple(_resolved_from_context: u64, _resolved_from_volatile: u64) {
        trace_record!(
            ledger::state::RESOLVE_INPUTS,
            resolved_from_context = _resolved_from_context,
            resolved_from_volatile = _resolved_from_volatile
        );
    }

    fn augment_with_extra(_resolved_from_context: u64, extra: &str) {
        trace_record!(
            ledger::state::RESOLVE_INPUTS,
            resolved_from_context = _resolved_from_context
        );
        let _ = extra;
    }

    fn augment_with_expression() {
        trace_record!(
            ledger::state::RESOLVE_INPUTS,
            resolved_from_context = 100_u64
        );
    }

    #[test]
    fn test_trace_record() {
        augment_with_optional(10);
        augment_with_multiple(10, 20);
        augment_with_extra(10, "extra");
        augment_with_expression();
    }
}

mod namespace_paths {
    use super::*;

    #[trace(network::chainsync_client::FIND_INTERSECTION)]
    fn network_namespace(_peer: String, _intersection_slot: u64) {}

    #[trace(consensus::validate_header::EVOLVE_NONCE)]
    fn consensus_namespace(_hash: String) {}

    #[trace(ledger::state::APPLY_BLOCK)]
    fn ledger_namespace(_point_slot: u64) {}

    #[test]
    fn test_namespace_paths() {
        network_namespace("peer".into(), 100);
        consensus_namespace("hash".into());
        ledger_namespace(42);
    }
}
