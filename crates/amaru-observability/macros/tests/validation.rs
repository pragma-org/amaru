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

//! Core validation tests for trace_span! and trace_record! macros
//!
//! This file consolidates all unit tests for:
//! - Required/optional field handling
//! - Custom expressions
//! - Extra function parameters (irrelevant unless explicitly referenced)
//! - trace_record! restrictions
//!
//! UI compile_fail tests cover error cases separately.

use amaru_observability_macros::{define_local_schemas, trace_record, trace_span};
use tracing::Instrument;

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
            public APPLY_BLOCK {
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

    fn all_required(block_body_hash: String, block_number: u64, block_body_size: u64) {
        let _span = trace_span!(
            ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = &block_body_hash,
            block_number = block_number,
            block_body_size = block_body_size
        );
        let _guard = _span.enter();
    }

    fn required_different_order(block_body_size: u64, block_body_hash: String, block_number: u64) {
        let _span = trace_span!(
            ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = &block_body_hash,
            block_number = block_number,
            block_body_size = block_body_size
        );
        let _guard = _span.enter();
    }

    fn required_plus_optional(block_body_hash: String, block_number: u64, block_body_size: u64, total_inputs: u64) {
        let _span = trace_span!(
            ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = &block_body_hash,
            block_number = block_number,
            block_body_size = block_body_size,
            total_inputs = total_inputs
        );
        let _guard = _span.enter();
    }

    fn simple_required(from: u64, into: u64) {
        let _span = trace_span!(ledger::state::EPOCH_TRANSITION, from = from, into = into);
        let _guard = _span.enter();
    }

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

    fn no_optional() {
        let _span = trace_span!(ledger::state::RESOLVE_INPUTS);
        let _guard = _span.enter();
    }

    fn one_optional(resolved_from_context: u64) {
        let _span = trace_span!(ledger::state::RESOLVE_INPUTS, resolved_from_context = resolved_from_context);
        let _guard = _span.enter();
    }

    fn two_optional(resolved_from_context: u64, resolved_from_volatile: u64) {
        let _span = trace_span!(
            ledger::state::RESOLVE_INPUTS,
            resolved_from_context = resolved_from_context,
            resolved_from_volatile = resolved_from_volatile
        );
        let _guard = _span.enter();
    }

    fn optional_only_schema() {
        let _span = trace_span!(ledger::state::ROLL_FORWARD);
        let _guard = _span.enter();
    }

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

    fn inline_expression(_hash: String) {
        let _span = trace_span!(consensus::validate_header::EVOLVE_NONCE, hash = compute_hash("test"));
        let _guard = _span.enter();
    }

    fn normal_parameter(hash: String) {
        let _span = trace_span!(consensus::validate_header::EVOLVE_NONCE, hash = &hash);
        let _guard = _span.enter();
    }

    fn multiple_expressions(_block_body_hash: String, _block_number: u64, _block_body_size: u64) {
        let _span = trace_span!(
            ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = compute_hash("block"),
            block_number = get_slot(),
            block_body_size = 512_u64 * 2
        );
        let _guard = _span.enter();
    }

    fn complex_expressions(_block_body_hash: String, _block_number: u64, _block_body_size: u64) {
        let _span = trace_span!(
            ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = format!("prefix_{}", "computed"),
            block_number = 42_u64 + 100,
            block_body_size = 1024
        );
        let _guard = _span.enter();
    }

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

    // Extra function parameters do not affect explicit trace_span field assignment.
    // Only the fields named in the macro invocation are validated and traced.

    // This works because trace_span only considers the explicit field assignments below.
    fn with_extra_params(
        block_body_hash: String, // Schema field - traced
        block_number: u64,       // Schema field - traced
        block_body_size: u64,    // Schema field - traced
        context: &str,           // Extra param - not referenced by trace_span
        config: Option<u32>,     // Extra param - not referenced by trace_span
    ) {
        let _span = trace_span!(
            ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = &block_body_hash,
            block_number = block_number,
            block_body_size = block_body_size
        );
        let _guard = _span.enter();
        let _ = (context, config);
    }

    fn with_extra_refs(
        block_body_hash: String, // Schema field - traced
        block_number: u64,       // Schema field - traced
        block_body_size: u64,    // Schema field - traced
        extra_ref: &str,         // Extra param - not referenced
        extra_slice: &[u8],      // Extra param - not referenced
    ) {
        let _span = trace_span!(
            ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = &block_body_hash,
            block_number = block_number,
            block_body_size = block_body_size
        );
        let _guard = _span.enter();
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
        trace_record!(ledger::state::RESOLVE_INPUTS, resolved_from_context = _resolved_from_context);
    }

    fn augment_with_multiple(_resolved_from_context: u64, _resolved_from_volatile: u64) {
        trace_record!(
            ledger::state::RESOLVE_INPUTS,
            resolved_from_context = _resolved_from_context,
            resolved_from_volatile = _resolved_from_volatile
        );
    }

    fn augment_with_extra(_resolved_from_context: u64, extra: &str) {
        trace_record!(ledger::state::RESOLVE_INPUTS, resolved_from_context = _resolved_from_context);
        let _ = extra;
    }

    fn augment_with_expression() {
        trace_record!(ledger::state::RESOLVE_INPUTS, resolved_from_context = 100_u64);
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

    fn network_namespace(peer: String, intersection_slot: u64) {
        let _span = trace_span!(
            network::chainsync_client::FIND_INTERSECTION,
            peer = &peer,
            intersection_slot = intersection_slot
        );
        let _guard = _span.enter();
    }

    fn consensus_namespace(hash: String) {
        let _span = trace_span!(consensus::validate_header::EVOLVE_NONCE, hash = &hash);
        let _guard = _span.enter();
    }

    fn ledger_namespace(point_slot: u64) {
        let _span = trace_span!(ledger::state::APPLY_BLOCK, point_slot = point_slot);
        let _guard = _span.enter();
    }

    #[test]
    fn test_namespace_paths() {
        network_namespace("peer".into(), 100);
        consensus_namespace("hash".into());
        ledger_namespace(42);
    }
}

mod async_functions {
    use std::{future::Future, pin::Pin};

    use async_trait::async_trait;
    use tracing_subscriber::Registry;

    use super::*;

    async fn traced_async(point_slot: u64) -> u64 {
        async move { point_slot }.instrument(trace_span!(ledger::state::APPLY_BLOCK, point_slot = point_slot)).await
    }

    fn traced_boxed_async_like(point_slot: u64) -> Pin<Box<dyn Future<Output = bool> + Send>> {
        Box::pin(
            async move { tracing::Span::current().metadata().is_some() }
                .instrument(trace_span!(ledger::state::APPLY_BLOCK, point_slot = point_slot)),
        )
    }

    #[async_trait]
    trait AsyncValidator {
        async fn validate(&self, point_slot: u64) -> bool;
    }

    struct Validator;

    #[async_trait]
    impl AsyncValidator for Validator {
        async fn validate(&self, point_slot: u64) -> bool {
            async move { tracing::Span::current().metadata().is_some() }
                .instrument(trace_span!(ledger::state::APPLY_BLOCK, point_slot = point_slot))
                .await
        }
    }

    fn assert_send<T: Send>(_: T) {}

    fn traced_async_with_trace_span(point_slot: u64) -> impl Future<Output = bool> + Send {
        async move { tracing::Span::current().metadata().is_some() }
            .instrument(trace_span!(ledger::state::APPLY_BLOCK, point_slot = point_slot))
    }

    #[test]
    fn test_traced_async_future_is_send() {
        assert_send(traced_async(42));
    }

    #[test]
    fn test_trace_span_instrument_future_is_send() {
        assert_send(traced_async_with_trace_span(42));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_boxed_async_like_runs_inside_span() {
        let _guard = tracing::subscriber::set_default(Registry::default());
        assert!(traced_boxed_async_like(42).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_async_trait_method_runs_inside_span() {
        let _guard = tracing::subscriber::set_default(Registry::default());
        assert!(Validator.validate(42).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_trace_span_instrument_runs_inside_span() {
        let _guard = tracing::subscriber::set_default(Registry::default());
        assert!(traced_async_with_trace_span(42).await);
    }
}
