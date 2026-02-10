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

use amaru_observability::define_schemas;
use amaru_observability::trace_span;

define_schemas! {
    test {
        span_test {
            /// Test span event for validation
            SpanEvent {
                required: [
                    event_type: &str,
                    timestamp: u64,
                ],
                optional: [
                    status: &str,
                    error_code: u32,
                    duration_ms: u64,
                    message: &str,
                    details: &str,
                ],
            }
        }
    }
}

#[test]
fn test_trace_span_without_fields() {
    // Create a span without any fields
    let span = trace_span!(amaru::test::span_test::SpanEvent);
    let _guard = span.enter();
    // Span is successfully created and entered
}

#[test]
fn test_trace_span_with_single_field() {
    // Create a span with a single field
    let span = trace_span!(amaru::test::span_test::SpanEvent, status = "active");
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_multiple_fields() {
    // Create a span with multiple fields
    let span = trace_span!(
        amaru::test::span_test::SpanEvent,
        status = "completed",
        error_code = 0,
        duration_ms = 42
    );
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_expressions() {
    // Create a span with computed values
    let status_str = String::from("running");
    let duration: u64 = 100;

    let span = trace_span!(
        amaru::test::span_test::SpanEvent,
        status = status_str.as_str(),
        duration_ms = duration
    );
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_format_macro() {
    // Create a span with format! macro (contains commas in the expression)
    let a = "hello";
    let b = "world";

    let span = trace_span!(
        amaru::test::span_test::SpanEvent,
        message = &format!("{}, {}", a, b)
    );
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_function_call_multiple_args() {
    // Create a span with function call containing multiple arguments
    fn join_strings(s1: &str, s2: &str, sep: &str) -> String {
        format!("{}{}{}", s1, sep, s2)
    }

    let span = trace_span!(
        amaru::test::span_test::SpanEvent,
        message = &join_strings("error", "code", ": ")
    );
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_tuple_values() {
    // Create a span with tuple-like expressions
    let status = "failed";
    let code = 500u32;

    let span = trace_span!(
        amaru::test::span_test::SpanEvent,
        status = status,
        error_code = code
    );
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_array_construction() {
    // Create a span with array literal (contains commas)
    let details_array = ["item1", "item2", "item3"];
    let details_str = details_array.join(", ");

    let span = trace_span!(amaru::test::span_test::SpanEvent, details = &details_str);
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_method_chain() {
    // Create a span with method chaining and multiple arguments
    let message = "  error  ".trim().to_uppercase();

    let span = trace_span!(amaru::test::span_test::SpanEvent, message = &message);
    let _guard = span.enter();
}

#[test]
fn test_trace_span_with_complex_nested_expr() {
    // Create a span with complex nested expression containing multiple commas
    let items = vec!["a", "b", "c"];
    let filtered = items.iter().filter(|s| s.len() > 0).collect::<Vec<_>>();

    let span = trace_span!(
        amaru::test::span_test::SpanEvent,
        details = &format!("Items: {} total", filtered.len())
    );
    let _guard = span.enter();
}

#[test]
fn test_trace_span_nested_scopes() {
    // Test that nested trace_span! calls work correctly
    let outer_span = trace_span!(amaru::test::span_test::SpanEvent, status = "outer");
    let _outer_guard = outer_span.enter();

    {
        let inner_span = trace_span!(
            amaru::test::span_test::SpanEvent,
            status = "inner",
            error_code = 0
        );
        let _inner_guard = inner_span.enter();
    }
}
